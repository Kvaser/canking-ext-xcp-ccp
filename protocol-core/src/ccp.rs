//! Transport-agnostic CCP DAQ command generation (ASAM MCD-1 CCP 2.1).
//!
//! [`build_init_commands`] emits the DAQ initialisation sequence, [`build_stop_commands`] the shutdown sequence, using
//! the same five [`Phase`](crate::protocol::Phase)s so the runtime and t-script drive both protocols identically.
//!
//! Every command is a **CRO** (Command Receive Object), 8 bytes on the CRO CAN identifier: `[cmd, ctr, data0..data5]`.
//! Byte 1 is the **command counter** (`ctr`), incremented for each command and echoed back in the slave's CRM
//!
//! | Phase | Command | Bytes `[cmd, ctr, ...]` | Response |
//! |-------|---------|-----------------------|----------|
//! | `Connect` | `CONNECT 0x01` | `01 ctr sa_lo sa_hi` | ack |
//! | `Connect` | `GET_CCP_VERSION 0x1B` | `1B ctr 02 01` | ack |
//! | `Connect` | `GET_SEED 0x12` (optional) | `12 ctr resource 00 00 00 00 00` | ack |
//! | `Connect` | `UNLOCK 0x13` (optional) | `13 ctr key[6]` | ack |
//! | `Prepare` | `START_STOP_ALL 0x08` (stop) | `08 ctr 00` | ack |
//! | `Prepare` | `GET_DAQ_SIZE 0x14` (per list) | `14 ctr daq 00 <dto-can-id[4]>` | first PID (CRM byte 4) |
//! | `WriteEntries` | `SET_DAQ_PTR 0x15` (per element) | `15 ctr daq odt element` | ack |
//! | `WriteEntries` | `WRITE_DAQ 0x16` (per element) | `16 ctr size ext <addr[4]>` | ack |
//! | `SetMode` | `START_STOP 0x06` (prepare, per list) | `06 ctr 02 daq last_odt evt <prescaler[2]>` | ack |
//! | `Start` | `START_STOP_ALL 0x08` (start) | `08 ctr 01` | ack |
//! | `Stop` | `START_STOP_ALL 0x08` (stop) | `08 ctr 00` | ack |
//! | `Stop` | `DISCONNECT 0x07` (end) | `07 ctr 01 00 sa_lo sa_hi` | ack |
//!
//! Byte-order rules (CCP 2.1 §3):
//! - The **station address** (`CONNECT`/`DISCONNECT`) is always Intel/little-endian.
//! - Multi-byte **address** (`WRITE_DAQ`), **DTO CAN id** (`GET_DAQ_SIZE`) and **prescaler** (`START_STOP`) follow the slave's byte order.
//!
//! Two rules shape the sequence:
//! - CCP 2.1 does not require the slave to post-increment its ODT-element pointer, so every element gets its own `SET_DAQ_PTR`
//!   before its `WRITE_DAQ`. When the slave declares `BYTES_ONLY`, each multi-byte signal is registered as consecutive 1-byte `WRITE_DAQ`s
//! - `GET_DAQ_SIZE` both clears the DAQ list and reports its size and first PID which the first PID is carried back in CRM byte 4.
//! - `GET_SEED`/`UNLOCK` are marked optional not every slave implements them, and a development ECU with no real security algorithm typically accepts any key,
//! so `UNLOCK` is sent with an all-zero key. A real seed-dependent key algorithm would replace the key bytes built.
//!
//! Address extension is emitted as 0 for now

use crate::types::{A2lProject, ByteOrder, DaqList, EventEntry, ParsedSignal};
use std::collections::HashMap;

pub use crate::protocol::{
    Command as CcpCommand, ExpectedResponse as CcpResponse, Phase as CcpPhase,
};
use crate::protocol::{dword, word};

// CCP 2.1 command codes.
const CONNECT: u8 = 0x01;
const GET_CCP_VERSION: u8 = 0x1B;
const DISCONNECT: u8 = 0x07;
const START_STOP: u8 = 0x06;
const START_STOP_ALL: u8 = 0x08;
const GET_DAQ_SIZE: u8 = 0x14;
const SET_DAQ_PTR: u8 = 0x15;
const WRITE_DAQ: u8 = 0x16;
const GET_SEED: u8 = 0x12;
const UNLOCK: u8 = 0x13;

