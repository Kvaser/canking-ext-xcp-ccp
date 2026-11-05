//! Transport-agnostic DAQ command generation.
//!
//! [`build_init_commands`] emits the DAQ initialisation sequence in five
//! phases. Every command must be answered by a positive response, a `0xFE` first byte is an error packet and the sequence must stop.
//! Commands whose response carries data the pipeline consumes are marked via [`XcpCommand::response`].
//! [`build_stop_commands`] emits the shutdown sequence for a running DAQ session.
//!
//! | Phase | Commands | Expected response |
//! |-------|----------|-------------------|
//! | `Connect` | CONNECT, GET_STATUS, GET_SEED + UNLOCK (optional, resource=DAQ) | ack |
//! | `Prepare` | START_STOP_SYNCH (stop all), then static: CLEAR_DAQ_LIST per list / dynamic: FREE_DAQ, ALLOC_DAQ, ALLOC_ODT xN, ALLOC_ODT_ENTRY xN, then SET_DAQ_ID per list with a user-assigned dynamic CAN id | ack |
//! | `WriteEntries` | per ODT: SET_DAQ_PTR (entry 0), WRITE_DAQ x entries | ack |
//! | `SetMode` | SET_DAQ_LIST_MODE per list | ack |
//! | `Start` | START_STOP_DAQ_LIST (select) per list, START_STOP_SYNCH (start) | select: byte 1 = FIRST_PID |
//! -------------------------------------------
//! | 'Stop' | START_STOP_SYNCH (stop all), then static: CLEAR_DAQ_LIST per list / dynamic: FREE_DAQ, DISCONNECT | ack |
//!
//! Static and dynamic DAQ share every phase except `Prepare`. Two protocol rules shape the sequence:
//!
//! - WRITE_DAQ auto-increments the slave's ODT-entry pointer, so one SET_DAQ_PTR per ODT (at entry 0) is enough.
//!   Some reject SET_DAQ_PTR with a non-zero entry number.
//! - ALLOC_DAQ numbers dynamic lists `0..count-1` regardless of the A2L `DAQ_LIST` identifiers, so in dynamic mode every command addresses lists
//!   by their allocation index. Static lists keep their A2L identifier.
//! - SET_DAQ_ID (transport-layer command `0xF2`, sub-command `0xFD`) is only sent for a list whose `CanIdSource::Dynamic` has a
//!   user-assigned id; everything else (no override, a `FIXED` A2L id, or a `Dynamic` list left on its fallback) keeps transmitting
//!   on the transport's shared default id, unchanged.
//! - GET_SEED/UNLOCK are marked optional: many ecu:s don't implement them at all, and a development ECU with no real security algorithm typically accepts any key,
//!  so UNLOCK is sent with an all-zero key. A real seed-dependent key algorithm would replace the key bytes built

use crate::types::{A2lProject, ByteOrder, DaqList, DaqListDef, EventEntry};

// The command/phase/response types are shared across protocols and live in
// [`crate::protocol`]; the XCP-prefixed names are kept as aliases.
use crate::protocol::{dword, word};
pub use crate::protocol::{
    Command as XcpCommand, ExpectedResponse as XcpResponse, Phase as XcpPhase,
};

/// Snapshot of everything the phase builders need from the parsed project.
struct Ctx<'a> {
    is_static: bool,
    min_daq: u16,
    big_endian: bool,
    timestamp_size: u16,
    events: &'a [EventEntry],
    daq_defs: &'a [DaqListDef],
    frame_len: Option<usize>,
}

impl Ctx<'_> {
    fn wire_id(&self, index: usize, daq: &DaqList) -> u16 {
        if self.is_static {
            daq.id
        } else {
            self.min_daq + index as u16
        }
    }
}

/// Build the full DAQ initialisation command sequence from a parsed project and packed DAQ lists.
///
/// Uses the slave's byte order
pub fn build_init_commands(project: &A2lProject, daq_lists: &[DaqList]) -> Vec<XcpCommand> {
    let ctx = build_ctx(project);

    let mut cmds: Vec<XcpCommand> = Vec::new();
    phase_connect(&mut cmds);
    phase_prepare(&mut cmds, &ctx, daq_lists);
    phase_set_daq_id(&mut cmds, &ctx, daq_lists);
    phase_write_entries(&mut cmds, &ctx, daq_lists);
    phase_set_mode(&mut cmds, &ctx, daq_lists);
    phase_start(&mut cmds, &ctx, daq_lists);
    pad(&mut cmds, ctx.frame_len);
    cmds
}

