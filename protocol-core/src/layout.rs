//! Where each DAQ signal lands in its CAN frame.
//!
//! The DBC writer, the standalone (DBC-free) t-script generator, and the CanKing
//! runtime decoder all need the same answer to "which bytes of which frame hold
//! signal X, and how is it scaled".

use crate::protocol::Protocol;
use crate::types::{A2lProject, ByteOrder, DaqList, OdtEntry, ParsedSignal};
use std::collections::HashMap;

/// One field of a DAQ frame, in the order the DBC declares them.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Timestamp(Timestamp),
    Signal(Box<SignalPlacement>),
}

/// The timestamp field at the head of a DAQ list's first ODT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    pub pid: u16,
    pub start_bit: u32,
    pub bit_length: u32,
}

/// A signal's position and scaling within the DAQ frame carrying it.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalPlacement {
    pub name: String,
    pub pid: u16,
    pub start_bit: u32,
    pub bit_length: u32,
    pub span_bytes: u16,
    pub signal: Option<ParsedSignal>,
    pub chunk_size: Option<u16>,
    pub factor: f64,
    pub offset: f64,
    pub unit: String,
}

impl SignalPlacement {
    /// Byte offset of the field's first byte from the start of the frame.
    pub fn byte_offset(&self) -> u32 {
        self.start_bit / 8
    }

    /// Byte order, defaulting to little endian (A2L `MSB_LAST`).
    pub fn byte_order(&self) -> ByteOrder {
        self.signal
            .as_ref()
            .and_then(|s| s.byte_order.clone())
            .unwrap_or(ByteOrder::MsbLast)
    }

    pub fn is_signed(&self) -> bool {
        self.signal.as_ref().is_some_and(|s| s.is_signed)
    }

    pub fn is_float(&self) -> bool {
        self.signal.as_ref().is_some_and(|s| s.is_float)
    }

    /// The A2L `BIT_MASK`, ignored for split chunks.
    pub fn bit_mask(&self) -> Option<u64> {
        if self.chunk_size.is_some() {
            return None;
        }
        self.signal.as_ref().and_then(|s| s.bit_mask)
    }
}

/// The CAN id every DAQ frame arrives on or `None` when the A2L declares no transport for `protocol`.
pub fn daq_can_id(project: &A2lProject, protocol: Protocol) -> Option<u32> {
    match protocol {
        Protocol::Xcp => project
            .xcp_data
            .as_ref()
            .and_then(|x| x.xcp_info.as_ref())
            .and_then(|c| c.can_id_slave),
        Protocol::Ccp => project.ccp_data.as_ref().map(|c| c.info.dto_id),
    }
}

/// Group DAQ lists by the CAN id they actually transmit on, given the transport's shared default id. Ascending by id.
///
/// Most lists share the module's default id, but a list may declare its own via XCP's `DAQ_LIST_CAN_ID` or CCP's `CAN_ID_FIXED` (see [`crate::types::CanIdSource`]).
///
/// A caller that needs one message/hook per CAN id (the DBC writer, the standalone t-script generator) packs each group through the normal single-CAN-id layout pass.
pub fn group_by_can_id(daq_lists: &[DaqList], default_id: u32) -> Vec<(u32, Vec<DaqList>)> {
    let mut groups: Vec<(u32, Vec<DaqList>)> = Vec::new();
    for list in daq_lists {
        let id = list.can_id.resolve(default_id);
        match groups.iter_mut().find(|(gid, _)| *gid == id) {
            Some((_, lists)) => lists.push(list.clone()),
            None => groups.push((id, vec![list.clone()])),
        }
    }
    groups.sort_by_key(|(id, _)| *id);
    groups
}

/// Number of timestamp bytes the ECU prepends to the first ODT of each DAQ list.
pub fn timestamp_bytes(project: &A2lProject) -> u16 {
    project
        .xcp_data
        .as_ref()
        .map(|x| x.daq_info.timestamp_size)
        .unwrap_or(0)
}

/// Build a lookup from signal name to its A2L definition.
pub fn signal_lookup(project: &A2lProject) -> HashMap<&str, &ParsedSignal> {
    project
        .signals
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect()
}

/// Resolve a signal name to its A2L definition: an exact match first, else the parent of a chunked array entry (`Name[i]`).
pub fn resolve_signal<'a>(
    signal_map: &HashMap<&str, &'a ParsedSignal>,
    name: &str,
) -> Option<&'a ParsedSignal> {
    signal_map.get(name).copied().or_else(|| {
        name.rfind('[')
            .map(|i| &name[..i])
            .and_then(|base| signal_map.get(base))
            .copied()
    })
}