// START_STOP / START_STOP_ALL modes.
const MODE_STOP: u8 = 0x00;
const MODE_START: u8 = 0x01;
const MODE_PREPARE: u8 = 0x02;

/// Resource bit for GET_SEED/UNLOCK: the DAQ resource this pipeline drives.
const RESOURCE_DAQ: u8 = 0x02;

/// Snapshot of everything the phase builders need from the parsed project.
struct Ctx<'a> {
    big_endian: bool,
    station_address: u16,
    dto_id: u32,
    bytes_only: bool,
    rasters: &'a [EventEntry],
    signals: HashMap<&'a str, &'a ParsedSignal>,
}

/// Accumulates CRO commands, assigning each an auto-incrementing command counter.
struct Seq {
    cmds: Vec<CcpCommand>,
    ctr: u8,
}

impl Seq {
    fn new() -> Self {
        Seq {
            cmds: Vec::new(),
            ctr: 0,
        }
    }

    /// Push a command expecting a plain positive response.
    fn push(&mut self, phase: CcpPhase, cmd: u8, data: &[u8], label: String) {
        self.push_full(phase, cmd, data, label, CcpResponse::Ack, false);
    }

    /// Push a command, tagging the response it carries.
    fn push_resp(
        &mut self,
        phase: CcpPhase,
        cmd: u8,
        data: &[u8],
        label: String,
        response: CcpResponse,
    ) {
        self.push_full(phase, cmd, data, label, response, false);
    }

    fn push_optional(&mut self, phase: CcpPhase, cmd: u8, data: &[u8], label: String) {
        self.push_full(phase, cmd, data, label, CcpResponse::Ack, true);
    }

    fn push_full(
        &mut self,
        phase: CcpPhase,
        cmd: u8,
        data: &[u8],
        label: String,
        response: CcpResponse,
        optional: bool,
    ) {
        let mut bytes = Vec::with_capacity(2 + data.len());
        bytes.push(cmd);
        bytes.push(self.ctr);
        bytes.extend_from_slice(data);
        // A CRO is always 8 bytes
        bytes.resize(bytes.len().max(8), 0);
        self.ctr = self.ctr.wrapping_add(1);
        self.cmds.push(CcpCommand {
            phase,
            bytes,
            label,
            response,
            optional,
        });
    }
}

/// Build the full CCP DAQ initialisation command sequence from a parsed project and packed DAQ lists.
pub fn build_init_commands(project: &A2lProject, daq_lists: &[DaqList]) -> Vec<CcpCommand> {
    let ctx = build_ctx(project);
    let mut seq = Seq::new();
    phase_connect(&mut seq, &ctx);
    phase_prepare(&mut seq, &ctx, daq_lists);
    phase_write_entries(&mut seq, &ctx, daq_lists);
    phase_set_mode(&mut seq, &ctx, daq_lists);
    phase_start(&mut seq);
    seq.cmds
}

/// Build the shutdown sequence for a running DAQ session: stop all lists, then end the session with `DISCONNECT`.
pub fn build_stop_commands(project: &A2lProject, _daq_lists: &[DaqList]) -> Vec<CcpCommand> {
    let ctx = build_ctx(project);
    let mut seq = Seq::new();
    seq.push(
        CcpPhase::Stop,
        START_STOP_ALL,
        &[MODE_STOP],
        "START_STOP_ALL  stop all".into(),
    );
    let sa = ctx.station_address.to_le_bytes();
    seq.push(
        CcpPhase::Stop,
        DISCONNECT,
        &[0x01, 0x00, sa[0], sa[1]],
        format!(
            "DISCONNECT  end session station=0x{:04X}",
            ctx.station_address
        ),
    );
    seq.cmds
}

