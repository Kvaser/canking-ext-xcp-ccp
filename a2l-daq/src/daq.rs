use crate::a2l::{
    A2lProject, CanIdSource, CcpData, CcpSource, DaqListDef, EventEntry, ParsedSignal, Protocol,
};
use std::collections::{BTreeMap, HashMap, HashSet};

// Re export the protocol-core types
pub use protocol_core::types::{DaqList, Odt, OdtEntry};

// Temporary struct used during packing
#[derive(Debug, Clone)]
struct Candidate {
    names: Vec<String>,
    ecu_address: u64,
    size: u16,
    signal_index: usize,
}

/// What the A2L declares about one DAQ list's place in the PID space.
#[derive(Debug, Clone, Copy, Default)]
struct PidDecl {
    first_pid: Option<u16>,
    reserved_odts: Option<u16>,
}

// -------------------------------- Public methods -------------------------------------------------------

/// Build DAQ list with a packing strategy.
///
/// Signals are grouped by their `event`. A `DaqList` can only have one event.
/// Each `DaqList` contains one or more `Odt`s. An `Odt` is filled by adding `OdtEntry`s until adding the next entry
/// would exceed `odt_max_bytes`, in which case a new `Odt` is started.
///
/// Returns `(daq_lists, unplaced)` where `unplaced` contains any signals that could not be assigned because no DAQ list slots remained.
pub fn build_daq_list(
    project: &A2lProject,
    protocol: Protocol,
) -> (Vec<DaqList>, Vec<ParsedSignal>) {
    const DEFAULT_ODT_MAX_BYTES: u16 = 7; // Classic CAN payload minus the PID byte

    if protocol == Protocol::Ccp {
        return build_daq_list_ccp(project.signals.clone(), project.ccp_data.as_ref());
    }

    let xcp = project.xcp_data.as_ref();
    let daq_info = xcp.map(|x| &x.daq_info);
    let events: &[EventEntry] = xcp
        .map(|x| x.event_info.events.as_slice())
        .unwrap_or_default();

    let mode = daq_info.map(|d| d.mode).unwrap_or_default();
    let available_daq = daq_info.map(|d| d.daq_lists.clone()).unwrap_or_default();
    let max_odt_entry_size = daq_info.map(|d| d.max_odt_entry_size).unwrap_or(0);
    let timestamp_reserved = daq_info.map(|d| d.timestamp_size).unwrap_or(0);

    // A DAQ DTO frame carries one PID byte plus the ODT payload.
    // MAX_DTO from PROTOCOL_LAYER takes precedence; fall back to CAN-FD DLC or the classic CAN default.
    let protocol_max_dto = xcp
        .and_then(|x| x.protocol_layer.as_ref())
        .filter(|p| p.max_dto > 0)
        .map(|p| p.max_dto);
    let can_fd_max_dlc = xcp
        .and_then(|x| x.xcp_info.as_ref())
        .and_then(|c| c.can_fd.as_ref())
        .map(|fd| fd.max_dlc);
    let odt_max_bytes = protocol_max_dto
        .map(|dto| dto.saturating_sub(1))
        .filter(|&b| b > 0)
        .or_else(|| {
            can_fd_max_dlc
                .map(|dlc| dlc.saturating_sub(1))
                .filter(|&b| b > 0)
        })
        .unwrap_or(DEFAULT_ODT_MAX_BYTES);

    // DAQ mode check
    let limits = PackLimits {
        odt_max_bytes,
        max_odt_entry_size,
        timestamp_reserved,
    };
    if mode.is_static() {
        build_daq_list_static(project.signals.clone(), limits, available_daq, events)
    } else {
        build_daq_list_with_packing(project.signals.clone(), limits, available_daq)
    }
}

/// CCP DAQ packing.
///
/// CCP has no master-side DAQ allocation and no CAN-FD: DAQ-DTO frames are classic CAN, so an ODT holds one PID byte plus 7 data bytes, and the ECU
/// defines each list's ODT count and PIDs. Each event (raster) therefore becomes exactly one DAQ list.
///
/// A `SOURCE`/`QP_BLOB` pair naming this event's raster supplies the list number, its reserved ODT count (`LENGTH`) and its `FIRST_PID`.
/// Rasters with no SOURCE fall back to the lowest unclaimed list number and an estimated PID, refined from `GET_DAQ_SIZE` at runtime.
fn build_daq_list_ccp(
    signals: Vec<ParsedSignal>,
    ccp: Option<&CcpData>,
) -> (Vec<DaqList>, Vec<ParsedSignal>) {
    const CCP_ODT_MAX_BYTES: u16 = 7;

    let rasters = ccp.map(|c| c.rasters.as_slice()).unwrap_or_default();
    let sources = ccp.map(|c| c.sources.as_slice()).unwrap_or_default();

    let mut lists: Vec<DaqList> = Vec::new();
    let mut unplaced: Vec<ParsedSignal> = Vec::new();
    let mut pid_decls: HashMap<u16, PidDecl> = HashMap::new();
    // Every declared list number is reserved up front so a raster without a SOURCE never lands on a number that belongs to another raster.
    let mut claimed: HashSet<u16> = sources.iter().map(|s| s.daq_list).collect();

    for (event, group_signals) in group_by_event(signals) {
        let (identifier, hint, can_id) = match ccp_source_for(sources, rasters, event.as_deref()) {
            Some(source) => (
                source.daq_list,
                PidDecl {
                    first_pid: source.first_pid,
                    reserved_odts: source.length,
                },
                source.can_id,
            ),
            None => (
                (0..=u16::MAX).find(|id| !claimed.contains(id)).unwrap_or(0),
                PidDecl::default(),
                None,
            ),
        };
        claimed.insert(identifier);
        pid_decls.insert(identifier, hint);

        // One DAQ list per raster, capped only by the ODTs the A2L reserves for it.
        let mut group_iter = std::iter::once(DaqListDef {
            identifier,
            nr_odt: hint.reserved_odts.unwrap_or(0),
            max_odt_entries: 0,
            event_fixed: None,
            first_pid: hint.first_pid,
            can_id: CanIdSource::from(can_id),
        });
        pack_event_group(
            event,
            group_signals,
            &mut group_iter,
            PackLimits {
                odt_max_bytes: CCP_ODT_MAX_BYTES,
                max_odt_entry_size: 0,
                timestamp_reserved: 0,
            },
            &mut lists,
            &mut unplaced,
        );
    }

    // The slave hands out PIDs in list order, so number them that way.
    lists.sort_by_key(|l| l.id);
    assign_first_pids(&mut lists, &pid_decls);
    (lists, unplaced)
}