/// The `ECU_ADDRESS_EXTENSION` an entry is sampled with.
///
/// Packing never merges signals with different extensions into one entry, so its first signal speaks for all of them.
pub fn address_extension(signal_map: &HashMap<&str, &ParsedSignal>, entry: &OdtEntry) -> u8 {
    entry
        .names
        .first()
        .and_then(|name| resolve_signal(signal_map, name))
        .map_or(0, |s| s.address_extension)
}

/// A signal's scaling: `(factor, offset, unit)`, defaulting to unscaled/unitless.
pub fn scaling(project: &A2lProject, sig: &ParsedSignal) -> (f64, f64, String) {
    sig.conversion
        .as_ref()
        .map(|c| c.identifier.as_str())
        .and_then(|id| project.compu_methods.get(id))
        .map(|cm| (cm.factor, cm.offset, cm.unit.clone()))
        .unwrap_or((1.0, 0.0, String::new()))
}

/// Go through the packed DAQ lists and report where every field lands.
pub fn daq_layout(project: &A2lProject, daq_lists: &[DaqList]) -> Vec<Field> {
    let signal_map = signal_lookup(project);
    let timestamp_bytes = timestamp_bytes(project);

    let mut fields = Vec::new();
    for daq in daq_lists {
        for (odt_idx, odt) in daq.odts.iter().enumerate() {
            let pid = daq.first_pid + odt_idx as u16;

            // The ECU places the timestamp to the first ODT of the list only.
            let ts_in_odt = if odt_idx == 0 { timestamp_bytes } else { 0 };
            if ts_in_odt > 0 {
                fields.push(Field::Timestamp(Timestamp {
                    pid,
                    start_bit: 8,
                    bit_length: ts_in_odt as u32 * 8,
                }));
            }
            let mut byte_offset: u32 = ts_in_odt as u32;

            for entry in &odt.entries {
                let entry_start: u32 = 8 + byte_offset * 8;
                let entry_end = entry.ecu_address + entry.size as u64;
                // Where the next name lands when its address can't place it.
                let mut cursor: u16 = 0;
                for name in &entry.names {
                    // Exact lookup first, then try to find a parent signal for a split chunk.
                    let exact = signal_map.get(name.as_str()).map(|s| (*s).clone());
                    let parent = || {
                        name.rfind('[')
                            .map(|i| &name[..i])
                            .and_then(|base| signal_map.get(base))
                            .map(|s| (*s).clone())
                    };
                    // A signal sits at its own address inside the entry, which may start earlier or hold other signals.
                    // A chunk is the part of its parent the entry covers.
                    let (signal, chunk_size, offset_in_entry) = match exact {
                        Some(sig) => {
                            let end = sig.ecu_address + sig.size as u64;
                            let inside = sig.ecu_address >= entry.ecu_address && end <= entry_end;
                            let at = if inside {
                                (sig.ecu_address - entry.ecu_address) as u16
                            } else {
                                cursor
                            };
                            (Some(sig), None, at)
                        }
                        None => {
                            let parent = parent();
                            let covered = parent.as_ref().and_then(|p| {
                                let from = p.ecu_address.max(entry.ecu_address);
                                let to = (p.ecu_address + p.size as u64).min(entry_end);
                                (from < to).then(|| {
                                    ((from - entry.ecu_address) as u16, (to - from) as u16)
                                })
                            });
                            let (at, size) =
                                covered.unwrap_or((cursor, entry.size.saturating_sub(cursor)));
                            (parent, Some(size), at)
                        }
                    };
                    let bit_cursor = entry_start + offset_in_entry as u32 * 8;

                    let bit_length = match (&signal, chunk_size) {
                        (_, Some(size)) => size as u32 * 8,
                        (Some(sig), None) => sig
                            .bit_mask
                            .map(effective_bit_length)
                            .unwrap_or(sig.size as u32 * 8),
                        // Unresolved name: the whole entry passes through raw.
                        (None, None) => entry.size as u32 * 8,
                    };

                    let (factor, offset, unit) = signal
                        .as_ref()
                        .map(|sig| scaling(project, sig))
                        .unwrap_or((1.0, 0.0, String::new()));

                    // A masked signal still occupies its full declared width, so the span is independent of `bit_length`.
                    let span_bytes = match (&signal, chunk_size) {
                        (_, Some(size)) => size,
                        (Some(sig), None) => sig.size,
                        (None, None) => entry.size,
                    };

                    fields.push(Field::Signal(Box::new(SignalPlacement {
                        name: name.clone(),
                        pid,
                        start_bit: bit_cursor,
                        bit_length,
                        span_bytes,
                        signal,
                        chunk_size,
                        factor,
                        offset,
                        unit,
                    })));
                    cursor = offset_in_entry + span_bytes;
                }
                byte_offset += entry.size as u32;
            }
        }
    }
    fields
}