/// Build the shutdown sequence for a running DAQ session: stop all lists, release them, then DISCONNECT.
pub fn build_stop_commands(project: &A2lProject, daq_lists: &[DaqList]) -> Vec<XcpCommand> {
    let ctx = build_ctx(project);
    let mut cmds: Vec<XcpCommand> = Vec::new();

    push(
        &mut cmds,
        XcpPhase::Stop,
        vec![0xDD, 0x00],
        "START_STOP_SYNCH  stop all".into(),
    );

    if ctx.is_static {
        for (idx, daq) in daq_lists.iter().enumerate() {
            let wire = ctx.wire_id(idx, daq);
            let id = word(wire, ctx.big_endian);
            push(
                &mut cmds,
                XcpPhase::Stop,
                vec![0xE3, 0x00, id[0], id[1]],
                format!("CLEAR_DAQ_LIST  daq={}", wire),
            );
        }
    } else {
        push(&mut cmds, XcpPhase::Stop, vec![0xD6], "FREE_DAQ".into());
    }

    push(&mut cmds, XcpPhase::Stop, vec![0xFE], "DISCONNECT".into());
    pad(&mut cmds, ctx.frame_len);
    cmds
}

/// Zero-pad every command to `len` bytes, for slaves that only accept full-length frames.
fn pad(cmds: &mut [XcpCommand], len: Option<usize>) {
    let Some(len) = len else { return };
    for c in cmds.iter_mut().filter(|c| c.bytes.len() < len) {
        c.bytes.resize(len, 0);
    }
}

/// Snapshot the parsed project's XCP settings
fn build_ctx(project: &A2lProject) -> Ctx<'_> {
    let xcp = project.xcp_data.as_ref();
    Ctx {
        is_static: xcp.map(|x| x.daq_info.mode.is_static()).unwrap_or(false),
        min_daq: xcp.map(|x| x.daq_info.min_daq).unwrap_or(0),
        big_endian: xcp
            .and_then(|x| x.protocol_layer.as_ref())
            .and_then(|p| p.byte_order.as_ref())
            .is_some_and(|bo| *bo == ByteOrder::MsbFirst),
        timestamp_size: xcp.map(|x| x.daq_info.timestamp_size).unwrap_or(0),
        events: xcp.map(|x| x.event_info.events.as_slice()).unwrap_or(&[]),
        daq_defs: xcp.map(|x| x.daq_info.daq_lists.as_slice()).unwrap_or(&[]),
        frame_len: xcp
            .and_then(|x| x.xcp_info.as_ref())
            .and_then(|t| match &t.can_fd {
                Some(fd) if fd.max_dlc_required => Some(fd.max_dlc as usize),
                _ if t.max_dlc_required => Some(8),
                _ => None,
            }),
    }
}

/// Push a command to the sequence with an ACK response.
fn push(cmds: &mut Vec<XcpCommand>, phase: XcpPhase, bytes: Vec<u8>, label: String) {
    push_full(cmds, phase, bytes, label, false);
}

/// Push a command that might not be supported
fn push_optional(cmds: &mut Vec<XcpCommand>, phase: XcpPhase, bytes: Vec<u8>, label: String) {
    push_full(cmds, phase, bytes, label, true);
}

fn push_full(
    cmds: &mut Vec<XcpCommand>,
    phase: XcpPhase,
    bytes: Vec<u8>,
    label: String,
    optional: bool,
) {
    cmds.push(XcpCommand {
        phase,
        bytes,
        label,
        response: XcpResponse::Ack,
        optional,
    });
}

/// Resource bit for GET_SEED/UNLOCK: the DAQ resource this pipeline drives.
const RESOURCE_DAQ: u8 = 0x04;

/// Phase builders
fn phase_connect(cmds: &mut Vec<XcpCommand>) {
    push(cmds, XcpPhase::Connect, vec![0xFF, 0x00], "CONNECT".into());
    push(cmds, XcpPhase::Connect, vec![0xFD], "GET_STATUS".into());
    push_optional(
        cmds,
        XcpPhase::Connect,
        vec![0xF8, 0x00, RESOURCE_DAQ],
        "GET_SEED  resource=DAQ".into(),
    );
    push_optional(
        cmds,
        XcpPhase::Connect,
        vec![0xF7, 0x06, 0, 0, 0, 0, 0, 0],
        "UNLOCK  key=zeros (dev placeholder; swap in a real seed-key algorithm here)".into(),
    );
}