/// Resolves an event name to its source. Falls back to the raster name if no SOURCE is declared for it, and returns None if neither exists.
fn ccp_source_for<'a>(
    sources: &'a [CcpSource],
    rasters: &[EventEntry],
    event: Option<&str>,
) -> Option<&'a CcpSource> {
    let event = event?;
    rasters
        .iter()
        .find(|r| r.name == event)
        .and_then(|r| sources.iter().find(|s| s.rasters.contains(&r.channel)))
        .or_else(|| sources.iter().find(|s| s.name == event))
}

/// Dynamic DAQ packing: signals are grouped by event and DAQ lists are handed out from a single shared pool in declaration order.
/// The master is free to associate any DAQ list with any event at runtime, so event-to-list binding does not matter here
fn build_daq_list_with_packing(
    signals: Vec<ParsedSignal>,
    limits: PackLimits,
    available_daq: Vec<DaqListDef>,
) -> (Vec<DaqList>, Vec<ParsedSignal>) {
    let mut lists: Vec<DaqList> = Vec::new();
    let mut unplaced: Vec<ParsedSignal> = Vec::new();

    let pid_decls: HashMap<u16, PidDecl> = available_daq
        .iter()
        .map(|d| {
            (
                d.identifier,
                PidDecl {
                    first_pid: d.first_pid,
                    reserved_odts: None,
                },
            )
        })
        .collect();
    let mut daq_iter = available_daq.into_iter();

    for (event, group_signals) in group_by_event(signals) {
        pack_event_group(
            event,
            group_signals,
            &mut daq_iter,
            limits,
            &mut lists,
            &mut unplaced,
        );
    }

    assign_first_pids(&mut lists, &pid_decls);
    (lists, unplaced)
}

/// Static DAQ packing: DAQ lists and ODTs are permanently defined in the ECU and each list is bound to one event channel via `EVENT_FIXED`.
/// Signals for a given event may therefore only be placed into the list(s) bound to that event.
/// Lists with no `EVENT_FIXED` are treated as freely assignable and used as a fallback.
fn build_daq_list_static(
    signals: Vec<ParsedSignal>,
    limits: PackLimits,
    available_daq: Vec<DaqListDef>,
    events: &[EventEntry],
) -> (Vec<DaqList>, Vec<ParsedSignal>) {
    let mut lists: Vec<DaqList> = Vec::new();
    let mut unplaced: Vec<ParsedSignal> = Vec::new();
    let mut used: HashSet<u16> = HashSet::new();

    let pid_decls: HashMap<u16, PidDecl> = available_daq
        .iter()
        .map(|d| {
            (
                d.identifier,
                PidDecl {
                    first_pid: d.first_pid,
                    reserved_odts: (d.nr_odt > 0).then_some(d.nr_odt),
                },
            )
        })
        .collect();

    for (event, group_signals) in group_by_event(signals) {
        // Resolve the event channel this group belongs to.
        let channel = event
            .as_deref()
            .and_then(|name| events.iter().find(|e| e.name == name))
            .map(|e| e.channel as u16);

        // Filter the available DAQ lists to those that are not already used and that either match the event channel or have no fixed channel.
        let mut eligible: Vec<DaqListDef> = available_daq
            .iter()
            .filter(|d| !used.contains(&d.identifier))
            .filter(|d| match channel {
                Some(ch) => d.event_fixed == Some(ch) || d.event_fixed.is_none(),
                None => d.event_fixed.is_none(),
            })
            .cloned()
            .collect();
        eligible.sort_by_key(|d| d.event_fixed != channel);

        let before = lists.len();
        let mut group_iter = eligible.into_iter();
        pack_event_group(
            event,
            group_signals,
            &mut group_iter,
            limits,
            &mut lists,
            &mut unplaced,
        );

        // Mark every list actually consumed by this group as used
        for list in &lists[before..] {
            used.insert(list.id);
        }
    }

    // Predefined lists carry predefined PIDs, so number them in list order.
    lists.sort_by_key(|l| l.id);
    assign_first_pids(&mut lists, &pid_decls);
    (lists, unplaced)
}