/// Every signal field in the layout, dropping the timestamps.
pub fn signal_placements(project: &A2lProject, daq_lists: &[DaqList]) -> Vec<SignalPlacement> {
    daq_layout(project, daq_lists)
        .into_iter()
        .filter_map(|f| match f {
            Field::Signal(p) => Some(*p),
            Field::Timestamp(_) => None,
        })
        .collect()
}

/// Find where `name` lands
///
/// A signal split across ODT entries only appears under its `Parent[i]` chunk names, so those are matched too.
/// Callers that cannot handle a split can detect it from the returned placement's `chunk_size`.
pub fn find_placement<'a>(
    placements: &'a [SignalPlacement],
    name: &str,
    sanitize: impl Fn(&str) -> String,
) -> Option<&'a SignalPlacement> {
    let ident = sanitize(name);
    placements
        .iter()
        .find(|p| p.name == name || sanitize(&p.name) == ident)
        .or_else(|| {
            placements.iter().find(|p| {
                p.chunk_size.is_some()
                    && p.name.rfind('[').is_some_and(|i| {
                        let parent = &p.name[..i];
                        parent == name || sanitize(parent) == ident
                    })
            })
        })
}

/// Number of bits spanned from the LSB to the MSB of `mask`.
pub fn effective_bit_length(mask: u64) -> u32 {
    if mask == 0 {
        return 0;
    }
    let msb = 63 - mask.leading_zeros();
    let lsb = mask.trailing_zeros();
    msb - lsb + 1
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CanIdSource, CcpData, CcpInfo, CompuMethod, Conversion, DaqInfo, EventInfo, Odt, OdtEntry,
        XcpData, XcpInfo,
    };

    fn signal(name: &str, size: u16) -> ParsedSignal {
        ParsedSignal {
            name: name.into(),
            size,
            ecu_address: 0x10,
            ..Default::default()
        }
    }

    fn one_odt(entries: Vec<OdtEntry>) -> Vec<DaqList> {
        vec![DaqList {
            id: 0,
            first_pid: 0,
            event: None,
            odts: vec![Odt { entries }],
            can_id: CanIdSource::Default,
        }]
    }

    fn placements(fields: &[Field]) -> Vec<&SignalPlacement> {
        fields
            .iter()
            .filter_map(|f| match f {
                Field::Signal(p) => Some(p.as_ref()),
                Field::Timestamp(_) => None,
            })
            .collect()
    }

    #[test]
    fn signals_follow_each_other_through_the_frame() {
        let project = A2lProject {
            signals: vec![signal("A", 2), signal("B", 4)],
            ..Default::default()
        };
        let daq = one_odt(vec![
            OdtEntry {
                names: vec!["A".into()],
                ecu_address: 0x10,
                size: 2,
            },
            OdtEntry {
                names: vec!["B".into()],
                ecu_address: 0x20,
                size: 4,
            },
        ]);
        let fields = daq_layout(&project, &daq);
        let p = placements(&fields);
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[0].start_bit, p[0].bit_length, p[0].byte_offset()),
            (8, 16, 1)
        );
        assert_eq!(
            (p[1].start_bit, p[1].bit_length, p[1].byte_offset()),
            (24, 32, 3)
        );
    }

    #[test]
    fn timestamp_shifts_the_first_odt_only() {
        let project = A2lProject {
            signals: vec![signal("A", 2)],
            xcp_data: Some(XcpData {
                daq_info: DaqInfo {
                    timestamp_size: 4,
                    ..Default::default()
                },
                event_info: EventInfo::default(),
                xcp_info: Some(XcpInfo::default()),
                protocol_layer: None,
            }),
            ..Default::default()
        };
        let entry = OdtEntry {
            names: vec!["A".into()],
            ecu_address: 0x10,
            size: 2,
        };
        let daq = vec![DaqList {
            id: 0,
            first_pid: 3,
            event: None,
            odts: vec![
                Odt {
                    entries: vec![entry.clone()],
                },
                Odt {
                    entries: vec![entry],
                },
            ],
            can_id: CanIdSource::Default,
        }];
        let fields = daq_layout(&project, &daq);

        let timestamps: Vec<&Timestamp> = fields
            .iter()
            .filter_map(|f| match f {
                Field::Timestamp(t) => Some(t),
                Field::Signal(_) => None,
            })
            .collect();
        assert_eq!(timestamps.len(), 1, "only the first ODT carries one");
        assert_eq!(timestamps[0].pid, 3);
        assert_eq!(timestamps[0].bit_length, 32);

        let p = placements(&fields);
        assert_eq!((p[0].pid, p[0].byte_offset()), (3, 5));
        assert_eq!((p[1].pid, p[1].byte_offset()), (4, 1));
    }

    #[test]
    fn bit_mask_narrows_the_width_but_not_the_stride() {
        let mut masked = signal("M", 2);
        masked.bit_mask = Some(0x0FF0);
        let project = A2lProject {
            signals: vec![masked, signal("After", 1)],
            ..Default::default()
        };
        let daq = one_odt(vec![
            OdtEntry {
                names: vec!["M".into()],
                ecu_address: 0x10,
                size: 2,
            },
            OdtEntry {
                names: vec!["After".into()],
                ecu_address: 0x20,
                size: 1,
            },
        ]);
        let fields = daq_layout(&project, &daq);
        let p = placements(&fields);
        assert_eq!(p[0].bit_length, 8, "0x0FF0 spans 8 bits");
        assert_eq!(p[0].bit_mask(), Some(0x0FF0));
        assert_eq!(
            p[1].byte_offset(),
            3,
            "the masked signal still occupies its declared 2 bytes"
        );
    }

    #[test]
    fn split_chunk_inherits_the_parent_scaling_and_ignores_its_mask() {
        let mut parent = signal("Big", 8);
        parent.bit_mask = Some(0xFF);
        parent.conversion = Some(Conversion {
            identifier: "CM".into(),
        });
        let mut project = A2lProject {
            signals: vec![parent],
            ..Default::default()
        };
        project.compu_methods.insert(
            "CM".into(),
            CompuMethod {
                factor: 0.5,
                offset: -3.0,
                unit: "V".into(),
                ..Default::default()
            },
        );
        let daq = one_odt(vec![OdtEntry {
            names: vec!["Big[0]".into()],
            ecu_address: 0x10,
            size: 4,
        }]);
        let fields = daq_layout(&project, &daq);
        let p = placements(&fields);
        assert_eq!(p[0].chunk_size, Some(4));
        assert_eq!(
            p[0].bit_length, 32,
            "the chunk's width, not the parent's mask"
        );
        assert_eq!(p[0].bit_mask(), None);
        assert_eq!((p[0].factor, p[0].offset), (0.5, -3.0));
        assert_eq!(p[0].unit, "V");
        assert!(p[0].signal.is_some(), "resolved through the parent");
    }

    #[test]
    fn last_chunk_sharing_an_entry_ends_at_its_parent() {
        // Big (10 bytes) split 7+3; the 3-byte tail merged with the adjacent Next.
        let mut big = signal("Big", 10);
        big.ecu_address = 0x100;
        let mut next = signal("Next", 2);
        next.ecu_address = 0x10A;
        let project = A2lProject {
            signals: vec![big, next],
            ..Default::default()
        };
        let daq = one_odt(vec![OdtEntry {
            names: vec!["Big[1]".into(), "Next".into()],
            ecu_address: 0x107,
            size: 5,
        }]);
        let fields = daq_layout(&project, &daq);
        let p = placements(&fields);
        assert_eq!(p[0].chunk_size, Some(3));
        assert_eq!(
            (p[1].name.as_str(), p[1].byte_offset()),
            ("Next", 4),
            "PID + 3 chunk bytes"
        );
    }

    #[test]
    fn chunk_excludes_padding_at_the_end_of_its_entry() {
        let project = A2lProject {
            signals: vec![signal("Big", 9)],
            ..Default::default()
        };
        let daq = one_odt(vec![OdtEntry {
            names: vec!["Big[1]".into()],
            ecu_address: 0x10 + 6,
            size: 4, // 3 bytes left of Big, padded to a WORD multiple
        }]);
        let p = signal_placements(&project, &daq);
        assert_eq!(p[0].chunk_size, Some(3));
    }

    #[test]
    fn signals_sit_at_their_address_in_a_granule_aligned_entry() {
        let mut a = signal("A", 1);
        a.ecu_address = 0x11;
        let mut b = signal("B", 1);
        b.ecu_address = 0x12;
        let project = A2lProject {
            signals: vec![a, b],
            ..Default::default()
        };
        let daq = one_odt(vec![OdtEntry {
            names: vec!["A".into(), "B".into()],
            ecu_address: 0x10,
            size: 4,
        }]);
        let p = signal_placements(&project, &daq);
        assert_eq!(
            (p[0].byte_offset(), p[1].byte_offset()),
            (2, 3),
            "PID + lead byte, then the next byte"
        );
    }

    #[test]
    fn unknown_name_passes_through_as_the_whole_entry() {
        let project = A2lProject::default();
        let daq = one_odt(vec![OdtEntry {
            names: vec!["Ghost".into()],
            ecu_address: 0x10,
            size: 2,
        }]);
        let fields = daq_layout(&project, &daq);
        let p = placements(&fields);
        assert_eq!(p[0].bit_length, 16);
        assert!(p[0].signal.is_none());
        assert_eq!((p[0].factor, p[0].offset), (1.0, 0.0));
        assert!(!p[0].is_signed() && !p[0].is_float());
        assert_eq!(p[0].byte_order(), ByteOrder::MsbLast);
    }

    #[test]
    fn effective_bit_length_spans_lsb_to_msb() {
        assert_eq!(effective_bit_length(0), 0);
        assert_eq!(effective_bit_length(0xFF), 8);
        assert_eq!(effective_bit_length(0x0FF0), 8);
        assert_eq!(effective_bit_length(0x8001), 16);
    }

    #[test]
    fn daq_can_id_reads_the_protocol_transport() {
        let xcp = A2lProject {
            xcp_data: Some(XcpData {
                daq_info: DaqInfo::default(),
                event_info: EventInfo::default(),
                xcp_info: Some(XcpInfo {
                    can_id_slave: Some(0x456),
                    ..Default::default()
                }),
                protocol_layer: None,
            }),
            ..Default::default()
        };
        assert_eq!(daq_can_id(&xcp, Protocol::Xcp), Some(0x456));
        assert_eq!(daq_can_id(&xcp, Protocol::Ccp), None);

        let ccp = A2lProject {
            ccp_data: Some(CcpData {
                info: CcpInfo {
                    dto_id: 0x321,
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(daq_can_id(&ccp, Protocol::Ccp), Some(0x321));
        assert_eq!(daq_can_id(&A2lProject::default(), Protocol::Xcp), None);
    }

    #[test]
    fn group_by_can_id_groups_default_and_fixed_lists_separately() {
        let default_list = DaqList {
            id: 0,
            can_id: CanIdSource::Default,
            ..one_odt(vec![]).remove(0)
        };
        let dynamic_unassigned = DaqList {
            id: 1,
            can_id: CanIdSource::Dynamic { assigned: None },
            ..one_odt(vec![]).remove(0)
        };
        let fixed_list = DaqList {
            id: 2,
            can_id: CanIdSource::Fixed { id: 0x789 },
            ..one_odt(vec![]).remove(0)
        };
        let dynamic_assigned = DaqList {
            id: 3,
            can_id: CanIdSource::Dynamic {
                assigned: Some(0x789),
            },
            ..one_odt(vec![]).remove(0)
        };
        let lists = vec![
            default_list,
            dynamic_unassigned,
            fixed_list,
            dynamic_assigned,
        ];

        let groups = group_by_can_id(&lists, 0x456);
        let ids: Vec<u32> = groups.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [0x456, 0x789], "ascending, deduplicated");

        let default_group = &groups[0].1;
        assert_eq!(
            default_group.iter().map(|l| l.id).collect::<Vec<_>>(),
            [0, 1],
            "an unassigned Dynamic list stays on the default id"
        );
        let fixed_group = &groups[1].1;
        assert_eq!(
            fixed_group.iter().map(|l| l.id).collect::<Vec<_>>(),
            [2, 3],
            "Fixed and an assigned Dynamic list land on the same id"
        );
    }

    #[test]
    fn signal_lookup_resolves_exact_and_chunk_parent_names() {
        let project = A2lProject {
            signals: vec![signal("Arr", 4)],
            ..Default::default()
        };
        let map = signal_lookup(&project);
        assert_eq!(
            resolve_signal(&map, "Arr").map(|s| s.name.as_str()),
            Some("Arr")
        );
        assert_eq!(
            resolve_signal(&map, "Arr[2]").map(|s| s.name.as_str()),
            Some("Arr"),
            "chunk names resolve to their parent"
        );
        assert!(resolve_signal(&map, "Ghost").is_none());
    }
}
