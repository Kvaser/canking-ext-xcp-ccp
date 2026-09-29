//! Runtime DAQ-frame decoder.

use protocol_core::layout::{
    daq_layout, effective_bit_length, resolve_signal, signal_lookup, timestamp_bytes, Field,
    SignalPlacement,
};
use protocol_core::protocol::Protocol;
use protocol_core::types::{A2lProject, ByteOrder, DaqList};
use std::collections::HashMap;
use std::sync::Arc;

/// Where a single signal lives inside a DAQ frame, and how to scale it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SignalLayout {
    pub(crate) name: Arc<str>,
    pub(crate) pid: u8,
    pub(crate) start_byte: usize,
    pub(crate) size: usize,
    pub(crate) is_signed: bool,
    pub(crate) is_float: bool,
    pub(crate) big_endian: bool,
    pub(crate) bit_length: u32,
    pub(crate) shift: u32,
    pub(crate) masked: bool,
    pub(crate) factor: f64,
    pub(crate) offset: f64,
    pub(crate) unit: Arc<str>,
}

impl SignalLayout {
    /// Frame bit (`byte * 8 + bit`) of the value's least significant bit, which is what CanKing expects as
    /// `start-bit` for both byte orders. Big-endian values keep their LSB in the last byte.
    fn lsb_start_bit(&self) -> u32 {
        if self.big_endian {
            (self.start_byte as u32 + self.size as u32 - 1 - self.shift / 8) * 8 + self.shift % 8
        } else {
            self.start_byte as u32 * 8 + self.shift
        }
    }

    /// A layout for a raw pass-through value: unscaled, unsigned, little-endian, unmasked.
    fn raw(name: Arc<str>, pid: u8, start_byte: usize, size: usize) -> Self {
        SignalLayout {
            name,
            pid,
            start_byte,
            size,
            is_signed: false,
            is_float: false,
            big_endian: false,
            bit_length: size as u32 * 8,
            shift: 0,
            masked: false,
            factor: 1.0,
            offset: 0.0,
            unit: Arc::from(""),
        }
    }

    /// A layout decoding `p` at `pid`, narrowed by its `BIT_MASK` if it has one.
    fn from_placement(pid: u8, p: &SignalPlacement) -> Self {
        let is_float = p.is_float();
        let (shift, masked) = if is_float {
            (0, false)
        } else {
            let mask = p.bit_mask();
            let shift = mask.map(|m| m.trailing_zeros()).unwrap_or(0);
            let masked = mask
                .is_some_and(|m| shift > 0 || effective_bit_length(m) < p.span_bytes as u32 * 8);
            (shift, masked)
        };
        SignalLayout {
            name: Arc::from(p.name.as_str()),
            pid,
            start_byte: p.byte_offset() as usize,
            size: p.span_bytes as usize,
            is_signed: p.is_signed(),
            is_float,
            big_endian: p.byte_order() == ByteOrder::MsbFirst,
            bit_length: p.bit_length,
            shift,
            masked,
            factor: p.factor,
            offset: p.offset,
            unit: Arc::from(p.unit.as_str()),
        }
    }
}

/// A decoded signal value extracted from a frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DecodedSignal {
    pub(crate) name: Arc<str>,
    pub(crate) value: f64,
    pub(crate) unit: Arc<str>,
}

/// Where a signal's raw bits live within a frame and how to interpret them, in bits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LayoutMeta {
    pub(crate) start_bit: u32,
    pub(crate) length: u32,
    pub(crate) big_endian: bool,
    pub(crate) is_signed: bool,
    pub(crate) is_float: bool,
}

/// How to turn a signal's raw value into a meaningful physical value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConversionMeta {
    Linear { factor: f64, offset: f64 },
    Enum(Vec<(String, f64)>),
}