/// Group signals by their assigned event
fn group_by_event(signals: Vec<ParsedSignal>) -> BTreeMap<Option<String>, Vec<ParsedSignal>> {
    let mut groups: BTreeMap<Option<String>, Vec<ParsedSignal>> = BTreeMap::new();
    for s in signals {
        groups.entry(s.event.clone()).or_default().push(s);
    }
    groups
}

/// The size limits packing has to respect, the same for every event group in one run.
///
/// A zero `max_odt_entry_size` means the A2L declares no per-entry granularity, in which
/// case an entry may span a whole ODT.
#[derive(Debug, Clone, Copy)]
struct PackLimits {
    /// Payload bytes one ODT may hold: the transport frame limit minus the PID byte.
    odt_max_bytes: u16,
    /// Largest single ODT entry the slave accepts, or 0 for no declared limit.
    max_odt_entry_size: u16,
    /// Bytes the ECU prepends to the first ODT of each list, reserved out of its budget.
    timestamp_reserved: u16,
}

impl PackLimits {
    /// Largest chunk one signal may be split into.
    fn split_max(&self) -> u16 {
        if self.max_odt_entry_size > 0 {
            self.max_odt_entry_size
        } else {
            self.odt_max_bytes
        }
    }

    /// Largest entry two contiguous candidates may merge into.
    fn merge_max(&self) -> u16 {
        if self.max_odt_entry_size > 0 {
            self.odt_max_bytes.min(self.max_odt_entry_size)
        } else {
            self.odt_max_bytes
        }
    }
}

fn pack_event_group(
    event: Option<String>,
    mut group_signals: Vec<ParsedSignal>,
    daq_iter: &mut impl Iterator<Item = DaqListDef>,
    limits: PackLimits,
    out: &mut Vec<DaqList>,
    unplaced: &mut Vec<ParsedSignal>,
) {
    group_signals.sort_by_key(|s| std::cmp::Reverse(s.size));

    while !group_signals.is_empty() {
        let daq_def = match daq_iter.next() {
            Some(d) => d,
            None => {
                unplaced.extend(group_signals);
                break;
            }
        };

        let max_odts = if daq_def.nr_odt > 0 {
            daq_def.nr_odt as usize
        } else {
            usize::MAX
        };
        let max_entries_per_odt = if daq_def.max_odt_entries > 0 {
            daq_def.max_odt_entries as usize
        } else {
            0
        };

        // Split oversized signals into chunks
        let candidates: Vec<Candidate> = group_signals
            .iter()
            .enumerate()
            .flat_map(|(i, s)| split_signal(s, i, limits.split_max()))
            .collect();

        // Merge memory-contiguous candidates into single entries
        let candidates = merge_contiguous(candidates, limits.merge_max());

        // First Fit Decreasing into ODTs, respecting both byte budget and entry count
        let (odts, failed_indices) = pack_ffd(
            candidates,
            limits.odt_max_bytes,
            max_odts,
            max_entries_per_odt,
            limits.timestamp_reserved,
        );

        out.push(DaqList {
            id: daq_def.identifier,
            first_pid: 0,
            odts,
            event: event.clone(),
            can_id: daq_def.can_id,
        });

        // Signals with any unplaceable chunk are retried on the next DAQ list
        group_signals = group_signals
            .into_iter()
            .enumerate()
            .filter(|(i, _)| failed_indices.contains(i))
            .map(|(_, s)| s)
            .collect();
    }
}

/// Assign each DAQ list its starting PID.
///
/// A list whose A2L declares `FIRST_PID` keeps that value. The rest continue from where the previous list's PID range ends,
/// stepping over every ODT the ECU reserves for it or, when the A2L reserves none, over just the ODTs this configuration filled.
fn assign_first_pids(lists: &mut [DaqList], pid_decls: &HashMap<u16, PidDecl>) {
    let mut next_pid: u16 = 0;
    for list in lists.iter_mut() {
        let hint = pid_decls.get(&list.id).copied().unwrap_or_default();
        let filled = list.odts.len() as u16;
        list.first_pid = hint.first_pid.unwrap_or(next_pid);
        let stride = hint.reserved_odts.unwrap_or(filled).max(filled);
        next_pid = list.first_pid.saturating_add(stride);
    }
}

// ------------------------------- Packing helpers ------------------------------------------

/// Split one signal into one or more `Candidate` entries.
fn split_signal(signal: &ParsedSignal, signal_index: usize, max_size: u16) -> Vec<Candidate> {
    if signal.size == 0 {
        return vec![];
    }
    if signal.size <= max_size {
        return vec![Candidate {
            names: vec![signal.name.clone()],
            ecu_address: signal.ecu_address,
            size: signal.size,
            signal_index,
        }];
    }
    let mut chunks = Vec::new();
    let mut remaining = signal.size;
    let mut offset = 0u64;
    let mut part = 0usize;
    while remaining > 0 {
        let chunk = remaining.min(max_size);
        chunks.push(Candidate {
            names: vec![format!("{}[{}]", signal.name, part)],
            ecu_address: signal.ecu_address + offset,
            size: chunk,
            signal_index,
        });
        offset += chunk as u64;
        remaining -= chunk;
        part += 1;
    }
    chunks
}

