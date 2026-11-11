//! XCP DAQ command generation, independent of the transport.
//!
//! [`build_init_commands`] emits the DAQ init sequence in five phases and [`build_stop_commands`] the shutdown.
//! Every command expects a positive response (`0xFF`); an error packet (`0xFE`) aborts the sequence unless the command is optional.
//! Commands whose response carries data the runtime uses are marked in [`Command::response`].
//!
//! | Phase | Commands |
//! |-------|----------|
//! | `Connect` | CONNECT, GET_STATUS, GET_SEED + UNLOCK (optional, resource DAQ) |
//! | `Prepare` | START_STOP_SYNCH (stop all); static: CLEAR_DAQ_LIST per list; dynamic: FREE_DAQ, ALLOC_DAQ, ALLOC_ODT, ALLOC_ODT_ENTRY; then SET_DAQ_ID per list with a user-assigned CAN id |
//! | `WriteEntries` | per ODT: SET_DAQ_PTR (entry 0), WRITE_DAQ per entry |
//! | `SetMode` | SET_DAQ_LIST_MODE per list (event channel, prescaler, timestamp) |
//! | `Start` | START_STOP_DAQ_LIST (select) per list, whose response carries the first PID; START_STOP_SYNCH (start) |
//! | `Stop` | START_STOP_SYNCH (stop all); static: CLEAR_DAQ_LIST per list; dynamic: FREE_DAQ; DISCONNECT |
//!
//! Rules that shape the bytes:
//!
//! - Commands are only as long as their content, unless the A2L sets `MAX_DLC_REQUIRED`: then they are zero-padded to 8 bytes, or to `MAX_DLC` when it is set in the CAN FD block.
//! - WRITE_DAQ auto-increments the slave's entry pointer, so one SET_DAQ_PTR per ODT is enough; some slaves reject a non-zero entry number anyway.
//!   Its address-extension byte is the signal's `ECU_ADDRESS_EXTENSION`.
//! - Dynamic lists are numbered `MIN_DAQ ..`, static lists keep their A2L identifier.
//! - The prescaler is one byte, so a list's prescaler is clamped to 1..=255.
//! - SET_DAQ_ID (`0xF2 0xFD`) is sent only for a `CanIdSource::Dynamic` list with a user-assigned id; every other list transmits on the transport's shared DAQ id.
//! - GET_SEED/UNLOCK are optional because many ECUs don't implement them. UNLOCK sends an all-zero key, which a development ECU without a real algorithm usually accepts.

use crate::protocol::{Command, ExpectedResponse, Phase, dword, word};
use crate::types::{
    A2lProject, ByteOrder, CanIdSource, DaqList, DaqListDef, EventEntry, ParsedSignal,
};
use std::collections::HashMap;

/// Snapshot of everything the phase builders need from the parsed project.
struct Ctx<'a> {
    is_static: bool,
    min_daq: u16,
    big_endian: bool,
    timestamp_size: u16,
    events: &'a [EventEntry],
    daq_defs: &'a [DaqListDef],
    frame_len: Option<usize>,
    signals: HashMap<&'a str, &'a ParsedSignal>,
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
pub fn build_init_commands(project: &A2lProject, daq_lists: &[DaqList]) -> Vec<Command> {
    let ctx = build_ctx(project);

    let mut cmds: Vec<Command> = Vec::new();
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
pub fn build_stop_commands(project: &A2lProject, daq_lists: &[DaqList]) -> Vec<Command> {
    let ctx = build_ctx(project);
    let mut cmds: Vec<Command> = Vec::new();
    stop_and_release(&mut cmds, Phase::Stop, &ctx, daq_lists);
    cmds.push(Command::ack(Phase::Stop, vec![0xFE], "DISCONNECT".into()));
    pad(&mut cmds, ctx.frame_len);
    cmds
}

/// Stop every list, then release them: CLEAR_DAQ_LIST per static list, one FREE_DAQ for dynamic ones.
fn stop_and_release(cmds: &mut Vec<Command>, phase: Phase, ctx: &Ctx, daq_lists: &[DaqList]) {
    cmds.push(Command::ack(
        phase,
        vec![0xDD, 0x00],
        "START_STOP_SYNCH  stop all".into(),
    ));
    if ctx.is_static {
        for (idx, daq) in daq_lists.iter().enumerate() {
            let wire = ctx.wire_id(idx, daq);
            let id = word(wire, ctx.big_endian);
            cmds.push(Command::ack(
                phase,
                vec![0xE3, 0x00, id[0], id[1]],
                format!("CLEAR_DAQ_LIST  daq={wire}"),
            ));
        }
    } else {
        cmds.push(Command::ack(phase, vec![0xD6], "FREE_DAQ".into()));
    }
}

/// Zero-pad every command to `len` bytes, for slaves that only accept full-length frames.
fn pad(cmds: &mut [Command], len: Option<usize>) {
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
        signals: crate::layout::signal_lookup(project),
    }
}