/// UI-facing metadata for one decoded signal, used to build the frame definitions reported to CanKing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SignalMeta {
    pub(crate) name: String,
    pub(crate) can_id: u32,
    pub(crate) pid: u8,
    pub(crate) comment: Option<String>,
    pub(crate) unit: Option<String>,
    pub(crate) min_value: Option<f64>,
    pub(crate) max_value: Option<f64>,
    pub(crate) layout: LayoutMeta,
    pub(crate) conversion: ConversionMeta,
}

/// The outcome of applying a slave's first-PID report to a DAQ list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PidUpdate {
    /// There is no DAQ list at the reported index, so the report cannot be attributed.
    NoSuchList,
    /// The list already decodes at the reported PID.
    Unchanged,
    /// The list was re-keyed; it previously decoded at `previous`.
    Applied { previous: u8 },
}

/// One DAQ list's decode layout, held independently of the PIDs it lands on.
#[derive(Debug, Clone)]
struct ListLayout {
    id: u16,
    can_id: u32,
    configured_first_pid: u8,
    first_pid: u8,
    odts: Vec<Vec<SignalLayout>>,
}

/// Decodes raw DAQ frames into signal values.
///
/// Most DAQ lists share the module's default CAN id, but the A2L may give a list
/// its own (via XCP's `DAQ_LIST_CAN_ID` or CCP's `CAN_ID_FIXED`), so lists and
/// their PIDs are keyed by `(CAN id, PID)`, not PID alone.
#[derive(Debug, Clone)]
pub(crate) struct Decoder {
    /// `CAN_ID_SLAVE` / `dto_id`: where command/response traffic always arrives, regardless of any per-list DTO override.
    default_can_id: u32,
    lists: Vec<ListLayout>,
    by_can_pid: HashMap<(u32, u8), Vec<(usize, usize)>>,
}

impl Decoder {
    /// Build a decoder from the project metadata and the compiled DAQ lists.
    ///
    /// Returns `None` if the module's default CAN id is absent, in which case there
    /// is nothing to decode frames from.
    pub(crate) fn build(
        project: &A2lProject,
        daq_lists: &[DaqList],
        protocol: Protocol,
    ) -> Option<Self> {
        let can_id_slave = match protocol {
            Protocol::Xcp => project
                .xcp_data
                .as_ref()
                .and_then(|x| x.xcp_info.as_ref())
                .and_then(|c| c.can_id_slave)?,
            Protocol::Ccp => project.ccp_data.as_ref().map(|c| c.info.dto_id)?,
        };

        // Where every field lands, computed once by the shared layout
        let ts_bytes = timestamp_bytes(project);
        let mut fields = daq_layout(project, daq_lists).into_iter();

        let mut lists: Vec<ListLayout> = Vec::with_capacity(daq_lists.len());
        for daq in daq_lists {
            let configured_first_pid = daq.first_pid as u8;
            let mut odts: Vec<Vec<SignalLayout>> = Vec::with_capacity(daq.odts.len());

            for (odt_idx, odt) in daq.odts.iter().enumerate() {
                let pid = configured_first_pid.wrapping_add(odt_idx as u8);
                let mut layouts: Vec<SignalLayout> = Vec::new();

                if odt_idx == 0 && ts_bytes > 0 {
                    let Some(Field::Timestamp(_)) = fields.next() else {
                        unreachable!("daq_layout must emit this ODT's timestamp field first");
                    };
                    layouts.push(SignalLayout::raw(
                        Arc::from(format!("Timestamp_{pid}")),
                        pid,
                        1,
                        ts_bytes as usize,
                    ));
                }

                for entry in &odt.entries {
                    for _ in &entry.names {
                        let Some(Field::Signal(placement)) = fields.next() else {
                            unreachable!("daq_layout must emit one field per entry name");
                        };
                        layouts.push(SignalLayout::from_placement(pid, &placement));
                    }
                }

                odts.push(layouts);
            }

            lists.push(ListLayout {
                id: daq.id,
                can_id: daq.can_id.resolve(can_id_slave),
                configured_first_pid,
                first_pid: configured_first_pid,
                odts,
            });
        }

        let mut decoder = Decoder {
            default_can_id: can_id_slave,
            lists,
            by_can_pid: HashMap::new(),
        };
        decoder.rebuild_pid_map();
        Some(decoder)
    }