/// Merge candidates that are contiguous in ECU memory into a single entry,
fn merge_contiguous(mut candidates: Vec<Candidate>, max_size: u16) -> Vec<Candidate> {
    if candidates.is_empty() {
        return candidates;
    }
    // Sort by address
    candidates.sort_by_key(|c| c.ecu_address);
    let mut merged: Vec<Candidate> = Vec::new();
    let mut current = candidates.remove(0);
    for next in candidates {
        let is_adjacent = current.ecu_address + current.size as u64 == next.ecu_address;
        let fits = current.size + next.size <= max_size;
        if is_adjacent && fits {
            current.size += next.size;
            current.names.extend(next.names);
        } else {
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);
    merged
}

/// First Fit Decreasing bin-packing of candidates into ODTs.
///
/// Enforces two independent constraints per ODT:
/// - `odt_max_bytes`: total payload bytes the ODT may hold, i.e. the transport frame
///   limit minus the PID byte (7 on classic CAN, up to 63 on CAN FD)
///
/// - `max_entries_per_odt`: number of entry slots (0 = unlimited)
///
/// Returns:
/// - The packed `Odt` list.
///
/// - The set of original signal indices that had at least one chunk that could not be placed.
fn pack_ffd(
    candidates: Vec<Candidate>,
    odt_max_bytes: u16,
    max_odts: usize,
    max_entries_per_odt: usize,
    timestamp_reserved: u16,
) -> (Vec<Odt>, HashSet<usize>) {
    let mut sorted = candidates;
    sorted.sort_by_key(|c| std::cmp::Reverse(c.size));
    let mut bins: Vec<(Vec<OdtEntry>, u16)> = Vec::new();
    let mut failed_signal_indices: HashSet<usize> = HashSet::new();

    if timestamp_reserved > 0 && !sorted.is_empty() && max_odts > 0 {
        bins.push((Vec::new(), odt_max_bytes.saturating_sub(timestamp_reserved)));
    }

    for candidate in sorted {
        if candidate.size > odt_max_bytes {
            failed_signal_indices.insert(candidate.signal_index);
            continue;
        }

        let placed = bins.iter_mut().find(|(bin_entries, cap)| {
            *cap >= candidate.size
                && (max_entries_per_odt == 0 || bin_entries.len() < max_entries_per_odt)
        });

        if let Some((bin_entries, cap)) = placed {
            *cap -= candidate.size;
            bin_entries.push(OdtEntry {
                names: candidate.names,
                ecu_address: candidate.ecu_address,
                size: candidate.size,
            });
        } else if bins.len() < max_odts {
            let cap = odt_max_bytes - candidate.size;
            bins.push((
                vec![OdtEntry {
                    names: candidate.names,
                    ecu_address: candidate.ecu_address,
                    size: candidate.size,
                }],
                cap,
            ));
        } else {
            failed_signal_indices.insert(candidate.signal_index);
        }
    }

    // An empty ODT 0 is kept on purpose to keep space for the timestamp, dropped if no timestamp is reserved.
    let odts = bins
        .into_iter()
        .map(|(entries, _)| Odt { entries })
        .collect();
    (odts, failed_signal_indices)
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::{
        build_daq_list, build_daq_list_ccp, build_daq_list_static, build_daq_list_with_packing,
        PackLimits,
    };
    use crate::{
        a2l::{CcpData, CcpSource, DaqInfo, EventEntry, EventInfo, ParsedSignal, XcpData},
        A2lProject, DaqListDef, DaqMode, Protocol,
    };

    fn event(channel: u32, name: &str) -> EventEntry {
        EventEntry {
            channel,
            name: name.into(),
            ..Default::default()
        }
    }

    fn fixed_daq(id: u16, channel: u16) -> DaqListDef {
        DaqListDef {
            identifier: id,
            event_fixed: Some(channel),
            ..Default::default()
        }
    }

    fn xcp_with_daq_lists(defs: Vec<DaqListDef>) -> Option<XcpData> {
        xcp_with_daq_lists_and_entry_size(defs, 0)
    }

    fn xcp_with_daq_lists_and_entry_size(
        defs: Vec<DaqListDef>,
        max_odt_entry_size: u16,
    ) -> Option<XcpData> {
        Some(XcpData {
            daq_info: DaqInfo {
                mode: DaqMode::Dynamic,
                daq_lists: defs,
                max_odt_entry_size,
                ..Default::default()
            },
            event_info: EventInfo::default(),
            xcp_info: None,
            protocol_layer: None,
        })
    }

    fn limits(odt_max_bytes: u16, max_odt_entry_size: u16, timestamp_reserved: u16) -> PackLimits {
        PackLimits {
            odt_max_bytes,
            max_odt_entry_size,
            timestamp_reserved,
        }
    }

    fn unlimited_daq(id: u16) -> DaqListDef {
        DaqListDef {
            identifier: id,
            ..Default::default()
        }
    }

    #[test]
    fn builds_daq_list_with_single_odt() {
        let (list, _) = build_daq_list(
            &A2lProject {
                signals: vec![ParsedSignal {
                    name: String::from("Status"),
                    long_identifier: String::from("Engine Speed"),
                    ecu_address: 0x1000,
                    size: 4,
                    bit_mask: Some(0x01),
                    event: Some(String::from("FastTask")),
                    ..Default::default()
                }],
                xcp_data: xcp_with_daq_lists(vec![unlimited_daq(1)]),
                ..Default::default()
            },
            Protocol::Xcp,
        );

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, 1);
        let entry = &list[0].odts[0].entries[0];
        assert_eq!(entry.names, vec!["Status"]);
        assert_eq!(entry.name(), "Status");
        assert_eq!(entry.ecu_address, 0x1000);
        assert_eq!(entry.size, 4);
        assert_eq!(list[0].event.as_deref(), Some("FastTask"));
    }

    #[test]
    fn packs_multiple_entries_into_same_odt_when_space() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 2,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x1004,
                size: 4,
                event: Some("E".into()),
                ..Default::default()
            },
        ];
        let (list, _) = build_daq_list(
            &A2lProject {
                signals,
                xcp_data: xcp_with_daq_lists(vec![unlimited_daq(5)]),
                ..Default::default()
            },
            Protocol::Xcp,
        );
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, 5);
        assert_eq!(list[0].odts.len(), 1);
        assert_eq!(list[0].odts[0].entries.len(), 2);
    }

    #[test]
    fn splits_entries_into_new_odt_when_overflow() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 3,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x1004,
                size: 5,
                event: Some("E".into()),
                ..Default::default()
            },
        ];
        let (list, _) = build_daq_list(
            &A2lProject {
                signals,
                xcp_data: xcp_with_daq_lists(vec![unlimited_daq(2)]),
                ..Default::default()
            },
            Protocol::Xcp,
        );
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].odts.len(), 2);
    }

    #[test]
    fn skips_event_groups_when_no_daq_slots_remain() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 2,
                event: Some("E1".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x2000,
                size: 2,
                event: Some("E2".into()),
                ..Default::default()
            },
        ];
        let (list, unplaced) =
            build_daq_list_with_packing(signals, limits(7, 0, 0), vec![unlimited_daq(10)]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, 10);
        assert_eq!(unplaced.len(), 1, "signal B should be unplaced");
        assert_eq!(unplaced[0].name, "B");
    }

    #[test]
    fn respects_max_odt_limit_per_daq_list() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 4,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x1004,
                size: 4,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "C".into(),
                ecu_address: 0x1008,
                size: 4,
                event: Some("E".into()),
                ..Default::default()
            },
        ];
        let defs = vec![
            DaqListDef {
                identifier: 3,
                nr_odt: 1,
                ..Default::default()
            },
            DaqListDef {
                identifier: 4,
                ..Default::default()
            },
        ];
        let (list, _) = build_daq_list_with_packing(signals, limits(4, 0, 0), defs);
        assert_eq!(list[0].odts.len(), 1);
        assert_eq!(list[1].odts.len(), 2);
    }

    #[test]
    fn respects_max_odt_entries_entry_count_per_odt() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 1,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x1010,
                size: 1,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "C".into(),
                ecu_address: 0x1020,
                size: 1,
                event: Some("E".into()),
                ..Default::default()
            },
        ];
        let daq = DaqListDef {
            identifier: 9,
            max_odt_entries: 2,
            ..Default::default()
        };
        let (list, _) = build_daq_list_with_packing(signals, limits(7, 0, 0), vec![daq]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].odts.len(), 2);
    }

    #[test]
    fn splits_large_signal_into_chunks() {
        let signals = vec![ParsedSignal {
            name: "Big".into(),
            ecu_address: 0x2000,
            size: 10,
            event: Some("E".into()),
            ..Default::default()
        }];
        let (list, _) =
            build_daq_list_with_packing(signals, limits(7, 0, 0), vec![unlimited_daq(1)]);
        assert_eq!(list.len(), 1);
        let all_entries: Vec<_> = list[0].odts.iter().flat_map(|o| &o.entries).collect();
        assert_eq!(all_entries.len(), 2);
        assert_eq!(all_entries[0].names, vec!["Big[0]"]);
        assert_eq!(all_entries[0].size, 7);
        assert_eq!(all_entries[0].ecu_address, 0x2000);
        assert_eq!(all_entries[1].names, vec!["Big[1]"]);
        assert_eq!(all_entries[1].size, 3);
        assert_eq!(all_entries[1].ecu_address, 0x2007);
    }

    #[test]
    fn large_signal_exact_multiple_of_odt_size() {
        let signals = vec![ParsedSignal {
            name: "X".into(),
            ecu_address: 0x3000,
            size: 14,
            event: Some("E".into()),
            ..Default::default()
        }];
        let (list, _) =
            build_daq_list_with_packing(signals, limits(7, 0, 0), vec![unlimited_daq(1)]);
        let all_entries: Vec<_> = list[0].odts.iter().flat_map(|o| &o.entries).collect();
        assert_eq!(all_entries.len(), 2);
        assert_eq!(all_entries[0].size, 7);
        assert_eq!(all_entries[1].size, 7);
        assert_eq!(all_entries[1].ecu_address, 0x3007);
    }

    #[test]
    fn merges_adjacent_signals_into_single_entry() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 2,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x1002,
                size: 3,
                event: Some("E".into()),
                ..Default::default()
            },
        ];
        let (list, _) =
            build_daq_list_with_packing(signals, limits(7, 0, 0), vec![unlimited_daq(1)]);
        let entries = &list[0].odts[0].entries;
        assert_eq!(
            entries.len(),
            1,
            "adjacent signals should merge into one entry"
        );
        assert_eq!(entries[0].size, 5);
        assert_eq!(entries[0].ecu_address, 0x1000);
        assert!(entries[0].names.contains(&"A".to_string()));
        assert!(entries[0].names.contains(&"B".to_string()));
    }

    #[test]
    fn does_not_merge_non_adjacent_signals() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 2,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x1010,
                size: 2,
                event: Some("E".into()),
                ..Default::default()
            },
        ];
        let (list, _) =
            build_daq_list_with_packing(signals, limits(7, 0, 0), vec![unlimited_daq(1)]);
        let entries = &list[0].odts[0].entries;
        assert_eq!(entries.len(), 2, "non-adjacent signals must stay separate");
    }

    #[test]
    fn does_not_merge_when_combined_size_exceeds_odt_max() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 4,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x1004,
                size: 4,
                event: Some("E".into()),
                ..Default::default()
            },
        ];
        let (list, _) =
            build_daq_list_with_packing(signals, limits(7, 0, 0), vec![unlimited_daq(1)]);
        let all_entries: Vec<_> = list[0].odts.iter().flat_map(|o| &o.entries).collect();
        assert_eq!(all_entries.len(), 2);
    }

    #[test]
    fn ffd_fills_gaps_in_existing_odts() {
        // Signals:         A(5), B(5), C(2) with odt_max=7.
        // Sequential fill: A->ODT0(5), B->ODT1(5), C->ODT2(2).
        // FFD:             A(5)->ODT0, B(5)->ODT1, C(2)->ODT0 -> 2 ODTs.
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 5,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x2000,
                size: 5,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "C".into(),
                ecu_address: 0x3000,
                size: 2,
                event: Some("E".into()),
                ..Default::default()
            },
        ];
        let (list, _) =
            build_daq_list_with_packing(signals, limits(7, 0, 0), vec![unlimited_daq(1)]);
        assert_eq!(
            list[0].odts.len(),
            2,
            "FFD should pack C into the gap left by A"
        );
    }

    #[test]
    fn splits_signal_at_max_odt_entry_size_not_odt_frame_size() {
        let signals = vec![ParsedSignal {
            name: "Val".into(),
            ecu_address: 0x5000,
            size: 5,
            event: Some("E".into()),
            ..Default::default()
        }];
        let (list, _) =
            build_daq_list_with_packing(signals, limits(7, 4, 0), vec![unlimited_daq(1)]);
        assert_eq!(
            list[0].odts.len(),
            1,
            "both chunks fit byte-wise in one ODT"
        );
        let all_entries: Vec<_> = list[0].odts.iter().flat_map(|o| &o.entries).collect();
        assert_eq!(
            all_entries.len(),
            2,
            "chunks stay separate (merge cap = max_odt_entry_size)"
        );
        assert_eq!(all_entries[0].size, 4);
        assert_eq!(all_entries[1].size, 1);
        assert_eq!(all_entries[1].ecu_address, 0x5004);
    }

    #[test]
    fn can_fd_widens_odt_budget_into_one_odt() {
        use crate::a2l::{CanFdInfo, XcpInfo};
        // Four non-adjacent 4-byte signals. On classic CAN (7-byte ODT) only one
        // fits per ODT (4+4 = 8 > 7) -> 4 ODTs. On CAN FD (MAX_DLC 24 -> 23-byte
        // ODT) all four share a single ODT.
        let signals: Vec<ParsedSignal> = [0x1000u64, 0x1100, 0x1200, 0x1300]
            .iter()
            .enumerate()
            .map(|(i, &addr)| ParsedSignal {
                name: format!("S{i}"),
                ecu_address: addr,
                size: 4,
                event: Some("E".into()),
                ..Default::default()
            })
            .collect();

        let xcp = Some(XcpData {
            daq_info: DaqInfo {
                mode: DaqMode::Dynamic,
                daq_lists: vec![unlimited_daq(0)],
                max_odt_entry_size: 0,
                ..Default::default()
            },
            event_info: EventInfo::default(),
            xcp_info: Some(XcpInfo {
                can_fd: Some(CanFdInfo {
                    max_dlc: 24,
                    data_baudrate: Some(2_000_000),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            protocol_layer: None,
        });

        let (list, _) = build_daq_list(
            &A2lProject {
                signals,
                xcp_data: xcp,
                ..Default::default()
            },
            Protocol::Xcp,
        );
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].odts.len(),
            1,
            "all four entries fit one 23-byte CAN FD ODT"
        );
        let entries: Vec<_> = list[0].odts.iter().flat_map(|o| &o.entries).collect();
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn static_routes_signals_to_their_event_fixed_list() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 4,
                event: Some("Rev".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x2000,
                size: 4,
                event: Some("5ms".into()),
                ..Default::default()
            },
        ];
        let events = vec![event(0, "Rev"), event(1, "5ms")];
        let defs = vec![fixed_daq(0, 0), fixed_daq(1, 1)];

        let (lists, _) = build_daq_list_static(signals, limits(7, 0, 0), defs, &events);

        let rev = lists
            .iter()
            .find(|l| l.id == 0)
            .expect("list fixed to event 0");
        let ms = lists
            .iter()
            .find(|l| l.id == 1)
            .expect("list fixed to event 1");
        assert_eq!(rev.event.as_deref(), Some("Rev"));
        assert_eq!(rev.odts[0].entries[0].names, vec!["A"]);
        assert_eq!(ms.event.as_deref(), Some("5ms"));
        assert_eq!(ms.odts[0].entries[0].names, vec!["B"]);
    }

    #[test]
    fn static_overflow_does_not_borrow_another_events_list() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 7,
                event: Some("Rev".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x2000,
                size: 7,
                event: Some("Rev".into()),
                ..Default::default()
            },
        ];
        let events = vec![event(0, "Rev"), event(1, "5ms")];
        let defs = vec![
            DaqListDef {
                identifier: 0,
                nr_odt: 1,
                event_fixed: Some(0),
                ..Default::default()
            },
            fixed_daq(1, 1),
        ];

        let (lists, unplaced) = build_daq_list_static(signals, limits(7, 0, 0), defs, &events);

        assert_eq!(lists.len(), 1, "only the event-0 list is used");
        assert_eq!(unplaced.len(), 1, "signal B should be unplaced");
        assert_eq!(lists[0].id, 0);
        let entries: Vec<_> = lists[0].odts.iter().flat_map(|o| &o.entries).collect();
        assert_eq!(
            entries.len(),
            1,
            "only one 7-byte signal fits in the single ODT"
        );
    }

    #[test]
    fn static_packs_multiple_entries_per_odt() {
        let signals: Vec<ParsedSignal> = [0x1000u64, 0x1100, 0x1200]
            .iter()
            .enumerate()
            .map(|(i, &addr)| ParsedSignal {
                name: format!("S{i}"),
                ecu_address: addr,
                size: 4,
                event: Some("Rev".into()),
                ..Default::default()
            })
            .collect();
        let events = vec![event(0, "Rev")];
        let defs = vec![fixed_daq(0, 0)];

        let (lists, unplaced) = build_daq_list_static(signals, limits(23, 0, 0), defs, &events);

        assert!(unplaced.is_empty());
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].odts.len(), 1, "all three entries share one ODT");
        assert_eq!(lists[0].odts[0].entries.len(), 3);
    }

    #[test]
    fn static_uses_freely_assignable_list_as_fallback() {
        let signals = vec![ParsedSignal {
            name: "A".into(),
            ecu_address: 0x1000,
            size: 4,
            event: Some("Rev".into()),
            ..Default::default()
        }];
        let events = vec![event(0, "Rev")];
        let defs = vec![unlimited_daq(7)]; // event_fixed: None

        let (lists, _) = build_daq_list_static(signals, limits(7, 0, 0), defs, &events);

        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].id, 7);
        assert_eq!(lists[0].odts[0].entries[0].names, vec!["A"]);
    }

    #[test]
    fn fixed_timestamp_shrinks_first_odt_so_frame_never_overflows() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 2,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x1002,
                size: 2,
                event: Some("E".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "C".into(),
                ecu_address: 0x1004,
                size: 2,
                event: Some("E".into()),
                ..Default::default()
            },
        ];
        let (list, unplaced) =
            build_daq_list_with_packing(signals, limits(7, 0, 4), vec![unlimited_daq(0)]);
        assert!(unplaced.is_empty());
        for (odt_idx, odt) in list[0].odts.iter().enumerate() {
            let data: u16 = odt.entries.iter().map(|e| e.size).sum();
            let ts = if odt_idx == 0 { 4 } else { 0 };
            let frame = data + ts + 1; //PID
            assert!(
                frame <= 8,
                "ODT {odt_idx}: {data} data + {ts} ts + 1 PID = {frame} exceeds classic CAN frame"
            );
        }
    }

    #[test]
    fn fixed_timestamp_keeps_small_entries_in_first_odt() {
        let signals = vec![ParsedSignal {
            name: "A".into(),
            ecu_address: 0x1000,
            size: 1,
            event: Some("E".into()),
            ..Default::default()
        }];
        let (list, _) =
            build_daq_list_with_packing(signals, limits(7, 0, 4), vec![unlimited_daq(0)]);
        assert_eq!(list[0].odts.len(), 1, "single small entry fits ODT 0");
        assert_eq!(list[0].odts[0].entries[0].names, vec!["A"]);
    }

    #[test]
    fn build_daq_list_uses_max_odt_entry_size_from_a2l() {
        let signals = vec![ParsedSignal {
            name: "D".into(),
            ecu_address: 0x1000,
            size: 8,
            event: Some("E".into()),
            ..Default::default()
        }];
        let daq = DaqListDef {
            identifier: 0,
            nr_odt: 5,
            max_odt_entries: 7,
            ..Default::default()
        };
        let (list, _) = build_daq_list_with_packing(signals, limits(7, 4, 0), vec![daq]);
        let all_entries: Vec<_> = list[0].odts.iter().flat_map(|o| &o.entries).collect();
        assert_eq!(all_entries.len(), 2);
        assert_eq!(all_entries[0].size, 4);
        assert_eq!(all_entries[1].size, 4);
        assert_eq!(list[0].odts.len(), 2);
    }

    #[test]
    fn ccp_packs_one_list_per_raster_with_sequential_ids() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 4,
                event: Some("DAQ_CHAN1".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x2000,
                size: 4,
                event: Some("DAQ_CHAN2".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "C".into(),
                ecu_address: 0x2100,
                size: 4,
                event: Some("DAQ_CHAN2".into()),
                ..Default::default()
            },
        ];
        let (lists, unplaced) = build_daq_list_ccp(signals, None);
        assert!(unplaced.is_empty());
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].id, 0);
        assert_eq!(lists[1].id, 1);
        assert_eq!(lists[0].first_pid, 0);
        assert_eq!(lists[1].first_pid, lists[0].odts.len() as u16);
        assert!(lists
            .iter()
            .any(|l| l.event.as_deref() == Some("DAQ_CHAN1")));
    }

    #[test]
    fn ccp_takes_list_numbers_and_pids_from_source_blocks() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 4,
                event: Some("Fast".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x2000,
                size: 4,
                event: Some("Slow".into()),
                ..Default::default()
            },
        ];

        let ccp = CcpData {
            rasters: vec![event(4, "Slow"), event(7, "Fast")],
            sources: vec![
                CcpSource {
                    name: "Slow".into(),
                    daq_list: 1,
                    rasters: vec![4],
                    first_pid: Some(0x20),
                    ..Default::default()
                },
                CcpSource {
                    name: "Fast".into(),
                    daq_list: 0,
                    rasters: vec![7],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let (lists, unplaced) = build_daq_list_ccp(signals, Some(&ccp));
        assert!(unplaced.is_empty());
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].id, 0);
        assert_eq!(lists[0].event.as_deref(), Some("Fast"));
        assert_eq!(lists[0].first_pid, 0, "no FIRST_PID: starts the PID space");
        assert_eq!(lists[1].id, 1);
        assert_eq!(lists[1].event.as_deref(), Some("Slow"));
        assert_eq!(lists[1].first_pid, 0x20, "FIRST_PID wins over the estimate");
    }

    #[test]
    fn ccp_steps_over_every_odt_a_source_reserves() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 4,
                event: Some("Fast".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x2000,
                size: 4,
                event: Some("Slow".into()),
                ..Default::default()
            },
        ];
        let ccp = CcpData {
            rasters: vec![event(0, "Fast"), event(1, "Slow")],
            sources: vec![
                CcpSource {
                    name: "Fast".into(),
                    daq_list: 0,
                    rasters: vec![0],
                    length: Some(18),
                    ..Default::default()
                },
                CcpSource {
                    name: "Slow".into(),
                    daq_list: 1,
                    rasters: vec![1],
                    length: Some(18),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let (lists, _) = build_daq_list_ccp(signals, Some(&ccp));
        assert_eq!(lists[0].odts.len(), 1, "one 4-byte signal, one ODT filled");
        assert_eq!(lists[0].first_pid, 0);
        assert_eq!(
            lists[1].first_pid, 18,
            "list 1 starts past list 0's 18 reserved ODTs, not past the 1 we filled"
        );
    }

    #[test]
    fn ccp_falls_back_to_list_numbers_no_source_claims() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 4,
                event: Some("Declared".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x2000,
                size: 4,
                event: Some("Undeclared".into()),
                ..Default::default()
            },
        ];
        let ccp = CcpData {
            rasters: vec![event(0, "Declared"), event(1, "Undeclared")],
            sources: vec![CcpSource {
                name: "Declared".into(),
                daq_list: 0,
                rasters: vec![0],
                ..Default::default()
            }],
            ..Default::default()
        };

        let (lists, _) = build_daq_list_ccp(signals, Some(&ccp));
        let declared = lists.iter().find(|l| l.id == 0).expect("declared list 0");
        assert_eq!(declared.event.as_deref(), Some("Declared"));
        let fallback = lists
            .iter()
            .find(|l| l.event.as_deref() == Some("Undeclared"))
            .expect("undeclared raster still gets a list");
        assert_eq!(fallback.id, 1, "list 0 belongs to the declared raster");
    }

    #[test]
    fn ccp_matches_a_source_by_name_when_it_declares_no_raster() {
        let signals = vec![ParsedSignal {
            name: "A".into(),
            ecu_address: 0x1000,
            size: 2,
            event: Some("CCP_10ms List".into()),
            ..Default::default()
        }];
        let ccp = CcpData {
            rasters: vec![event(0, "CCP_10ms List")],
            sources: vec![CcpSource {
                name: "CCP_10ms List".into(),
                daq_list: 3,
                first_pid: Some(9),
                ..Default::default()
            }],
            ..Default::default()
        };

        let (lists, _) = build_daq_list_ccp(signals, Some(&ccp));
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].id, 3);
        assert_eq!(lists[0].first_pid, 9);
    }

    #[test]
    fn ccp_splits_into_seven_byte_odts() {
        let signals = vec![
            ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 4,
                event: Some("R".into()),
                ..Default::default()
            },
            ParsedSignal {
                name: "B".into(),
                ecu_address: 0x2000,
                size: 4,
                event: Some("R".into()),
                ..Default::default()
            },
        ];
        let (lists, _) = build_daq_list_ccp(signals, None);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].odts.len(), 2);
    }

    #[test]
    fn build_daq_list_dispatches_to_ccp_path() {
        let project = A2lProject {
            signals: vec![ParsedSignal {
                name: "A".into(),
                ecu_address: 0x1000,
                size: 2,
                event: Some("R".into()),
                ..Default::default()
            }],
            ccp_data: Some(CcpData {
                rasters: vec![event(0, "R")],
                ..Default::default()
            }),
            ..Default::default()
        };
        let (lists, _) = build_daq_list(&project, Protocol::Ccp);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].odts[0].entries[0].names, vec!["A"]);
    }
}