/// Build the preparation sequence for a new DAQ session.
fn phase_prepare(cmds: &mut Vec<XcpCommand>, ctx: &Ctx, daq_lists: &[DaqList]) {
    push(
        cmds,
        XcpPhase::Prepare,
        vec![0xDD, 0x00],
        "START_STOP_SYNCH  stop all".into(),
    );

    if ctx.is_static {
        for (idx, daq) in daq_lists.iter().enumerate() {
            let wire = ctx.wire_id(idx, daq);
            let id = word(wire, ctx.big_endian);
            push(
                cmds,
                XcpPhase::Prepare,
                vec![0xE3, 0x00, id[0], id[1]],
                format!("CLEAR_DAQ_LIST  daq={}", wire),
            );
        }
        return;
    }

    push(cmds, XcpPhase::Prepare, vec![0xD6], "FREE_DAQ".into());

    let daq_count = daq_lists.len() as u16;
    let cnt = word(daq_count, ctx.big_endian);
    push(
        cmds,
        XcpPhase::Prepare,
        vec![0xD5, 0x00, cnt[0], cnt[1]],
        format!("ALLOC_DAQ  count={}", daq_count),
    );

    for (idx, daq) in daq_lists.iter().enumerate() {
        let n = daq.odts.len() as u8;
        let id = word(ctx.wire_id(idx, daq), ctx.big_endian);
        push(
            cmds,
            XcpPhase::Prepare,
            vec![0xD4, 0x00, id[0], id[1], n],
            format!("ALLOC_ODT  daq={} odts={}", idx, n),
        );
    }

    for (idx, daq) in daq_lists.iter().enumerate() {
        let id = word(ctx.wire_id(idx, daq), ctx.big_endian);
        for (odt_idx, odt) in daq.odts.iter().enumerate() {
            let n = odt.entries.len() as u8;
            push(
                cmds,
                XcpPhase::Prepare,
                vec![0xD3, 0x00, id[0], id[1], odt_idx as u8, n],
                format!("ALLOC_ODT_ENTRY  daq={} odt={} entries={}", idx, odt_idx, n),
            );
        }
    }
}

/// Assign each dynamic list's CAN id via `SET_DAQ_ID`, so it stops sharing the transport's
/// default id. Lists with no override, a `FIXED` A2L id, or a `Dynamic` list left unassigned
/// are untouched: they keep transmitting on the default id, same as before this phase existed.
fn phase_set_daq_id(cmds: &mut Vec<XcpCommand>, ctx: &Ctx, daq_lists: &[DaqList]) {
    for (idx, daq) in daq_lists.iter().enumerate() {
        let Some(can_id) = daq.can_id.set_daq_id() else {
            continue;
        };
        let wire = ctx.wire_id(idx, daq);
        let id = word(wire, ctx.big_endian);
        let can = dword(encode_can_id(can_id), ctx.big_endian);
        push(
            cmds,
            XcpPhase::Prepare,
            vec![0xF2, 0xFD, id[0], id[1], can[0], can[1], can[2], can[3]],
            format!("SET_DAQ_ID  daq={} can_id=0x{:X}", wire, can_id),
        );
    }
}

/// Encodes a CAN identifier for the `SET_DAQ_ID`/`GET_DAQ_ID` DWORD field: bit 31 set flags a 29-bit extended id.
fn encode_can_id(can_id: u32) -> u32 {
    if can_id > 0x7FF {
        can_id | 0x8000_0000
    } else {
        can_id
    }
}