    /// Metadata for every decoded signal, resolved against the project.
    ///
    /// Chunked array entries (`Name[i]`) inherit the metadata of their parent
    /// signal; names that don't resolve to a MEASUREMENT get name-only
    /// metadata, matching the raw pass-through decoding.
    pub(crate) fn signal_metadata(&self, project: &A2lProject) -> Vec<SignalMeta> {
        let signal_map = signal_lookup(project);

        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (can_id, pid) in self.expected_can_pids() {
            for l in self.by_can_pid[&(can_id, pid)]
                .iter()
                .flat_map(|&(list_idx, odt_idx)| &self.lists[list_idx].odts[odt_idx])
            {
                if !seen.insert(l.name.clone()) {
                    continue;
                }
                let name = l.name.to_string();
                let sig = resolve_signal(&signal_map, &name);
                let bit_layout = LayoutMeta {
                    start_bit: l.lsb_start_bit(),
                    length: l.bit_length,
                    big_endian: l.big_endian,
                    is_signed: l.is_signed,
                    is_float: l.is_float,
                };
                out.push(match sig {
                    Some(sig) => {
                        let (min, max) = sig.physical_limits(l.bit_length, l.factor, l.offset);
                        let enums = sig
                            .conversion
                            .as_ref()
                            .and_then(|c| project.compu_methods.get(&c.identifier))
                            .filter(|cm| !cm.text_table.is_empty())
                            .map(|cm| {
                                cm.text_table
                                    .iter()
                                    .map(|e| (e.text.clone(), e.lower))
                                    .collect()
                            });
                        SignalMeta {
                            name,
                            can_id,
                            pid: l.pid,
                            comment: (!sig.long_identifier.is_empty())
                                .then(|| sig.long_identifier.clone()),
                            unit: (!l.unit.is_empty()).then(|| l.unit.to_string()),
                            min_value: Some(min),
                            max_value: Some(max),
                            layout: bit_layout,
                            conversion: match enums {
                                Some(enums) => ConversionMeta::Enum(enums),
                                None => ConversionMeta::Linear {
                                    factor: l.factor,
                                    offset: l.offset,
                                },
                            },
                        }
                    }
                    None => SignalMeta {
                        name,
                        can_id,
                        pid: l.pid,
                        comment: None,
                        unit: None,
                        min_value: None,
                        max_value: None,
                        layout: bit_layout,
                        conversion: ConversionMeta::Linear {
                            factor: l.factor,
                            offset: l.offset,
                        },
                    },
                });
            }
        }
        out
    }

    /// Decode one CAN frame. Returns an empty vec if the frame is not a recognized DAQ frame
    pub(crate) fn decode(&self, can_id: u32, payload: &[u8]) -> Vec<DecodedSignal> {
        if payload.is_empty() {
            return Vec::new();
        }
        let Some(slots) = self.by_can_pid.get(&(can_id, payload[0])) else {
            return Vec::new();
        };

        let mut out = Vec::new();
        for &(list_idx, odt_idx) in slots {
            for l in &self.lists[list_idx].odts[odt_idx] {
                let end = l.start_byte + l.size;
                if end > payload.len() {
                    // Truncated frame; the rest of this ODT is not there.
                    break;
                }
                let mut raw = read_raw(&payload[l.start_byte..end], l.big_endian);
                if l.masked {
                    raw = (raw >> l.shift) & ((1u64 << l.bit_length) - 1);
                }
                let phys = if l.is_float {
                    match l.size {
                        4 => f32::from_bits(raw as u32) as f64,
                        8 => f64::from_bits(raw),
                        _ => raw as f64,
                    }
                } else if l.is_signed {
                    sign_extend(raw, l.bit_length as usize) as f64
                } else {
                    raw as f64
                };
                out.push(DecodedSignal {
                    name: l.name.clone(),
                    value: phys * l.factor + l.offset,
                    unit: l.unit.clone(),
                });
            }
        }
        out
    }