/// Resource bit for GET_SEED/UNLOCK: the DAQ resource this pipeline drives.
const RESOURCE_DAQ: u8 = 0x04;

/// Phase builders
fn phase_connect(cmds: &mut Vec<Command>) {
    cmds.push(Command::ack(
        Phase::Connect,
        vec![0xFF, 0x00],
        "CONNECT".into(),
    ));
    cmds.push(Command::ack(
        Phase::Connect,
        vec![0xFD],
        "GET_STATUS".into(),
    ));
    cmds.push(Command {
        optional: true,
        ..Command::ack(
            Phase::Connect,
            vec![0xF8, 0x00, RESOURCE_DAQ],
            "GET_SEED  resource=DAQ".into(),
        )
    });
    cmds.push(Command {
        optional: true,
        ..Command::ack(
            Phase::Connect,
            vec![0xF7, 0x06, 0, 0, 0, 0, 0, 0],
            "UNLOCK  key=zeros (dev placeholder; swap in a real seed-key algorithm here)".into(),
        )
    });
}

/// Build the preparation sequence for a new DAQ session.
fn phase_prepare(cmds: &mut Vec<Command>, ctx: &Ctx, daq_lists: &[DaqList]) {
    stop_and_release(cmds, Phase::Prepare, ctx, daq_lists);
    if ctx.is_static {
        return;
    }

    let daq_count = daq_lists.len() as u16;
    let cnt = word(daq_count, ctx.big_endian);
    cmds.push(Command::ack(
        Phase::Prepare,
        vec![0xD5, 0x00, cnt[0], cnt[1]],
        format!("ALLOC_DAQ  count={daq_count}"),
    ));

    for (idx, daq) in daq_lists.iter().enumerate() {
        let n = daq.odts.len() as u8;
        let id = word(ctx.wire_id(idx, daq), ctx.big_endian);
        cmds.push(Command::ack(
            Phase::Prepare,
            vec![0xD4, 0x00, id[0], id[1], n],
            format!("ALLOC_ODT  daq={idx} odts={n}"),
        ));
    }

    for (idx, daq) in daq_lists.iter().enumerate() {
        let id = word(ctx.wire_id(idx, daq), ctx.big_endian);
        for (odt_idx, odt) in daq.odts.iter().enumerate() {
            let n = odt.entries.len() as u8;
            cmds.push(Command::ack(
                Phase::Prepare,
                vec![0xD3, 0x00, id[0], id[1], odt_idx as u8, n],
                format!("ALLOC_ODT_ENTRY  daq={idx} odt={odt_idx} entries={n}"),
            ));
        }
    }
}