/// Snapshot the parsed project's CCP settings.
fn build_ctx(project: &A2lProject) -> Ctx<'_> {
    let ccp = project.ccp_data.as_ref();
    Ctx {
        big_endian: project
            .default_byte_order
            .as_ref()
            .is_some_and(|bo| *bo == ByteOrder::MsbFirst),
        station_address: ccp.map(|c| c.info.station_address).unwrap_or(0),
        dto_id: ccp.map(|c| c.info.dto_id).unwrap_or(0),
        bytes_only: ccp.is_some_and(|c| c.info.bytes_only),
        rasters: ccp.map(|c| c.rasters.as_slice()).unwrap_or(&[]),
        signals: crate::layout::signal_lookup(project),
    }
}

/// `CONNECT` (station address, little-endian) then `GET_CCP_VERSION` (desired 2.1).
fn phase_connect(seq: &mut Seq, ctx: &Ctx) {
    let sa = ctx.station_address.to_le_bytes();
    seq.push(
        CcpPhase::Connect,
        CONNECT,
        &[sa[0], sa[1]],
        format!("CONNECT  station=0x{:04X}", ctx.station_address),
    );
    seq.push(
        CcpPhase::Connect,
        GET_CCP_VERSION,
        &[0x02, 0x01],
        "GET_CCP_VERSION  2.1".into(),
    );
    seq.push_optional(
        CcpPhase::Connect,
        GET_SEED,
        &[RESOURCE_DAQ, 0, 0, 0, 0, 0],
        "GET_SEED  resource=DAQ".into(),
    );
    seq.push_optional(
        CcpPhase::Connect,
        UNLOCK,
        &[0, 0, 0, 0, 0, 0],
        "UNLOCK  key=zeros (dev placeholder; swap in a real seed-key algorithm here)".into(),
    );
}

/// Stop any running DAQ, then `GET_DAQ_SIZE` per list to clear it and learn its size and first PID.
fn phase_prepare(seq: &mut Seq, ctx: &Ctx, daq_lists: &[DaqList]) {
    seq.push(
        CcpPhase::Prepare,
        START_STOP_ALL,
        &[MODE_STOP],
        "START_STOP_ALL  stop all".into(),
    );
    let id = dword(ctx.dto_id, ctx.big_endian);
    for (idx, daq) in daq_lists.iter().enumerate() {
        let list = daq.id as u8;
        seq.push_resp(
            CcpPhase::Prepare,
            GET_DAQ_SIZE,
            &[list, 0x00, id[0], id[1], id[2], id[3]],
            format!("GET_DAQ_SIZE  daq={} dto=0x{:X}", list, ctx.dto_id),
            CcpResponse::FirstPid { daq_index: idx },
        );
    }
}

/// Per element: point the DAQ pointer at it, then write its element description.
fn phase_write_entries(seq: &mut Seq, ctx: &Ctx, daq_lists: &[DaqList]) {
    for daq in daq_lists {
        let list = daq.id as u8;
        for (odt_idx, odt) in daq.odts.iter().enumerate() {
            let odt_no = odt_idx as u8;
            let mut element: u8 = 0;

            for entry in &odt.entries {
                let addr = entry.ecu_address as u32;
                let ext = crate::layout::address_extension(&ctx.signals, entry);
                // A `BYTES_ONLY` slave rejects multi-byte elements so each signal is registered as consecutive 1-byte elements.
                let (chunk, count) = if ctx.bytes_only {
                    (1u16, entry.size)
                } else {
                    (entry.size, 1u16)
                };

                for i in 0..count as u32 {
                    let byte_addr = addr.wrapping_add(i * chunk as u32);
                    seq.push(
                        CcpPhase::WriteEntries,
                        SET_DAQ_PTR,
                        &[list, odt_no, element],
                        format!(
                            "SET_DAQ_PTR  daq={} odt={} element={}",
                            list, odt_no, element
                        ),
                    );
                    let a = dword(byte_addr, ctx.big_endian);
                    seq.push(
                        CcpPhase::WriteEntries,
                        WRITE_DAQ,
                        &[chunk as u8, ext, a[0], a[1], a[2], a[3]],
                        if ctx.bytes_only {
                            format!(
                                "WRITE_DAQ  {} byte {}/{} addr=0x{:08X}",
                                entry.names.join("/"),
                                i + 1,
                                entry.size,
                                byte_addr
                            )
                        } else {
                            format!(
                                "WRITE_DAQ  {} size={} addr=0x{:08X}",
                                entry.names.join("/"),
                                entry.size,
                                byte_addr
                            )
                        },
                    );
                    element += 1;
                }
            }
        }
    }
}