    /// The PID of a list's ODT 0 as currently understood, with the `DaqList::id` it belongs to.
    pub(crate) fn list_pid(&self, daq_index: usize) -> Option<(u16, u8)> {
        self.lists.get(daq_index).map(|l| (l.id, l.first_pid))
    }

    /// The number of ODTs configured for a list.
    pub(crate) fn odt_count(&self, daq_index: usize) -> Option<usize> {
        self.lists.get(daq_index).map(|l| l.odts.len())
    }

    /// Move a DAQ list's PID range to where the slave says it is.
    pub(crate) fn set_first_pid(&mut self, daq_index: usize, first_pid: u8) -> PidUpdate {
        let Some(list) = self.lists.get_mut(daq_index) else {
            return PidUpdate::NoSuchList;
        };
        let previous = list.first_pid;
        if previous == first_pid {
            return PidUpdate::Unchanged;
        }
        list.first_pid = first_pid;
        self.rebuild_pid_map();
        PidUpdate::Applied { previous }
    }

    /// The lists whose PID assignment has been corrected away from the configuration, as `(DaqList::id, configured_pid, pid_in_force)`.
    pub(crate) fn corrections(&self) -> Vec<(u16, u8, u8)> {
        self.lists
            .iter()
            .filter(|l| l.first_pid != l.configured_first_pid)
            .map(|l| (l.id, l.configured_first_pid, l.first_pid))
            .collect()
    }

    /// Re-stamp `metadata` so every entry reports the PID its signal currently decodes.
    pub(crate) fn restamp_pids(&self, metadata: &mut [SignalMeta]) {
        let mut pids: HashMap<&str, u8> = HashMap::new();
        for (can_id, pid) in self.expected_can_pids() {
            for l in self.by_can_pid[&(can_id, pid)]
                .iter()
                .flat_map(|&(list_idx, odt_idx)| &self.lists[list_idx].odts[odt_idx])
            {
                pids.entry(&l.name).or_insert(pid);
            }
        }

        for meta in metadata.iter_mut() {
            if let Some(&pid) = pids.get(meta.name.as_str()) {
                meta.pid = pid;
            }
        }
        // Stable, so signals sharing a PID keep the order they were laid out in.
        metadata.sort_by_key(|m| m.pid);
    }