/// Build the command sequence for writing DAQ entries.
fn phase_write_entries(cmds: &mut Vec<XcpCommand>, ctx: &Ctx, daq_lists: &[DaqList]) {
    for (idx, daq) in daq_lists.iter().enumerate() {
        let wire = ctx.wire_id(idx, daq);
        let id = word(wire, ctx.big_endian);
        for (odt_idx, odt) in daq.odts.iter().enumerate() {
            push(
                cmds,
                XcpPhase::WriteEntries,
                vec![0xE2, 0x00, id[0], id[1], odt_idx as u8, 0x00],
                format!("SET_DAQ_PTR  daq={} odt={} entry=0", wire, odt_idx),
            );
            for entry in &odt.entries {
                let label = entry.names.join("/");
                let addr = entry.ecu_address as u32;
                let a = dword(addr, ctx.big_endian);
                push(
                    cmds,
                    XcpPhase::WriteEntries,
                    vec![0xE1, 0xFF, entry.size as u8, 0x00, a[0], a[1], a[2], a[3]],
                    format!(
                        "WRITE_DAQ  {} size={} addr=0x{:08X}",
                        label, entry.size, addr
                    ),
                );
            }
        }
    }
}

/// Build the command sequence for setting DAQ list modes.
fn phase_set_mode(cmds: &mut Vec<XcpCommand>, ctx: &Ctx, daq_lists: &[DaqList]) {
    let mode: u8 = if ctx.timestamp_size > 0 { 0x10 } else { 0x00 };

    for (idx, daq) in daq_lists.iter().enumerate() {
        let wire = ctx.wire_id(idx, daq);
        let event_fixed = ctx
            .daq_defs
            .iter()
            .find(|d| d.identifier == daq.id)
            .and_then(|d| d.event_fixed);
        let event_ch: u16 = event_fixed.unwrap_or_else(|| {
            daq.event
                .as_deref()
                .and_then(|name| ctx.events.iter().find(|e| e.name == name))
                .map(|e| e.channel as u16)
                .unwrap_or(0)
        });
        let id = word(wire, ctx.big_endian);
        let ev = word(event_ch, ctx.big_endian);
        // Prescaler 1; priority 0
        push(
            cmds,
            XcpPhase::SetMode,
            vec![0xE0, mode, id[0], id[1], ev[0], ev[1], 0x01, 0x00],
            format!(
                "SET_DAQ_LIST_MODE  daq={} event={} mode=0x{:02X}",
                wire, event_ch, mode
            ),
        );
    }
}