/// Assign each dynamic list's CAN id via `SET_DAQ_ID`, so it stops sharing the transport's default id.
/// Lists with no override, a `FIXED` A2L id, or a `Dynamic` list left unassigned are untouched: they keep transmitting on the default id.
fn phase_set_daq_id(cmds: &mut Vec<Command>, ctx: &Ctx, daq_lists: &[DaqList]) {
    for (idx, daq) in daq_lists.iter().enumerate() {
        let CanIdSource::Dynamic {
            assigned: Some(can_id),
        } = daq.can_id
        else {
            continue;
        };
        let wire = ctx.wire_id(idx, daq);
        let id = word(wire, ctx.big_endian);
        let can = dword(encode_can_id(can_id), ctx.big_endian);
        cmds.push(Command::ack(
            Phase::Prepare,
            vec![0xF2, 0xFD, id[0], id[1], can[0], can[1], can[2], can[3]],
            format!("SET_DAQ_ID  daq={wire} can_id=0x{can_id:X}"),
        ));
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
fn phase_write_entries(cmds: &mut Vec<Command>, ctx: &Ctx, daq_lists: &[DaqList]) {
    for (idx, daq) in daq_lists.iter().enumerate() {
        let wire = ctx.wire_id(idx, daq);
        let id = word(wire, ctx.big_endian);
        for (odt_idx, odt) in daq.odts.iter().enumerate() {
            cmds.push(Command::ack(
                Phase::WriteEntries,
                vec![0xE2, 0x00, id[0], id[1], odt_idx as u8, 0x00],
                format!("SET_DAQ_PTR  daq={wire} odt={odt_idx} entry=0"),
            ));
            for entry in &odt.entries {
                let addr = entry.ecu_address as u32;
                let a = dword(addr, ctx.big_endian);
                let ext = crate::layout::address_extension(&ctx.signals, entry);
                cmds.push(Command::ack(
                    Phase::WriteEntries,
                    vec![0xE1, 0xFF, entry.size as u8, ext, a[0], a[1], a[2], a[3]],
                    format!(
                        "WRITE_DAQ  {} size={} addr=0x{addr:08X}",
                        entry.name(),
                        entry.size
                    ),
                ));
            }
        }
    }
}

/// Build the command sequence for setting DAQ list modes.
fn phase_set_mode(cmds: &mut Vec<Command>, ctx: &Ctx, daq_lists: &[DaqList]) {
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
        // The prescaler is one byte in XCP; priority 0
        let prescaler = daq.prescaler.clamp(1, u8::MAX as u16) as u8;
        cmds.push(Command::ack(
            Phase::SetMode,
            vec![0xE0, mode, id[0], id[1], ev[0], ev[1], prescaler, 0x00],
            format!(
                "SET_DAQ_LIST_MODE  daq={wire} event={event_ch} mode=0x{mode:02X} prescaler={prescaler}"
            ),
        ));
    }
}

/// Build the command sequence for starting DAQ lists.
fn phase_start(cmds: &mut Vec<Command>, ctx: &Ctx, daq_lists: &[DaqList]) {
    for (idx, daq) in daq_lists.iter().enumerate() {
        let wire = ctx.wire_id(idx, daq);
        let id = word(wire, ctx.big_endian);
        cmds.push(Command {
            response: ExpectedResponse::FirstPid { daq_index: idx },
            ..Command::ack(
                Phase::Start,
                vec![0xDE, 0x02, id[0], id[1]],
                format!("START_STOP_DAQ_LIST  select daq={wire}"),
            )
        });
    }
    cmds.push(Command::ack(
        Phase::Start,
        vec![0xDD, 0x01],
        "START_STOP_SYNCH  start selected".into(),
    ));
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DaqInfo, DaqMode, EventInfo, Odt, OdtEntry, XcpData};

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
            prescaler: 1,
            id,
            first_pid: 0,
            odts: vec![Odt {
                entries: vec![entry("A", 0x1000), entry("B", 0x2000)],
            }],
            event: Some("Rev".into()),
            can_id: CanIdSource::Default,
        }]
    }

    fn labels(cmds: &[Command]) -> Vec<&str> {
        cmds.iter().map(|c| c.label.as_str()).collect()
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
                .find(|c| c.phase == Phase::Prepare)
                .expect("prepare phase present");
            assert_eq!(first_prepare.bytes, vec![0xDD, 0x00], "{mode:?}");
        }
    }

    #[test]
    fn dynamic_lists_are_numbered_from_min_daq_on_the_wire() {
        // MIN_DAQ 0 renumbers list 5 to allocated list 0; otherwise it follows the predefined ones.
        for min_daq in [0u16, 2] {
            let mut p = project(DaqMode::Dynamic, vec![], 0);
            p.xcp_data.as_mut().unwrap().daq_info.min_daq = min_daq;
            let cmds = build_init_commands(&p, &one_daq_list(5));

            for code in [0xD4u8, 0xD3, 0xE2, 0xE0, 0xDE] {
                let cmd = cmds.iter().find(|c| c.bytes[0] == code).unwrap();
                assert_eq!(
                    &cmd.bytes[2..4],
                    &min_daq.to_le_bytes(),
                    "0x{code:02X}: {}",
                    cmd.label
                );
            }
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
            prescaler: 1,
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

        let ptrs: Vec<&Command> = cmds.iter().filter(|c| c.bytes[0] == 0xE2).collect();
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
    fn set_daq_list_mode_carries_the_list_prescaler_as_one_byte() {
        let prescaler_byte = |prescaler| {
            let mut lists = one_daq_list(0);
            lists[0].prescaler = prescaler;
            let cmds = build_init_commands(&project(DaqMode::Dynamic, vec![], 0), &lists);
            cmds.iter().find(|c| c.bytes[0] == 0xE0).unwrap().bytes[6]
        };
        assert_eq!(prescaler_byte(100), 100);
        assert_eq!(
            prescaler_byte(300),
            255,
            "clamped to the byte XCP has for it"
        );
        assert_eq!(prescaler_byte(0), 1, "0 is not a valid prescaler");
    }

    #[test]
    fn select_response_is_marked_as_first_pid_carrier() {
        let p = project(DaqMode::Dynamic, vec![], 0);
        let cmds = build_init_commands(&p, &one_daq_list(0));

        let select = cmds.iter().find(|c| c.bytes[0] == 0xDE).unwrap();
        assert_eq!(select.bytes[1], 0x02, "mode = select");
        assert_eq!(select.response, ExpectedResponse::FirstPid { daq_index: 0 });

        assert!(
            cmds.iter()
                .filter(|c| c.bytes[0] != 0xDE)
                .all(|c| c.response == ExpectedResponse::Ack)
        );
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
        let find = |cmds: &[Command], code: u8| -> Vec<u8> {
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
    fn stop_commands_start_with_stop_all_and_end_with_disconnect() {
        for mode in [DaqMode::Static, DaqMode::Dynamic] {
            let p = project(mode, vec![], 0);
            let cmds = build_stop_commands(&p, &one_daq_list(0));
            assert_eq!(cmds.first().unwrap().bytes, vec![0xDD, 0x00], "{mode:?}");
            assert_eq!(cmds.last().unwrap().bytes, vec![0xFE], "{mode:?}");
            assert!(cmds.iter().all(|c| c.phase == Phase::Stop));
        }
    }

    #[test]
    fn static_stop_clears_every_list_by_a2l_identifier_and_never_frees() {
        let p = project(DaqMode::Static, vec![], 0);
        let daq = vec![
            DaqList {
                prescaler: 1,
                id: 2,
                first_pid: 0,
                odts: vec![Odt {
                    entries: vec![entry("A", 0x1000)],
                }],
                event: Some("Rev".into()),
                can_id: CanIdSource::Default,
            },
            DaqList {
                prescaler: 1,
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
        assert_eq!(set.phase, Phase::Prepare);
        assert_eq!(set.response, ExpectedResponse::Ack);
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