    /// Pairs of `DaqList::id` whose PID ranges collide under the current assignment.
    ///
    /// Scoped per CAN id: two lists sharing a PID on different CAN ids are not a conflict.
    pub(crate) fn pid_conflicts(&self) -> Vec<(u16, u16)> {
        let mut out: Vec<(u16, u16)> = Vec::new();
        for slots in self.by_can_pid.values() {
            for (i, &(list_a, _)) in slots.iter().enumerate() {
                for &(list_b, _) in &slots[i + 1..] {
                    if list_a == list_b {
                        continue;
                    }
                    let pair = (self.lists[list_a].id, self.lists[list_b].id);
                    if !out.contains(&pair) {
                        out.push(pair);
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// Every distinct CAN id a DAQ list actually transmits on, ascending order.
    ///
    /// Used to declare frame definitions: a CAN id only belongs here if some list's
    /// DTOs actually land on it, so CanKing isn't told to expect DAQ-shaped payloads
    /// on an id that only ever carries command/response traffic.
    pub(crate) fn can_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.lists.iter().map(|l| l.can_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Every CAN id the host must deliver frames from.
    ///
    /// The default id is included unconditionally because command/response traffic always arrives there
    pub(crate) fn subscription_can_ids(&self) -> Vec<u32> {
        let mut ids = self.can_ids();
        if let Err(pos) = ids.binary_search(&self.default_can_id) {
            ids.insert(pos, self.default_can_id);
        }
        ids
    }

    /// The largest payload length any list on `can_id` needs, in bytes.
    pub(crate) fn frame_data_length(&self, can_id: u32) -> u32 {
        self.lists
            .iter()
            .filter(|list| list.can_id == can_id)
            .flat_map(|list| list.odts.iter())
            .flat_map(|odt| odt.iter())
            .map(|l| (l.start_byte + l.size) as u32)
            .max()
            .unwrap_or(1)
    }

    /// Re-key the PID lookup, and each layout's reported PID, from the first PIDs currently active.
    fn rebuild_pid_map(&mut self) {
        self.by_can_pid.clear();
        for (list_idx, list) in self.lists.iter_mut().enumerate() {
            for (odt_idx, layouts) in list.odts.iter_mut().enumerate() {
                let pid = list.first_pid.wrapping_add(odt_idx as u8);
                for l in layouts.iter_mut() {
                    l.pid = pid;
                }
                self.by_can_pid
                    .entry((list.can_id, pid))
                    .or_default()
                    .push((list_idx, odt_idx));
            }
        }
    }
    /// Every `(CAN id, PID)` pair the assignment in force expects, ascending order.
    fn expected_can_pids(&self) -> Vec<(u32, u8)> {
        let mut pairs: Vec<(u32, u8)> = self.by_can_pid.keys().copied().collect();
        pairs.sort_unstable();
        pairs
    }
}

/// Read a raw integer from a byte slice, respecting endianness.
fn read_raw(bytes: &[u8], big_endian: bool) -> u64 {
    let mut v: u64 = 0;
    if big_endian {
        for &b in bytes {
            v = (v << 8) | b as u64;
        }
    } else {
        for (i, &b) in bytes.iter().enumerate() {
            v |= (b as u64) << (8 * i);
        }
    }
    v
}

/// Sign-extend a raw integer of `bits` width to a signed 64-bit integer.
fn sign_extend(raw: u64, bits: usize) -> i64 {
    if bits == 0 || bits >= 64 {
        return raw as i64;
    }
    let sign_bit = 1u64 << (bits - 1);
    if raw & sign_bit != 0 {
        (raw as i64) - (1i64 << bits)
    } else {
        raw as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use protocol_core::types::{CanIdSource, DaqList, Odt, OdtEntry, ParsedSignal};

    fn two_list_config() -> Config {
        let mut cfg = Config::sample();
        let mut second = ParsedSignal {
            name: "Load".to_string(),
            long_identifier: "Engine load".to_string(),
            conversion: None,
            ecu_address: 0x2000,
            size: 1,
            event: Some("Task100ms".to_string()),
            ..Default::default()
        };
        second.upper_limit = 255.0;
        cfg.project.signals.push(second);
        cfg.daq_lists.push(DaqList {
            id: 1,
            first_pid: 1,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["Load".to_string()],
                    ecu_address: 0x2000,
                    size: 1,
                }],
            }],
            event: Some("Task100ms".to_string()),
            can_id: CanIdSource::Default,
        });
        cfg
    }

    /// `two_list_config`, but list 1 (`Load`) transmits on its own CAN id.
    fn two_can_id_config() -> Config {
        let mut cfg = two_list_config();
        cfg.daq_lists[1].can_id = CanIdSource::Fixed { id: 0x789 };
        cfg
    }

    fn build(cfg: &Config) -> Decoder {
        Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap()
    }

    #[test]
    fn set_first_pid_rekeys_the_decoder() {
        let cfg = Config::sample();
        let mut decoder = build(&cfg);
        assert_eq!(decoder.decode(0x456, &[0x00, 0x90, 0x01]).len(), 1);

        assert_eq!(
            decoder.set_first_pid(0, 0x12),
            PidUpdate::Applied { previous: 0x00 }
        );
        assert!(
            decoder.decode(0x456, &[0x00, 0x90, 0x01]).is_empty(),
            "the configured PID must stop decoding once it is known to be wrong"
        );
        let decoded = decoder.decode(0x456, &[0x12, 0x90, 0x01]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name.as_ref(), "Rpm");
        assert_eq!(decoded[0].value, 200.0);
        assert_eq!(decoder.corrections(), [(0, 0x00, 0x12)]);
        assert_eq!(decoder.list_pid(0), Some((0, 0x12)));

        assert_eq!(decoder.set_first_pid(0, 0x12), PidUpdate::Unchanged);
        assert_eq!(decoder.set_first_pid(9, 0x12), PidUpdate::NoSuchList);
    }

    #[test]
    fn correcting_one_list_leaves_the_others_alone() {
        let cfg = two_list_config();
        let mut decoder = build(&cfg);

        assert_eq!(
            decoder.set_first_pid(1, 0x12),
            PidUpdate::Applied { previous: 0x01 }
        );
        assert_eq!(
            decoder.decode(0x456, &[0x00, 0x90, 0x01]).len(),
            1,
            "list 0"
        );
        assert!(decoder.decode(0x456, &[0x01, 0x07]).is_empty(), "old PID");
        let decoded = decoder.decode(0x456, &[0x12, 0x07]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name.as_ref(), "Load");
        assert_eq!(decoded[0].value, 7.0);
        assert_eq!(decoder.corrections(), [(1, 0x01, 0x12)]);
    }

    #[test]
    fn subscription_keeps_the_default_id_even_when_every_list_moves_off_it() {
        let mut cfg = two_list_config();
        cfg.daq_lists[0].can_id = CanIdSource::Fixed { id: 0x789 };
        cfg.daq_lists[1].can_id = CanIdSource::Fixed { id: 0x78A };
        let decoder = build(&cfg);

        assert_eq!(
            decoder.can_ids(),
            [0x789, 0x78A],
            "no list transmits on 0x456, so it isn't a DAQ frame definition"
        );
        assert_eq!(
            decoder.subscription_can_ids(),
            [0x456, 0x789, 0x78A],
            "0x456 must still be subscribed to, for command/response traffic"
        );
    }

    #[test]
    fn lists_on_different_can_ids_decode_independently() {
        let cfg = two_can_id_config();
        let decoder = build(&cfg);
        assert_eq!(decoder.can_ids(), [0x456, 0x789]);

        assert_eq!(decoder.decode(0x456, &[0x00, 0x90, 0x01]).len(), 1);
        assert!(decoder.decode(0x456, &[0x01, 0x07]).is_empty());
        let decoded = decoder.decode(0x789, &[0x01, 0x07]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name.as_ref(), "Load");
        assert_eq!(decoded[0].value, 7.0);
    }

    #[test]
    fn same_pid_on_different_can_ids_is_not_a_conflict() {
        let mut cfg = two_can_id_config();
        cfg.daq_lists[1].first_pid = 0;
        let decoder = build(&cfg);
        assert!(decoder.pid_conflicts().is_empty());
    }

    #[test]
    fn signal_metadata_reports_which_can_id_each_signal_is_on() {
        let cfg = two_can_id_config();
        let decoder = build(&cfg);
        let meta = decoder.signal_metadata(&cfg.project);
        let rpm = meta.iter().find(|m| m.name == "Rpm").expect("Rpm");
        assert_eq!(rpm.can_id, 0x456);
        let load = meta.iter().find(|m| m.name == "Load").expect("Load");
        assert_eq!(load.can_id, 0x789);
    }

    #[test]
    fn frame_data_length_is_scoped_to_can_id() {
        let cfg = two_can_id_config();
        let decoder = build(&cfg);
        assert_eq!(decoder.frame_data_length(0x456), 3, "PID byte + 2-byte Rpm");
        assert_eq!(
            decoder.frame_data_length(0x789),
            2,
            "PID byte + 1-byte Load"
        );
    }

    #[test]
    fn signal_metadata_reports_the_corrected_pid() {
        let cfg = two_list_config();
        let mut decoder = build(&cfg);
        decoder.set_first_pid(1, 0x12);

        let meta = decoder.signal_metadata(&cfg.project);
        let load = meta.iter().find(|m| m.name == "Load").expect("Load");
        assert_eq!(load.pid, 0x12, "the host's multiplex value must follow");
        let rpm = meta.iter().find(|m| m.name == "Rpm").expect("Rpm");
        assert_eq!(rpm.pid, 0x00);
    }

    #[test]
    fn restamping_matches_resolving_the_project_again() {
        let cfg = two_list_config();
        let mut decoder = build(&cfg);

        let mut restamped = decoder.signal_metadata(&cfg.project);
        decoder.set_first_pid(1, 0x12);
        decoder.restamp_pids(&mut restamped);

        assert_eq!(
            restamped,
            decoder.signal_metadata(&cfg.project),
            "restamping must be indistinguishable from re-resolving the A2L project"
        );
    }

    #[test]
    fn overlapping_pid_ranges_are_reported_as_conflicts() {
        let cfg = two_list_config();
        let mut decoder = build(&cfg);
        assert!(
            decoder.pid_conflicts().is_empty(),
            "0x00 and 0x01 are apart"
        );

        decoder.set_first_pid(1, 0x00);
        assert_eq!(decoder.pid_conflicts(), [(0, 1)]);

        decoder.set_first_pid(1, 0x12);
        assert!(decoder.pid_conflicts().is_empty());
    }

    #[test]
    fn decodes_scaled_unsigned_signal() {
        let cfg = Config::sample();
        let decoder = Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap();

        let frame = [0x00, 0x90, 0x01];
        let decoded = decoder.decode(0x456, &frame);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name.as_ref(), "Rpm");
        assert_eq!(decoded[0].value, 200.0);
        assert_eq!(decoded[0].unit.as_ref(), "rpm");
    }

    #[test]
    fn ignores_other_can_ids_and_unknown_pids() {
        let cfg = Config::sample();
        let decoder = Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap();
        assert!(decoder.decode(0x123, &[0x00, 0x90, 0x01]).is_empty());
        assert!(decoder.decode(0x456, &[0x09, 0x90, 0x01]).is_empty());
    }

    #[test]
    fn truncated_frame_is_skipped() {
        let cfg = Config::sample();
        let decoder = Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap();
        assert!(decoder.decode(0x456, &[0x00]).is_empty());
    }

    #[test]
    fn signal_metadata_resolves_project_info() {
        let cfg = Config::sample();
        let decoder = Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap();
        let meta = decoder.signal_metadata(&cfg.project);
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].name, "Rpm");
        assert_eq!(meta[0].pid, 0);
        assert_eq!(meta[0].comment.as_deref(), Some("Engine speed"));
        assert_eq!(meta[0].unit.as_deref(), Some("rpm"));
        assert_eq!(meta[0].min_value, Some(0.0));
        assert_eq!(meta[0].max_value, Some(8000.0));
        assert!(matches!(meta[0].conversion, ConversionMeta::Linear { .. }));
    }

