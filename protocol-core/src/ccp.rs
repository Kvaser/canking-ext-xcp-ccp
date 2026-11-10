//! CCP 2.1 DAQ command generation.
//!
//! [`build_init_commands`] and [`build_stop_commands`] use the same five [`Phase`]s as XCP.
//! Every command is an 8-byte CRO on the CRO CAN id, `[cmd, ctr, data0..data5]`, where `ctr` counts up per command and the slave echoes it in its CRM.
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
//! Rules that shape the bytes:
//!
//! - The station address (`CONNECT`/`DISCONNECT`) is always little-endian. The `WRITE_DAQ` address, the `GET_DAQ_SIZE` DTO id and the `START_STOP` prescaler follow the slave's byte order.
//! - CCP doesn't require the slave to advance its element pointer, so every element gets its own `SET_DAQ_PTR`. With
//!   `BYTES_ONLY`, a multi-byte signal is written as consecutive 1-byte elements.
//! - `GET_DAQ_SIZE` clears the list and reports its first PID, which the runtime uses to correct the configured one.
//! - `WRITE_DAQ`'s `ext` byte is the signal's `ECU_ADDRESS_EXTENSION`.
//! - `GET_SEED`/`UNLOCK` are optional because not every slave implements them. `UNLOCK` sends an all-zero key.

use crate::protocol::{Command, ExpectedResponse, Phase, dword, word};
use crate::types::{A2lProject, ByteOrder, DaqList, EventEntry, ParsedSignal};
use std::collections::HashMap;

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
#[derive(Default)]
struct Seq {
    cmds: Vec<Command>,
    ctr: u8,
}

impl Seq {
    /// Push a mandatory command expecting a plain positive response, returned for the caller to adjust.
    fn push(&mut self, phase: Phase, cmd: u8, data: &[u8], label: String) -> &mut Command {
        let mut bytes = vec![cmd, self.ctr];
        bytes.extend_from_slice(data);
        // A CRO is always 8 bytes
        bytes.resize(bytes.len().max(8), 0);
        self.ctr = self.ctr.wrapping_add(1);
        self.cmds.push(Command::ack(phase, bytes, label));
        self.cmds.last_mut().expect("just pushed")
    }
}

/// Build the full CCP DAQ initialisation command sequence from a parsed project and packed DAQ lists.
pub fn build_init_commands(project: &A2lProject, daq_lists: &[DaqList]) -> Vec<Command> {
    let ctx = build_ctx(project);
    let mut seq = Seq::default();
    phase_connect(&mut seq, &ctx);
    phase_prepare(&mut seq, &ctx, daq_lists);
    phase_write_entries(&mut seq, &ctx, daq_lists);
    phase_set_mode(&mut seq, &ctx, daq_lists);
    phase_start(&mut seq);
    seq.cmds
}

/// Build the shutdown sequence for a running DAQ session: stop all lists, then end the session with `DISCONNECT`.
pub fn build_stop_commands(project: &A2lProject, _daq_lists: &[DaqList]) -> Vec<Command> {
    let ctx = build_ctx(project);
    let mut seq = Seq::default();
    seq.push(
        Phase::Stop,
        START_STOP_ALL,
        &[MODE_STOP],
        "START_STOP_ALL  stop all".into(),
    );
    let sa = ctx.station_address.to_le_bytes();
    seq.push(
        Phase::Stop,
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
        Phase::Connect,
        CONNECT,
        &[sa[0], sa[1]],
        format!("CONNECT  station=0x{:04X}", ctx.station_address),
    );
    seq.push(
        Phase::Connect,
        GET_CCP_VERSION,
        &[0x02, 0x01],
        "GET_CCP_VERSION  2.1".into(),
    );
    seq.push(
        Phase::Connect,
        GET_SEED,
        &[RESOURCE_DAQ, 0, 0, 0, 0, 0],
        "GET_SEED  resource=DAQ".into(),
    )
    .optional = true;
    seq.push(
        Phase::Connect,
        UNLOCK,
        &[0, 0, 0, 0, 0, 0],
        "UNLOCK  key=zeros (dev placeholder; swap in a real seed-key algorithm here)".into(),
    )
    .optional = true;
}