/// Build the command sequence for starting DAQ lists.
fn phase_start(cmds: &mut Vec<XcpCommand>, ctx: &Ctx, daq_lists: &[DaqList]) {
    for (idx, daq) in daq_lists.iter().enumerate() {
        let wire = ctx.wire_id(idx, daq);
        let id = word(wire, ctx.big_endian);
        cmds.push(XcpCommand {
            phase: XcpPhase::Start,
            bytes: vec![0xDE, 0x02, id[0], id[1]],
            label: format!("START_STOP_DAQ_LIST  select daq={}", wire),
            response: XcpResponse::FirstPid { daq_index: idx },
            optional: false,
        });
    }
    push(
        cmds,
        XcpPhase::Start,
        vec![0xDD, 0x01],
        "START_STOP_SYNCH  start selected".into(),
    );
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CanIdSource, DaqInfo, DaqMode, EventInfo, Odt, OdtEntry, XcpData};

    fn project(mode: DaqMode, daq_defs: Vec<DaqListDef>, timestamp_size: u16) -> A2lProject {
        A2lProject {
            xcp_data: Some(XcpData {
                daq_info: DaqInfo {
                    mode,
                    daq_lists: daq_defs,
                    timestamp_size,
                    ..Default::default()
                },
                event_info: EventInfo {
                    events: vec![EventEntry {
                        channel: 3,
                        name: "Rev".into(),
                        ..Default::default()
                    }],
                },
                xcp_info: None,
                protocol_layer: None,
            }),
            ..Default::default()
        }
    }

    fn entry(name: &str, addr: u64) -> OdtEntry {
        OdtEntry {
            names: vec![name.into()],
            ecu_address: addr,
            size: 4,
        }
    }

    fn one_daq_list(id: u16) -> Vec<DaqList> {
        vec![DaqList {
            id,
            first_pid: 0,
            odts: vec![Odt {
                entries: vec![entry("A", 0x1000), entry("B", 0x2000)],
            }],
            event: Some("Rev".into()),
            can_id: CanIdSource::Default,
        }]
    }

    fn labels(cmds: &[XcpCommand]) -> Vec<&str> {
        cmds.iter().map(|c| c.label.as_str()).collect()
    }

    #[test]
    fn phases_appear_in_order() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        let cmds = build_init_commands(&p, &one_daq_list(0));
        let order = [
            XcpPhase::Connect,
            XcpPhase::Prepare,
            XcpPhase::WriteEntries,
            XcpPhase::SetMode,
            XcpPhase::Start,
        ];
        let mut last = 0;
        for c in &cmds {
            let pos = order.iter().position(|p| *p == c.phase).unwrap();
            assert!(pos >= last, "phase went backwards at {}", c.label);
            last = pos;
        }
        for phase in order {
            assert!(cmds.iter().any(|c| c.phase == phase), "missing {phase:?}");
        }
    }

    #[test]
    fn static_prepare_clears_lists_and_never_allocates() {
        let p = project(DaqMode::Static, vec![], 0);
        let cmds = build_init_commands(&p, &one_daq_list(2));
        let l = labels(&cmds);

        assert!(l.iter().any(|c| c.starts_with("CLEAR_DAQ_LIST  daq=2")));
        assert!(!l.iter().any(|c| c.starts_with("FREE_DAQ")));
        assert!(!l.iter().any(|c| c.starts_with("ALLOC_DAQ")));
        assert!(!l.iter().any(|c| c.starts_with("ALLOC_ODT")));
    }

    #[test]
    fn dynamic_prepare_allocates_and_omits_clear() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        let cmds = build_init_commands(&p, &one_daq_list(0));
        let l = labels(&cmds);

        assert!(l.iter().any(|c| c.starts_with("FREE_DAQ")));
        assert!(l.iter().any(|c| c.starts_with("ALLOC_DAQ  count=1")));
        assert!(l.iter().any(|c| c.starts_with("ALLOC_ODT ")));
        assert!(l.iter().any(|c| c.starts_with("ALLOC_ODT_ENTRY ")));
        assert!(!l.iter().any(|c| c.starts_with("CLEAR_DAQ_LIST")));
    }

    #[test]
    fn prepare_starts_with_stop_all() {
        for mode in [DaqMode::Static, DaqMode::Dynamic] {
            let p = project(mode, vec![], 0);
            let cmds = build_init_commands(&p, &one_daq_list(0));
            let first_prepare = cmds
                .iter()
                .find(|c| c.phase == XcpPhase::Prepare)
                .expect("prepare phase present");
            assert_eq!(first_prepare.bytes, vec![0xDD, 0x00], "{mode:?}");
        }
    }

    #[test]
    fn dynamic_lists_are_renumbered_from_zero_on_the_wire() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        let cmds = build_init_commands(&p, &one_daq_list(5));

        for code in [0xD4u8, 0xD3, 0xE2, 0xE0, 0xDE] {
            let cmd = cmds.iter().find(|c| c.bytes[0] == code).unwrap();
            assert_eq!(
                &cmd.bytes[2..4],
                &[0x00, 0x00],
                "command 0x{code:02X} must address allocated list 0: {}",
                cmd.label
            );
        }
    }

    #[test]
    fn dynamic_lists_are_numbered_after_the_predefined_ones() {
        let mut p = project(DaqMode::Dynamic, vec![], 0);
        p.xcp_data.as_mut().unwrap().daq_info.min_daq = 2;
        let cmds = build_init_commands(&p, &one_daq_list(5));

        for code in [0xD4u8, 0xD3, 0xE2, 0xE0, 0xDE] {
            let cmd = cmds.iter().find(|c| c.bytes[0] == code).unwrap();
            assert_eq!(
                &cmd.bytes[2..4],
                &[0x02, 0x00],
                "0x{code:02X}: {}",
                cmd.label
            );
        }
    }

    #[test]
    fn static_lists_keep_their_a2l_identifier_on_the_wire() {
        let p = project(DaqMode::Static, vec![], 0);
        let cmds = build_init_commands(&p, &one_daq_list(5));
        for code in [0xE3u8, 0xE2, 0xE0, 0xDE] {
            let cmd = cmds.iter().find(|c| c.bytes[0] == code).unwrap();
            assert_eq!(&cmd.bytes[2..4], &[0x05, 0x00], "{}", cmd.label);
        }
    }

    #[test]
    fn set_daq_ptr_once_per_odt_at_entry_zero() {
        let daq = vec![DaqList {
            id: 0,
            first_pid: 0,
            odts: vec![
                Odt {
                    entries: vec![entry("A", 0x1000), entry("B", 0x2000), entry("C", 0x3000)],
                },
                Odt {
                    entries: vec![entry("D", 0x4000)],
                },
            ],
            event: Some("Rev".into()),
            can_id: CanIdSource::Default,
        }];
        let p = project(DaqMode::Static, vec![], 0);
        let cmds = build_init_commands(&p, &daq);

        let ptrs: Vec<&XcpCommand> = cmds.iter().filter(|c| c.bytes[0] == 0xE2).collect();
        assert_eq!(ptrs.len(), 2, "exactly one SET_DAQ_PTR per ODT");
        for ptr in &ptrs {
            assert_eq!(ptr.bytes[5], 0x00, "entry number must always be 0");
        }
        let writes = cmds.iter().filter(|c| c.bytes[0] == 0xE1).count();
        assert_eq!(
            writes, 4,
            "one WRITE_DAQ per entry, pointer auto-increments"
        );
    }

    #[test]
    fn event_fixed_list_gets_mode_with_fixed_channel() {
        let defs = vec![DaqListDef {
            identifier: 0,
            event_fixed: Some(7),
            ..Default::default()
        }];
        let p = project(DaqMode::Static, defs, 0);
        let cmds = build_init_commands(&p, &one_daq_list(0));
        let mode = cmds
            .iter()
            .find(|c| c.bytes[0] == 0xE0)
            .expect("EVENT_FIXED lists still take SET_DAQ_LIST_MODE");
        assert_eq!(
            &mode.bytes[4..6],
            &[0x07, 0x00],
            "event must be the fixed channel"
        );
    }

    #[test]
    fn non_fixed_list_resolves_event_channel_by_name() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        let cmds = build_init_commands(&p, &one_daq_list(0));
        let mode = cmds.iter().find(|c| c.bytes[0] == 0xE0).unwrap();
        assert_eq!(&mode.bytes[4..6], &[0x03, 0x00], "channel of event 'Rev'");
    }

    #[test]
    fn timestamp_declared_sets_mode_bit_and_priority_is_zero() {
        let with_ts = build_init_commands(&project(DaqMode::Dynamic, vec![], 4), &one_daq_list(0));
        let without = build_init_commands(&project(DaqMode::Dynamic, vec![], 0), &one_daq_list(0));

        let mode_with = with_ts.iter().find(|c| c.bytes[0] == 0xE0).unwrap();
        let mode_without = without.iter().find(|c| c.bytes[0] == 0xE0).unwrap();
        assert_eq!(mode_with.bytes[1], 0x10, "timestamp bit set");
        assert_eq!(mode_without.bytes[1], 0x00, "no timestamp declared");
        assert_eq!(mode_with.bytes[6], 0x01, "prescaler 1");
        assert_eq!(mode_with.bytes[7], 0x00, "priority 0");
    }

    #[test]
    fn select_response_is_marked_as_first_pid_carrier() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        let cmds = build_init_commands(&p, &one_daq_list(0));

        let select = cmds.iter().find(|c| c.bytes[0] == 0xDE).unwrap();
        assert_eq!(select.bytes[1], 0x02, "mode = select");
        assert_eq!(select.response, XcpResponse::FirstPid { daq_index: 0 });

        assert!(cmds
            .iter()
            .filter(|c| c.bytes[0] != 0xDE)
            .all(|c| c.response == XcpResponse::Ack));
        assert_eq!(cmds.last().unwrap().bytes, vec![0xDD, 0x01]);
    }

    #[test]
    fn word_params_follow_slave_byte_order() {
        use crate::types::ProtocolLayerInfo;

        let make = |bo: ByteOrder| {
            let mut p = project(DaqMode::Static, vec![], 0);
            p.xcp_data.as_mut().unwrap().protocol_layer = Some(ProtocolLayerInfo {
                byte_order: Some(bo),
                ..Default::default()
            });
            p
        };
        let find = |cmds: &[XcpCommand], code: u8| -> Vec<u8> {
            cmds.iter()
                .find(|c| c.bytes[0] == code)
                .unwrap()
                .bytes
                .clone()
        };

        // Standard 11-bit id: distinct from the WRITE_DAQ address below, and low enough
        // to leave the extended-id bit clear (that encoding is covered separately).
        const ASSIGNED_CAN_ID: u32 = 0x123;
        let daq_with_id = {
            let mut d = one_daq_list(0x0102);
            d[0].can_id = CanIdSource::Dynamic {
                assigned: Some(ASSIGNED_CAN_ID),
            };
            d
        };
        let le = build_init_commands(&make(ByteOrder::MsbLast), &daq_with_id);
        let be = build_init_commands(&make(ByteOrder::MsbFirst), &daq_with_id);
        assert_eq!(&find(&le, 0xE2)[2..4], &[0x02, 0x01], "little-endian word");
        assert_eq!(&find(&be, 0xE2)[2..4], &[0x01, 0x02], "big-endian word");
        assert_eq!(&find(&be, 0xDE)[2..4], &[0x01, 0x02]);

        let w_le = find(&le, 0xE1);
        let w_be = find(&be, 0xE1);
        assert_eq!(&w_le[4..8], &0x1000u32.to_le_bytes());
        assert_eq!(&w_be[4..8], &0x1000u32.to_be_bytes());

        // SET_DAQ_ID's DAQ_LIST_NUMBER/CAN id fields follow the same slave byte order as every other CTO command here.
        let id_le = find(&le, 0xF2);
        let id_be = find(&be, 0xF2);
        assert_eq!(&id_le[2..4], &[0x02, 0x01], "little-endian list index");
        assert_eq!(&id_be[2..4], &[0x01, 0x02], "big-endian list index");
        assert_eq!(&id_le[4..8], &ASSIGNED_CAN_ID.to_le_bytes());
        assert_eq!(&id_be[4..8], &ASSIGNED_CAN_ID.to_be_bytes());
    }

    #[test]
    fn get_seed_and_unlock_are_optional_connect_phase_commands() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        let cmds = build_init_commands(&p, &one_daq_list(0));

        let get_seed = cmds.iter().find(|c| c.bytes[0] == 0xF8).unwrap();
        assert_eq!(get_seed.phase, XcpPhase::Connect);
        assert_eq!(get_seed.bytes, vec![0xF8, 0x00, 0x04], "mode=start, resource=DAQ");
        assert!(get_seed.optional, "GET_SEED must not abort init if unsupported");

        let unlock = cmds.iter().find(|c| c.bytes[0] == 0xF7).unwrap();
        assert_eq!(unlock.phase, XcpPhase::Connect);
        assert_eq!(unlock.bytes, vec![0xF7, 0x06, 0, 0, 0, 0, 0, 0], "6-byte zero key");
        assert!(unlock.optional, "UNLOCK must not abort init if unsupported");

        assert!(
            !cmds
                .iter()
                .any(|c| !matches!(c.bytes[0], 0xF7 | 0xF8) && c.optional),
            "every other command stays mandatory"
        );
    }

    #[test]
    fn stop_commands_start_with_stop_all_and_end_with_disconnect() {
        for mode in [DaqMode::Static, DaqMode::Dynamic] {
            let p = project(mode, vec![], 0);
            let cmds = build_stop_commands(&p, &one_daq_list(0));
            assert_eq!(cmds.first().unwrap().bytes, vec![0xDD, 0x00], "{mode:?}");
            assert_eq!(cmds.last().unwrap().bytes, vec![0xFE], "{mode:?}");
            assert!(cmds.iter().all(|c| c.phase == XcpPhase::Stop));
        }
    }

    #[test]
    fn static_stop_clears_every_list_by_a2l_identifier_and_never_frees() {
        let p = project(DaqMode::Static, vec![], 0);
        let daq = vec![
            DaqList {
                id: 2,
                first_pid: 0,
                odts: vec![Odt {
                    entries: vec![entry("A", 0x1000)],
                }],
                event: Some("Rev".into()),
                can_id: CanIdSource::Default,
            },
            DaqList {
                id: 7,
                first_pid: 0,
                odts: vec![Odt {
                    entries: vec![entry("B", 0x2000)],
                }],
                event: Some("Rev".into()),
                can_id: CanIdSource::Default,
            },
        ];
        let cmds = build_stop_commands(&p, &daq);
        let l = labels(&cmds);

        assert!(l.iter().any(|c| c.starts_with("CLEAR_DAQ_LIST  daq=2")));
        assert!(l.iter().any(|c| c.starts_with("CLEAR_DAQ_LIST  daq=7")));
        assert!(!l.iter().any(|c| c.starts_with("FREE_DAQ")));
    }

    #[test]
    fn dynamic_stop_frees_once_instead_of_clearing_per_list() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        let cmds = build_stop_commands(&p, &one_daq_list(0));
        let l = labels(&cmds);

        assert_eq!(l.iter().filter(|c| c.starts_with("FREE_DAQ")).count(), 1);
        assert!(!l.iter().any(|c| c.starts_with("CLEAR_DAQ_LIST")));
    }

    fn daq_list_with_can_id(id: u16, can_id: CanIdSource) -> Vec<DaqList> {
        let mut daq = one_daq_list(id);
        daq[0].can_id = can_id;
        daq
    }

    #[test]
    fn dynamic_list_with_assigned_id_gets_set_daq_id_in_prepare() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        let daq = daq_list_with_can_id(
            0,
            CanIdSource::Dynamic {
                assigned: Some(0x789),
            },
        );
        let cmds = build_init_commands(&p, &daq);

        let set = cmds
            .iter()
            .find(|c| c.bytes[0] == 0xF2)
            .expect("SET_DAQ_ID must be sent");
        assert_eq!(set.phase, XcpPhase::Prepare);
        assert_eq!(set.response, XcpResponse::Ack);
        assert_eq!(set.bytes[1], 0xFD, "SET_DAQ_ID sub-command");
        assert_eq!(&set.bytes[2..4], &[0x00, 0x00], "allocated list index 0");
        assert_eq!(
            &set.bytes[4..8],
            &0x789u32.to_le_bytes(),
            "standard 11-bit id, extended-id bit clear"
        );
    }

    #[test]
    fn extended_can_id_sets_the_top_bit() {
        let p = project(DaqMode::Static, vec![], 0);
        let daq = daq_list_with_can_id(
            3,
            CanIdSource::Dynamic {
                assigned: Some(0x1ABCDEF),
            },
        );
        let cmds = build_init_commands(&p, &daq);

        let set = cmds.iter().find(|c| c.bytes[0] == 0xF2).unwrap();
        assert_eq!(
            &set.bytes[2..4],
            &[0x03, 0x00],
            "static keeps its A2L identifier"
        );
        let can_word = u32::from_le_bytes(set.bytes[4..8].try_into().unwrap());
        assert_eq!(can_word & 0x8000_0000, 0x8000_0000, "extended-id bit set");
        assert_eq!(can_word & 0x1FFF_FFFF, 0x1ABCDEF);
    }

    #[test]
    fn no_set_daq_id_without_a_user_assignment() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        for can_id in [
            CanIdSource::Default,
            CanIdSource::Fixed { id: 0x789 },
            CanIdSource::Dynamic { assigned: None },
        ] {
            let daq = daq_list_with_can_id(0, can_id);
            let cmds = build_init_commands(&p, &daq);
            assert!(
                !cmds.iter().any(|c| c.bytes[0] == 0xF2),
                "{can_id:?} must not emit SET_DAQ_ID"
            );
        }
    }

    #[test]
    fn max_dlc_required_pads_every_frame_to_the_required_length() {
        use crate::types::{CanFdInfo, XcpInfo};
        let with = |classic: bool, fd: Option<CanFdInfo>| {
            let mut p = project(DaqMode::Dynamic, vec![], 0);
            p.xcp_data.as_mut().unwrap().xcp_info = Some(XcpInfo {
                max_dlc_required: classic,
                can_fd: fd,
                ..Default::default()
            });
            let daq = one_daq_list(0);
            let mut lens: Vec<usize> = build_init_commands(&p, &daq)
                .into_iter()
                .chain(build_stop_commands(&p, &daq))
                .map(|c| c.bytes.len())
                .collect();
            lens.sort_unstable();
            lens.dedup();
            lens
        };

        assert!(with(false, None).contains(&1), "no padding unless required");
        assert_eq!(with(true, None), vec![8]);
        let fd = |required| CanFdInfo {
            max_dlc: 64,
            max_dlc_required: required,
            ..Default::default()
        };
        assert_eq!(with(false, Some(fd(true))), vec![64]);
        assert_eq!(
            with(true, Some(fd(false))),
            vec![8],
            "classic flag only: pad to 8"
        );
    }
}