    #[test]
    fn fixed_timestamp_is_decoded_as_own_signal() {
        let mut cfg = Config::sample();
        let daq_info = &mut cfg.project.xcp_data.as_mut().unwrap().daq_info;
        daq_info.timestamp_fixed = true;
        daq_info.timestamp_size = 4;

        let decoder = Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap();

        let frame = [0x00, 0x10, 0x00, 0x00, 0x00, 0x90, 0x01];
        let decoded = decoder.decode(0x456, &frame);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].name.as_ref(), "Timestamp_0");
        assert_eq!(decoded[0].value, 16.0);
        assert_eq!(decoded[1].name.as_ref(), "Rpm");
        assert_eq!(decoded[1].value, 200.0);

        let meta = decoder.signal_metadata(&cfg.project);
        assert_eq!(meta[0].name, "Timestamp_0");
        assert_eq!(meta[0].pid, 0);
    }

    #[test]
    fn masked_bitfield_signal_decodes_only_its_own_bits() {
        let mut cfg = Config::sample();
        let masked = ParsedSignal {
            name: "Masked".to_string(),
            long_identifier: String::new(),
            conversion: None,
            ecu_address: 0x3000,
            size: 2,
            bit_mask: Some(0x0FF0),
            event: Some("Task100ms".to_string()),
            ..Default::default()
        };
        cfg.project.signals.push(masked);
        cfg.daq_lists.push(DaqList {
            id: 1,
            first_pid: 1,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["Masked".to_string()],
                    ecu_address: 0x3000,
                    size: 2,
                }],
            }],
            event: Some("Task100ms".to_string()),
            can_id: CanIdSource::Default,
        });

        let decoder = Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap();

        let decoded = decoder.decode(0x456, &[0x01, 0xF0, 0x00]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name.as_ref(), "Masked");
        assert_eq!(
            decoded[0].value, 15.0,
            "only the masked byte's middle 8 bits, shifted into place"
        );

        let meta = decoder.signal_metadata(&cfg.project);
        let masked_meta = meta.iter().find(|m| m.name == "Masked").expect("Masked");
        assert_eq!(
            masked_meta.layout.length, 8,
            "the reported width follows BIT_MASK, not the declared 2-byte size"
        );
        assert_eq!(
            masked_meta.layout.start_bit,
            8 + 4,
            "byte 1, then the mask's shift"
        );
    }

    #[test]
    fn full_width_bit_mask_is_a_no_op() {
        let mut cfg = Config::sample();
        let unmasked = ParsedSignal {
            name: "FullMask".to_string(),
            long_identifier: String::new(),
            conversion: None,
            ecu_address: 0x4000,
            size: 2,
            bit_mask: Some(0xFFFF),
            event: Some("Task100ms".to_string()),
            ..Default::default()
        };
        cfg.project.signals.push(unmasked);
        cfg.daq_lists.push(DaqList {
            id: 1,
            first_pid: 1,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["FullMask".to_string()],
                    ecu_address: 0x4000,
                    size: 2,
                }],
            }],
            event: Some("Task100ms".to_string()),
            can_id: CanIdSource::Default,
        });

        let decoder = Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap();

        let decoded = decoder.decode(0x456, &[0x01, 0x34, 0x12]);
        assert_eq!(decoded[0].value, 0x1234 as f64);

        let meta = decoder.signal_metadata(&cfg.project);
        let m = meta
            .iter()
            .find(|m| m.name == "FullMask")
            .expect("FullMask");
        assert_eq!(m.layout.length, 16);
        assert_eq!(
            m.layout.start_bit, 8,
            "no shift: the mask covers the whole span"
        );
    }

    #[test]
    fn start_bit_is_the_lsb_for_both_byte_orders() {
        let at = |start_byte, size, big_endian, shift| SignalLayout {
            big_endian,
            shift,
            ..SignalLayout::raw(Arc::from("x"), 0, start_byte, size)
        };
        assert_eq!(at(1, 2, false, 0).lsb_start_bit(), 8);
        assert_eq!(
            at(1, 4, true, 0).lsb_start_bit(),
            32,
            "4-byte BE in bytes 1-4: LSB is byte 4 bit 0"
        );
        assert_eq!(
            at(5, 2, true, 0).lsb_start_bit(),
            48,
            "2-byte BE in bytes 5-6: LSB is byte 6 bit 0"
        );
        assert_eq!(
            at(1, 2, true, 4).lsb_start_bit(),
            20,
            "masked BE, shift 4: byte 2 bit 4"
        );
        assert_eq!(
            at(1, 2, true, 12).lsb_start_bit(),
            12,
            "masked BE, shift 12: byte 1 bit 4"
        );
    }

    #[test]
    fn sign_extend_handles_negative() {
        assert_eq!(sign_extend(0xFFFF, 16), -1);
        assert_eq!(sign_extend(0x8000, 16), -32768);
        assert_eq!(sign_extend(0x7FFF, 16), 32767);
    }
}