/// `START_STOP` in prepare mode per list: bind it to its raster (event channel),
/// declare its last ODT number and the transmission prescaler.
fn phase_set_mode(seq: &mut Seq, ctx: &Ctx, daq_lists: &[DaqList]) {
    for daq in daq_lists {
        let list = daq.id as u8;
        let last_odt = daq.odts.len().saturating_sub(1) as u8;
        let event_ch = daq
            .event
            .as_deref()
            .and_then(|name| ctx.rasters.iter().find(|r| r.name == name))
            .map(|r| r.channel as u8)
            .unwrap_or(0);
        // Prescaler 1 (every event triggers a transmission).
        let pre = word(1, ctx.big_endian);
        seq.push(
            CcpPhase::SetMode,
            START_STOP,
            &[MODE_PREPARE, list, last_odt, event_ch, pre[0], pre[1]],
            format!(
                "START_STOP  prepare daq={} last_odt={} event={}",
                list, last_odt, event_ch
            ),
        );
    }
}

/// Start every prepared DAQ list synchronously.
fn phase_start(seq: &mut Seq) {
    seq.push(
        CcpPhase::Start,
        START_STOP_ALL,
        &[MODE_START],
        "START_STOP_ALL  start".into(),
    );
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CanIdSource, CcpData, CcpInfo, Odt, OdtEntry};

    fn project(big_endian: bool) -> A2lProject {
        A2lProject {
            ccp_data: Some(CcpData {
                info: CcpInfo {
                    version: 0x0201,
                    cro_id: 0x455,
                    dto_id: 0x456,
                    station_address: 0x0100,
                    baudrate: Some(500_000),
                    ..Default::default()
                },
                rasters: vec![EventEntry {
                    channel: 3,
                    name: "DAQ_CHAN1".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            default_byte_order: Some(if big_endian {
                ByteOrder::MsbFirst
            } else {
                ByteOrder::MsbLast
            }),
            ..Default::default()
        }
    }

    fn one_daq_list() -> Vec<DaqList> {
        vec![DaqList {
            id: 0,
            first_pid: 0,
            odts: vec![Odt {
                entries: vec![
                    OdtEntry {
                        names: vec!["A".into()],
                        ecu_address: 0x1000,
                        size: 4,
                    },
                    OdtEntry {
                        names: vec!["B".into()],
                        ecu_address: 0x2000,
                        size: 2,
                    },
                ],
            }],
            event: Some("DAQ_CHAN1".into()),
            can_id: CanIdSource::Default,
        }]
    }

    fn find(cmds: &[CcpCommand], code: u8) -> &CcpCommand {
        cmds.iter().find(|c| c.bytes[0] == code).unwrap()
    }

    #[test]
    fn init_phases_appear_in_order() {
        let cmds = build_init_commands(&project(false), &one_daq_list());
        let order = [
            CcpPhase::Connect,
            CcpPhase::Prepare,
            CcpPhase::WriteEntries,
            CcpPhase::SetMode,
            CcpPhase::Start,
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
    fn connect_carries_station_address_little_endian() {
        let cmds = build_init_commands(&project(true), &one_daq_list());
        let connect = find(&cmds, CONNECT);
        assert_eq!(
            connect.bytes,
            vec![CONNECT, 0x00, 0x00, 0x01, 0, 0, 0, 0],
            "0x0100 LE"
        );
        assert!(
            cmds.iter().all(|c| c.bytes.len() == 8),
            "every CRO is 8 bytes"
        );
    }

    #[test]
    fn write_daq_carries_the_signal_address_extension() {
        let mut p = project(false);
        p.signals = vec![ParsedSignal {
            name: "A".into(),
            address_extension: 3,
            ..Default::default()
        }];
        let cmds = build_init_commands(&p, &one_daq_list());
        let writes: Vec<&[u8]> = cmds
            .iter()
            .filter(|c| c.bytes[0] == WRITE_DAQ)
            .map(|c| c.bytes.as_slice())
            .collect();
        assert_eq!(writes[0][3], 3, "A: extension from its MEASUREMENT");
        assert_eq!(writes.last().unwrap()[3], 0, "B: undeclared means 0");
    }

    #[test]
    fn get_seed_and_unlock_are_optional_connect_phase_commands() {
        let cmds = build_init_commands(&project(false), &one_daq_list());

        let get_seed = find(&cmds, GET_SEED);
        assert_eq!(get_seed.phase, CcpPhase::Connect);
        assert_eq!(&get_seed.bytes[2..], &[0x02, 0, 0, 0, 0, 0], "resource=DAQ");
        assert!(
            get_seed.optional,
            "GET_SEED must not abort init if unsupported"
        );

        let unlock = find(&cmds, UNLOCK);
        assert_eq!(unlock.phase, CcpPhase::Connect);
        assert_eq!(&unlock.bytes[2..], &[0, 0, 0, 0, 0, 0], "6-byte zero key");
        assert!(unlock.optional, "UNLOCK must not abort init if unsupported");

        assert!(
            !cmds
                .iter()
                .any(|c| !matches!(c.bytes[0], GET_SEED | UNLOCK) && c.optional),
            "every other command stays mandatory"
        );
    }

    #[test]
    fn command_counter_increments_per_command() {
        let cmds = build_init_commands(&project(false), &one_daq_list());
        for (i, c) in cmds.iter().enumerate() {
            assert_eq!(c.bytes[1], i as u8, "ctr at {}: {}", i, c.label);
        }
    }

    #[test]
    fn get_daq_size_carries_dto_id_in_slave_order_and_marks_first_pid() {
        let le = build_init_commands(&project(false), &one_daq_list());
        let be = build_init_commands(&project(true), &one_daq_list());
        let le_g = find(&le, GET_DAQ_SIZE);
        let be_g = find(&be, GET_DAQ_SIZE);
        assert_eq!(le_g.bytes[0], GET_DAQ_SIZE);
        assert_eq!(le_g.bytes[2], 0x00, "daq list 0");
        assert_eq!(&le_g.bytes[4..8], &0x456u32.to_le_bytes(), "dto id LE");
        assert_eq!(&be_g.bytes[4..8], &0x456u32.to_be_bytes(), "dto id BE");
        assert_eq!(le_g.response, CcpResponse::FirstPid { daq_index: 0 });
    }

    #[test]
    fn write_daq_uses_slave_byte_order_for_address() {
        let le = build_init_commands(&project(false), &one_daq_list());
        let be = build_init_commands(&project(true), &one_daq_list());
        let le_w = find(&le, WRITE_DAQ);
        let be_w = find(&be, WRITE_DAQ);
        assert_eq!(le_w.bytes[2], 4, "size");
        assert_eq!(le_w.bytes[3], 0, "address extension");
        assert_eq!(&le_w.bytes[4..8], &0x1000u32.to_le_bytes());
        assert_eq!(&be_w.bytes[4..8], &0x1000u32.to_be_bytes());
    }

    #[test]
    fn bytes_only_splits_entries_into_single_byte_writes() {
        let mut proj = project(true);
        proj.ccp_data.as_mut().unwrap().info.bytes_only = true;
        let cmds = build_init_commands(&proj, &one_daq_list());
        let writes: Vec<&CcpCommand> = cmds.iter().filter(|c| c.bytes[0] == WRITE_DAQ).collect();

        assert_eq!(writes.len(), 6, "one WRITE_DAQ per byte");
        assert!(
            writes.iter().all(|w| w.bytes[2] == 1),
            "every element size 1"
        );

        let ptrs: Vec<&CcpCommand> = cmds.iter().filter(|c| c.bytes[0] == SET_DAQ_PTR).collect();
        assert_eq!(ptrs.len(), 6, "one SET_DAQ_PTR per byte element");
        let elements: Vec<u8> = ptrs.iter().map(|p| p.bytes[4]).collect();
        assert_eq!(elements, vec![0, 1, 2, 3, 4, 5], "element index advances");

        for (i, w) in writes.iter().take(4).enumerate() {
            let expected = 0x1000u32 + i as u32;
            assert_eq!(&w.bytes[4..8], &expected.to_be_bytes(), "byte {i} address");
        }
        assert_eq!(&writes[4].bytes[4..8], &0x2000u32.to_be_bytes());
        assert_eq!(&writes[5].bytes[4..8], &0x2001u32.to_be_bytes());
    }

    #[test]
    fn every_write_daq_is_preceded_by_its_own_set_daq_ptr() {
        let cmds = build_init_commands(&project(false), &one_daq_list());

        let ptrs: Vec<&CcpCommand> = cmds.iter().filter(|c| c.bytes[0] == SET_DAQ_PTR).collect();
        let writes: Vec<&CcpCommand> = cmds.iter().filter(|c| c.bytes[0] == WRITE_DAQ).collect();
        assert_eq!(writes.len(), 2, "one WRITE_DAQ per entry");
        assert_eq!(ptrs.len(), 2, "one SET_DAQ_PTR per element, not per ODT");

        for (idx, ptr) in ptrs.iter().enumerate() {
            assert_eq!(ptr.bytes[2], 0, "daq list");
            assert_eq!(ptr.bytes[3], 0, "odt number");
            assert_eq!(ptr.bytes[4], idx as u8, "element index");
        }

        let order: Vec<u8> = cmds
            .iter()
            .filter(|c| c.bytes[0] == SET_DAQ_PTR || c.bytes[0] == WRITE_DAQ)
            .map(|c| c.bytes[0])
            .collect();
        assert_eq!(
            order,
            vec![SET_DAQ_PTR, WRITE_DAQ, SET_DAQ_PTR, WRITE_DAQ],
            "pointer/write must interleave"
        );
    }

    #[test]
    fn set_daq_ptr_element_resets_per_odt() {
        let mut lists = one_daq_list();
        lists[0].odts.push(Odt {
            entries: vec![OdtEntry {
                names: vec!["C".into()],
                ecu_address: 0x3000,
                size: 2,
            }],
        });
        let cmds = build_init_commands(&project(false), &lists);
        let ptrs: Vec<(u8, u8)> = cmds
            .iter()
            .filter(|c| c.bytes[0] == SET_DAQ_PTR)
            .map(|c| (c.bytes[3], c.bytes[4]))
            .collect();
        assert_eq!(ptrs, vec![(0, 0), (0, 1), (1, 0)]);
    }

    #[test]
    fn start_stop_prepare_binds_event_channel_and_last_odt() {
        let cmds = build_init_commands(&project(false), &one_daq_list());
        let ss = find(&cmds, START_STOP);
        assert_eq!(ss.bytes[2], MODE_PREPARE, "prepare mode");
        assert_eq!(ss.bytes[3], 0, "daq list 0");
        assert_eq!(ss.bytes[4], 0, "last odt (single ODT)");
        assert_eq!(ss.bytes[5], 3, "raster DAQ_CHAN1 channel 3");
        assert_eq!(&ss.bytes[6..8], &1u16.to_le_bytes(), "prescaler 1");
    }

    #[test]
    fn start_is_start_stop_all_start_and_is_last() {
        let cmds = build_init_commands(&project(false), &one_daq_list());
        let last = cmds.last().unwrap();
        assert_eq!(
            &last.bytes[..3],
            &[START_STOP_ALL, last.bytes[1], MODE_START]
        );
        assert_eq!(last.phase, CcpPhase::Start);
    }

    #[test]
    fn stop_sequence_stops_all_then_disconnects() {
        let cmds = build_stop_commands(&project(false), &one_daq_list());
        assert_eq!(&cmds[0].bytes[..3], &[START_STOP_ALL, 0x00, MODE_STOP]);
        let disc = cmds.last().unwrap();
        assert_eq!(disc.bytes[0], DISCONNECT);
        assert_eq!(disc.bytes[2], 0x01, "end of session");
        assert_eq!(
            &disc.bytes[4..6],
            &0x0100u16.to_le_bytes(),
            "station addr LE"
        );
        assert!(cmds.iter().all(|c| c.phase == CcpPhase::Stop));
    }
}