/// Stop any running DAQ, then `GET_DAQ_SIZE` per list to clear it and learn its size and first PID.
fn phase_prepare(seq: &mut Seq, ctx: &Ctx, daq_lists: &[DaqList]) {
    seq.push(
        Phase::Prepare,
        START_STOP_ALL,
        &[MODE_STOP],
        "START_STOP_ALL  stop all".into(),
    );
    let id = dword(ctx.dto_id, ctx.big_endian);
    for (idx, daq) in daq_lists.iter().enumerate() {
        let list = daq.id as u8;
        seq.push(
            Phase::Prepare,
            GET_DAQ_SIZE,
            &[list, 0x00, id[0], id[1], id[2], id[3]],
            format!("GET_DAQ_SIZE  daq={list} dto=0x{:X}", ctx.dto_id),
        )
        .response = ExpectedResponse::FirstPid { daq_index: idx };
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
                        Phase::WriteEntries,
                        SET_DAQ_PTR,
                        &[list, odt_no, element],
                        format!("SET_DAQ_PTR  daq={list} odt={odt_no} element={element}"),
                    );
                    let a = dword(byte_addr, ctx.big_endian);
                    let what = if ctx.bytes_only {
                        format!("byte {}/{}", i + 1, entry.size)
                    } else {
                        format!("size={}", entry.size)
                    };
                    seq.push(
                        Phase::WriteEntries,
                        WRITE_DAQ,
                        &[chunk as u8, ext, a[0], a[1], a[2], a[3]],
                        format!("WRITE_DAQ  {} {what} addr=0x{byte_addr:08X}", entry.name()),
                    );
                    element += 1;
                }
            }
        }
    }
}

/// `START_STOP` in prepare mode per list: bind it to its raster (event channel), declare its last ODT number and the transmission prescaler.
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
        let prescaler = daq.prescaler.max(1);
        let pre = word(prescaler, ctx.big_endian);
        seq.push(
            Phase::SetMode,
            START_STOP,
            &[MODE_PREPARE, list, last_odt, event_ch, pre[0], pre[1]],
            format!(
                "START_STOP  prepare daq={list} last_odt={last_odt} event={event_ch} prescaler={prescaler}"
            ),
        );
    }
}

/// Start every prepared DAQ list synchronously.
fn phase_start(seq: &mut Seq) {
    seq.push(
        Phase::Start,
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
            prescaler: 1,
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

    fn find(cmds: &[Command], code: u8) -> &Command {
        cmds.iter().find(|c| c.bytes[0] == code).unwrap()
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
        assert_eq!(le_g.response, ExpectedResponse::FirstPid { daq_index: 0 });
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
        let writes: Vec<&Command> = cmds.iter().filter(|c| c.bytes[0] == WRITE_DAQ).collect();

        assert_eq!(writes.len(), 6, "one WRITE_DAQ per byte");
        assert!(
            writes.iter().all(|w| w.bytes[2] == 1),
            "every element size 1"
        );

        let ptrs: Vec<&Command> = cmds.iter().filter(|c| c.bytes[0] == SET_DAQ_PTR).collect();
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

        let ptrs: Vec<&Command> = cmds.iter().filter(|c| c.bytes[0] == SET_DAQ_PTR).collect();
        let writes: Vec<&Command> = cmds.iter().filter(|c| c.bytes[0] == WRITE_DAQ).collect();
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
    fn start_stop_prepare_carries_the_list_prescaler_in_slave_byte_order() {
        let mut lists = one_daq_list();
        lists[0].prescaler = 1000;
        let little = build_init_commands(&project(false), &lists);
        let big = build_init_commands(&project(true), &lists);
        assert_eq!(
            &find(&little, START_STOP).bytes[6..8],
            &1000u16.to_le_bytes()
        );
        assert_eq!(&find(&big, START_STOP).bytes[6..8], &1000u16.to_be_bytes());
    }

    #[test]
    fn start_is_start_stop_all_start_and_is_last() {
        let cmds = build_init_commands(&project(false), &one_daq_list());
        let last = cmds.last().unwrap();
        assert_eq!(
            &last.bytes[..3],
            &[START_STOP_ALL, last.bytes[1], MODE_START]
        );
        assert_eq!(last.phase, Phase::Start);
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
        assert!(cmds.iter().all(|c| c.phase == Phase::Stop));
    }
}
