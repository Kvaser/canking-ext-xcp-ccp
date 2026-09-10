use crate::a2l::{A2lProject, ByteOrder, CompuMethod, ParsedSignal};
use crate::daq::DaqList;
use crate::layout::{daq_can_id, daq_layout, timestamp_bytes, Field, SignalPlacement};
use crate::traceability::traceability_lines;
use crate::Protocol;
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter, Write as FmtWrite};

/// Errors that can occur while building a DBC file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbcError {
    /// The A2L has no `XCP_ON_CAN` `CAN_ID_SLAVE`, so the DAQ (DTO) message has no ID.
    MissingCanIdSlave,
    /// The A2L declares no CCP `TP_BLOB`, so the DAQ (DTO) message has no ID.
    MissingCcpDtoId,
}

impl Display for DbcError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCanIdSlave => {
                f.write_str("A2L has no XCP_ON_CAN CAN_ID_SLAVE; cannot build DBC")
            }
            Self::MissingCcpDtoId => {
                f.write_str("A2L has no CCP TP_BLOB DTO CAN id; cannot build DBC")
            }
        }
    }
}

impl Error for DbcError {}

/// The fixed DBC preamble: format version, the `NS_` new-symbol list, and the single `Slave` node every DAQ message is transmitted by.
const DBC_HEADER: &str = "VERSION \"\"\n\
\n\
NS_ : \n\
\tNS_DESC_\n\
\tCM_\n\
\tBA_DEF_\n\
\tBA_\n\
\tVAL_\n\
\tCAT_DEF_\n\
\tCAT_\n\
\tFILTER\n\
\tBA_DEF_DEF_\n\
\tEV_DATA_\n\
\tENVVAR_DATA_\n\
\tSGTYPE_\n\
\tSGTYPE_VAL_\n\
\tBA_DEF_SGTYPE_\n\
\tBA_SGTYPE_\n\
\tSIG_TYPE_REF_\n\
\tVAL_TABLE_\n\
\tSIG_GROUP_\n\
\tSIG_VALTYPE_\n\
\tSIGTYPE_VALTYPE_\n\
\tBO_TX_BU_\n\
\tBA_DEF_REL_\n\
\tBA_REL_\n\
\tBA_DEF_DEF_REL_\n\
\tBU_SG_REL_\n\
\tBU_EV_REL_\n\
\tBU_BO_REL_\n\
\tSG_MUL_VAL_\n\
\n\
BS_:\n\
\n\
BU_: Slave\n\
\n";

/// The DBC message name every DAQ CAN id gets
pub const DAQ_MESSAGE_NAME: &str = "DAQ";

fn message_name(can_id: u32, total_messages: usize) -> String {
    if total_messages <= 1 {
        DAQ_MESSAGE_NAME.to_string()
    } else {
        format!("{DAQ_MESSAGE_NAME}_{can_id:X}")
    }
}

pub fn create_dbc_file(
    project: &A2lProject,
    daq_lists: &[DaqList],
    protocol: Protocol,
    source_name: &str,
) -> Result<String, DbcError> {
    let default_can_id = daq_can_id(project, protocol).ok_or(match protocol {
        Protocol::Xcp => DbcError::MissingCanIdSlave,
        Protocol::Ccp => DbcError::MissingCcpDtoId,
    })?;

    let can_fd = project
        .xcp_data
        .as_ref()
        .and_then(|x| x.xcp_info.as_ref())
        .and_then(|c| c.can_fd.as_ref());

    let timestamp_bytes = timestamp_bytes(project);
    let groups = crate::layout::group_by_can_id(daq_lists, default_can_id);

    // Output
    let mut out = String::new();
    out.push_str(DBC_HEADER);

    let mut sig_valtypes = String::new();
    let mut val_tables = String::new();
    let mut val_table_emitted: HashSet<String> = HashSet::new();
    let mut fd_frame_formats = String::new();

    for (can_id, lists) in &groups {
        // DLC = max payload bytes of any single ODT on this CAN id + 1 for the PID byte.
        let max_odt_bytes: u16 = lists
            .iter()
            .flat_map(|d| d.odts.iter().enumerate())
            .map(|(odt_idx, odt)| {
                let data: u16 = odt.entries.iter().map(|e| e.size).sum();
                data + if odt_idx == 0 { timestamp_bytes } else { 0 }
            })
            .max()
            .unwrap_or(0);
        let needed = max_odt_bytes + 1;
        let dlc = match (can_fd, protocol) {
            (Some(fd), _) => fd_frame_len(needed).min(fd.max_dlc.max(8)),
            (None, Protocol::Ccp) => 8,
            (None, Protocol::Xcp) => needed.min(8),
        };

        let name = message_name(*can_id, groups.len());
        let _ = writeln!(out, "BO_ {can_id} {name}: {dlc} Slave");
        out.push_str(" SG_ PID M : 0|8@1+ (1,0) [0|255] \"\"  Unspecified__XXX\n");

        for field in daq_layout(project, lists) {
            let placement = match field {
                Field::Timestamp(ts) => {
                    let _ = writeln!(
                        out,
                        " SG_ Timestamp_{} m{} : {}|{}@1+ (1,0) [0|4294967295] \"\"  Unspecified__XXX",
                        ts.pid, ts.pid, ts.start_bit, ts.bit_length,
                    );
                    continue;
                }
                Field::Signal(p) => p,
            };

            let Some(sig) = placement.signal.as_ref() else {
                // Unresolved name: raw unsigned pass-through
                let _ = writeln!(
                    out,
                    " SG_ {} m{} : {}|{}@1+ (1,0) [0|{}] \"\"  Unspecified__XXX",
                    sanitize_name(&placement.name),
                    placement.pid,
                    placement.start_bit,
                    placement.bit_length,
                    format_dbc_number(2f64.powi(placement.bit_length as i32) - 1.0),
                );
                continue;
            };

            out.push_str(&format_signal(&placement, sig));
            if sig.is_float {
                let effective_size = placement.chunk_size.unwrap_or(sig.size);
                let valtype = if effective_size >= 8 { 2 } else { 1 };
                let _ = writeln!(
                    sig_valtypes,
                    "SIG_VALTYPE_ {} {} : {};",
                    can_id,
                    sanitize_name(&placement.name),
                    valtype,
                );
            }
            if let Some(cm) = sig
                .conversion
                .as_ref()
                .and_then(|c| project.compu_methods.get(&c.identifier))
            {
                if !cm.text_table.is_empty() && val_table_emitted.insert(sig.name.clone()) {
                    val_tables.push_str(&format_val_table(
                        *can_id,
                        &sanitize_name(&placement.name),
                        cm,
                    ));
                }
            }
        }

        if can_fd.is_some() {
            fd_frame_formats.push_str(&fd_frame_format_line(*can_id));
        }
    }

    let prov = traceability_lines(source_name, project, daq_lists, protocol);
    if !prov.is_empty() {
        let body = prov.join("\n").replace('"', "'");
        out.push('\n');
        let _ = writeln!(out, "CM_ \"{body}\";");
    }

    if !sig_valtypes.is_empty() {
        out.push('\n');
        out.push_str(&sig_valtypes);
    }

    if !val_tables.is_empty() {
        out.push('\n');
        out.push_str(&val_tables);
    }

    if can_fd.is_some() {
        out.push('\n');
        out.push_str(&fd_attribute_definitions());
        out.push_str(&fd_frame_formats);
    }

    out.push('\n');
    Ok(out)
}

/// Returns the smallest valid CAN FD frame length that can hold `needed` bytes.
fn fd_frame_len(needed: u16) -> u16 {
    const VALID: [u16; 8] = [8, 12, 16, 20, 24, 32, 48, 64];
    VALID
        .iter()
        .copied()
        .find(|&len| len >= needed)
        .unwrap_or(64)
}

/// Returns the DBC attribute definitions shared by every CAN FD message, emitted once per file.
fn fd_attribute_definitions() -> String {
    let mut out = String::new();
    out.push_str("BA_DEF_ \"BusType\" STRING ;\n");
    out.push_str("BA_DEF_ BO_ \"VFrameFormat\" ENUM  \"StandardCAN\",\"ExtendedCAN\",\"reserved\",\
                \"J1939PG\",\"reserved\",\"reserved\",\"reserved\",\"reserved\",\"reserved\",\"reserved\",\
                \"reserved\",\"reserved\",\"reserved\",\"reserved\",\"StandardCAN_FD\",\"ExtendedCAN_FD\";\n");
    out.push_str("BA_DEF_DEF_ \"BusType\" \"CAN FD\";\n");
    out.push_str("BA_DEF_DEF_ \"VFrameFormat\" \"StandardCAN\";\n");
    out.push_str("BA_ \"BusType\" \"CAN FD\";\n");
    out
}

/// Returns the `VFrameFormat` attribute value for one CAN FD message.
fn fd_frame_format_line(can_id: u32) -> String {
    let frame_format = if can_id > 0x7FF { 15 } else { 14 };
    format!("BA_ \"VFrameFormat\" BO_ {can_id} {frame_format};\n")
}

/// Render the `SG_` line for a placement whose name resolved to `sig`.
///
/// Geometry and scaling come from the shared [`layout`] pass; `sig` supplies the signedness and the declared physical range.
fn format_signal(placement: &SignalPlacement, sig: &ParsedSignal) -> String {
    let (byte_order_char, dbc_start_bit) = match placement.byte_order() {
        ByteOrder::MsbLast => ('1', placement.start_bit),
        ByteOrder::MsbFirst => ('0', placement.start_bit + 7),
    };

    let value_type = if sig.is_signed { '-' } else { '+' };

    let safe_unit = sanitize_unit(&placement.unit);
    let (min, max) = sig.physical_limits(placement.bit_length, placement.factor, placement.offset);
    format!(
        " SG_ {} m{} : {}|{}@{}{} ({},{}) [{}|{}] \"{}\"  Unspecified__XXX\n",
        sanitize_name(&sig.name),
        placement.pid,
        dbc_start_bit,
        placement.bit_length,
        byte_order_char,
        value_type,
        format_dbc_number(placement.factor),
        format_dbc_number(placement.offset),
        format_dbc_number(min),
        format_dbc_number(max),
        safe_unit,
    )
}

/// Renders a `VAL_` line mapping raw values to their TAB_VERB texts.
fn format_val_table(can_id: u32, name: &str, cm: &CompuMethod) -> String {
    let mut pairs = String::new();
    for entry in &cm.text_table {
        if entry.lower != entry.upper {
            continue;
        }
        let _ = write!(
            pairs,
            " {} \"{}\"",
            format_dbc_number(entry.lower),
            entry.text
        );
    }
    if pairs.is_empty() {
        return String::new();
    }
    format!("VAL_ {} {}{} ;\n", can_id, name, pairs)
}

/// Render a number the way a DBC field wants it: integral values without a decimal point.
///
/// Not interchangeable with `inline_signals::t_float`, which renders the same values for t
/// source and must always emit a decimal point so the compiler reads them as floats.
fn format_dbc_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

/// Replaces characters not valid in a DBC identifier ([A-Za-z0-9_]) with underscores.
/// Ensures the name starts with a letter or underscore.
pub fn sanitize_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// Replaces characters not valid in a DBC unit with underscores, and replaces some Unicode symbols with ASCII equivalents.
fn sanitize_unit(unit: &str) -> String {
    let mut out = String::with_capacity(unit.len());
    for c in unit.chars() {
        match c {
            '"' => out.push('\''),
            '\u{03BC}' => out.push('µ'),
            '\u{2103}' => out.push_str("°C"),
            '\u{2109}' => out.push_str("°F"),
            '\u{03A9}' | '\u{2126}' => out.push_str("Ohm"),
            '\u{2030}' => out.push_str("o/oo"),
            c if c == ' ' || c.is_ascii_graphic() => out.push(c),
            c if ('\u{A1}'..='\u{FF}').contains(&c) => out.push(c),
            _ => out.push('_'),
        }
    }
    out
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2l::{CanFdInfo, CanIdSource, DaqInfo, EventInfo, XcpData, XcpInfo};
    use crate::daq::{DaqList, Odt, OdtEntry};

    fn project(can_fd: Option<CanFdInfo>) -> A2lProject {
        A2lProject {
            xcp_data: Some(XcpData {
                daq_info: DaqInfo::default(),
                event_info: EventInfo::default(),
                xcp_info: Some(XcpInfo {
                    can_id_slave: Some(0x456),
                    can_fd,
                    ..Default::default()
                }),
                protocol_layer: None,
            }),
            ..Default::default()
        }
    }

    fn daq_lists_16_bytes() -> Vec<DaqList> {
        let entries = (0..4)
            .map(|i| OdtEntry {
                names: vec![format!("S{i}")],
                ecu_address: 0x10 * (i + 1),
                size: 4,
            })
            .collect();
        vec![DaqList {
            id: 0,
            first_pid: 0,
            event: None,
            odts: vec![Odt { entries }],
            can_id: CanIdSource::Default,
        }]
    }

    #[test]
    fn can_fd_message_uses_valid_fd_dlc_and_attributes() {
        let dbc = create_dbc_file(
            &project(Some(CanFdInfo {
                max_dlc: 24,
                data_baudrate: Some(2_000_000),
            })),
            &daq_lists_16_bytes(),
            Protocol::Xcp,
            "test.a2l",
        )
        .unwrap();
        assert!(dbc.contains("BO_ 1110 DAQ: 20 Slave"), "got:\n{dbc}");
        assert!(dbc.contains("BA_ \"BusType\" \"CAN FD\";"));
        assert!(dbc.contains("BA_ \"VFrameFormat\" BO_ 1110 14;"));
    }

    #[test]
    fn classic_can_caps_dlc_at_8_without_fd_attributes() {
        let dbc = create_dbc_file(
            &project(None),
            &daq_lists_16_bytes(),
            Protocol::Xcp,
            "test.a2l",
        )
        .unwrap();
        assert!(dbc.contains("BO_ 1110 DAQ: 8 Slave"), "got:\n{dbc}");
        assert!(!dbc.contains("VFrameFormat"));
        assert!(!dbc.contains("BusType"));
    }

    #[test]
    fn float_measurement_gets_sig_valtype_not_plain_integer() {
        use crate::a2l::ParsedSignal;
        let mut proj = project(None);
        proj.signals = vec![ParsedSignal {
            name: "FloatSignal".into(),
            size: 4,
            is_float: true,
            is_signed: true,
            ecu_address: 0x20005EFC,
            ..Default::default()
        }];
        let daq = vec![DaqList {
            id: 0,
            first_pid: 0,
            event: None,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["FloatSignal".into()],
                    ecu_address: 0x20005EFC,
                    size: 4,
                }],
            }],
            can_id: CanIdSource::Default,
        }];
        let dbc = create_dbc_file(&proj, &daq, Protocol::Xcp, "test.a2l").unwrap();
        assert!(
            dbc.contains("SIG_VALTYPE_ 1110 FloatSignal : 1;"),
            "got:\n{dbc}"
        );
    }

    #[test]
    fn tab_verb_signal_gets_val_table() {
        use crate::a2l::{CompuMethod, Conversion, ParsedSignal, VTabEntry};
        let mut proj = project(None);
        proj.signals = vec![ParsedSignal {
            name: "EngSt".into(),
            size: 1,
            ecu_address: 0x10,
            conversion: Some(Conversion {
                identifier: "CM_EngSt".into(),
            }),
            ..Default::default()
        }];
        proj.compu_methods.insert(
            "CM_EngSt".into(),
            CompuMethod {
                factor: 1.0,
                offset: 0.0,
                unit: "-".into(),
                text_table: vec![
                    VTabEntry {
                        lower: 0.0,
                        upper: 0.0,
                        text: "ENGST_DEFAULT".into(),
                    },
                    VTabEntry {
                        lower: 1.0,
                        upper: 1.0,
                        text: "ENGST_START".into(),
                    },
                    VTabEntry {
                        lower: 2.0,
                        upper: 5.0,
                        text: "ENGST_RUNNING".into(),
                    },
                ],
                default_text: None,
                formula: None,
            },
        );
        let daq = vec![DaqList {
            id: 0,
            first_pid: 0,
            event: None,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["EngSt".into()],
                    ecu_address: 0x10,
                    size: 1,
                }],
            }],
            can_id: CanIdSource::Default,
        }];
        let dbc = create_dbc_file(&proj, &daq, Protocol::Xcp, "test.a2l").unwrap();
        assert!(
            dbc.contains("VAL_ 1110 EngSt 0 \"ENGST_DEFAULT\" 1 \"ENGST_START\" ;"),
            "got:\n{dbc}"
        );
        assert!(!dbc.contains("ENGST_RUNNING"));
    }

    #[test]
    fn timestamp_fixed_offsets_first_odt_and_emits_timestamp_signal() {
        use crate::a2l::ParsedSignal;
        let mut proj = project(None);
        if let Some(x) = proj.xcp_data.as_mut() {
            x.daq_info.timestamp_size = 4;
            x.daq_info.timestamp_fixed = true;
        }
        proj.signals = vec![ParsedSignal {
            name: "Sig".into(),
            size: 2,
            ecu_address: 0x10,
            ..Default::default()
        }];
        let daq = vec![DaqList {
            id: 0,
            first_pid: 0,
            event: None,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["Sig".into()],
                    ecu_address: 0x10,
                    size: 2,
                }],
            }],
            can_id: CanIdSource::Default,
        }];
        let dbc = create_dbc_file(&proj, &daq, Protocol::Xcp, "test.a2l").unwrap();
        assert!(dbc.contains(" SG_ Timestamp_0 m0 : 8|32@1+"), "got:\n{dbc}");
        assert!(
            dbc.contains(" SG_ Sig m0 : 40|"),
            "signal must start past the timestamp\n{dbc}"
        );
    }

    #[test]
    fn timestamp_declared_but_not_fixed_is_still_inserted() {
        use crate::a2l::ParsedSignal;
        let mut proj = project(None);
        if let Some(x) = proj.xcp_data.as_mut() {
            x.daq_info.timestamp_size = 4;
            x.daq_info.timestamp_fixed = false;
        }
        proj.signals = vec![ParsedSignal {
            name: "Sig".into(),
            size: 2,
            ecu_address: 0x10,
            ..Default::default()
        }];
        let daq = vec![DaqList {
            id: 0,
            first_pid: 0,
            event: None,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["Sig".into()],
                    ecu_address: 0x10,
                    size: 2,
                }],
            }],
            can_id: CanIdSource::Default,
        }];
        let dbc = create_dbc_file(&proj, &daq, Protocol::Xcp, "test.a2l").unwrap();
        assert!(dbc.contains(" SG_ Timestamp_0 m0 : 8|32@1+"), "got:\n{dbc}");
        assert!(
            dbc.contains(" SG_ Sig m0 : 40|"),
            "signal must start past the timestamp\n{dbc}"
        );
    }

    #[test]
    fn no_timestamp_when_not_declared() {
        use crate::a2l::ParsedSignal;
        let mut proj = project(None);
        if let Some(x) = proj.xcp_data.as_mut() {
            x.daq_info.timestamp_size = 0;
            x.daq_info.timestamp_fixed = false;
        }
        proj.signals = vec![ParsedSignal {
            name: "Sig".into(),
            size: 2,
            ecu_address: 0x10,
            ..Default::default()
        }];
        let daq = vec![DaqList {
            id: 0,
            first_pid: 0,
            event: None,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["Sig".into()],
                    ecu_address: 0x10,
                    size: 2,
                }],
            }],
            can_id: CanIdSource::Default,
        }];
        let dbc = create_dbc_file(&proj, &daq, Protocol::Xcp, "test.a2l").unwrap();
        assert!(!dbc.contains("Timestamp_0"));
        assert!(
            dbc.contains(" SG_ Sig m0 : 8|"),
            "signal starts right after PID\n{dbc}"
        );
    }

    #[test]
    fn missing_can_id_slave_returns_error() {
        let proj = A2lProject::default(); // no xcp_data at all
        let err = create_dbc_file(&proj, &daq_lists_16_bytes(), Protocol::Xcp, "test.a2l");
        assert_eq!(err, Err(DbcError::MissingCanIdSlave));
    }

    #[test]
    fn ccp_dbc_uses_dto_id_as_message_id() {
        use crate::{CcpData, CcpInfo};
        let mut proj = project(None);
        proj.xcp_data = None;
        proj.ccp_data = Some(CcpData {
            info: CcpInfo {
                dto_id: 0x456,
                ..Default::default()
            },
            ..Default::default()
        });
        let dbc = create_dbc_file(&proj, &daq_lists_16_bytes(), Protocol::Ccp, "mecu.a2l").unwrap();
        assert!(dbc.contains("BO_ 1110 DAQ:"), "got:\n{dbc}");
    }

    #[test]
    fn ccp_without_transport_is_an_error() {
        let proj = A2lProject::default();
        let err = create_dbc_file(&proj, &daq_lists_16_bytes(), Protocol::Ccp, "mecu.a2l");
        assert_eq!(err, Err(DbcError::MissingCcpDtoId));
    }

    #[test]
    fn dbc_embeds_traceability_comment() {
        let dbc = create_dbc_file(
            &project(None),
            &daq_lists_16_bytes(),
            Protocol::Xcp,
            "MyEcu.a2l",
        )
        .unwrap();
        assert!(dbc.contains("CM_ \""), "expected a CM_ comment:\n{dbc}");
        assert!(dbc.contains("Source A2L: MyEcu.a2l"), "got:\n{dbc}");
        assert!(dbc.contains("Protocol: XCP"), "got:\n{dbc}");
    }

    #[test]
    fn sanitize_unit_handles_quotes_and_non_latin1() {
        assert_eq!(sanitize_unit(r#"m/s""#), "m/s'");
        assert_eq!(sanitize_unit("µs"), "µs");
        assert_eq!(sanitize_unit("\u{03BC}s"), "µs");
        assert_eq!(sanitize_unit("°C"), "°C");
        assert_eq!(sanitize_unit("\u{2103}"), "°C");
        assert_eq!(sanitize_unit("k\u{2126}"), "kOhm");
        assert_eq!(sanitize_unit("m²"), "m²");
        assert_eq!(sanitize_unit("转速"), "__");
    }

    #[test]
    fn emits_one_message_per_can_id() {
        use crate::a2l::ParsedSignal;
        let mut proj = project(None);
        proj.signals = vec![
            ParsedSignal {
                name: "Rpm".into(),
                size: 2,
                ecu_address: 0x10,
                ..Default::default()
            },
            ParsedSignal {
                name: "Load".into(),
                size: 1,
                ecu_address: 0x2000,
                ..Default::default()
            },
        ];
        let daq = vec![
            DaqList {
                id: 0,
                first_pid: 0,
                event: None,
                odts: vec![Odt {
                    entries: vec![OdtEntry {
                        names: vec!["Rpm".into()],
                        ecu_address: 0x10,
                        size: 2,
                    }],
                }],
                can_id: CanIdSource::Default,
            },
            DaqList {
                id: 1,
                first_pid: 0,
                event: None,
                odts: vec![Odt {
                    entries: vec![OdtEntry {
                        names: vec!["Load".into()],
                        ecu_address: 0x2000,
                        size: 1,
                    }],
                }],
                can_id: CanIdSource::Fixed { id: 0x789 },
            },
        ];
        let dbc = create_dbc_file(&proj, &daq, Protocol::Xcp, "test.a2l").unwrap();

        assert!(dbc.contains("BO_ 1110 DAQ_456: "), "got:\n{dbc}");
        assert!(dbc.contains(" SG_ Rpm m0 :"), "got:\n{dbc}");
        assert!(dbc.contains("BO_ 1929 DAQ_789: "), "got:\n{dbc}");
        assert!(dbc.contains(" SG_ Load m0 :"), "got:\n{dbc}");

        let rpm_pos = dbc.find(" SG_ Rpm").unwrap();
        let load_pos = dbc.find(" SG_ Load").unwrap();
        let second_bo_pos = dbc.find("BO_ 1929").unwrap();
        assert!(rpm_pos < second_bo_pos, "Rpm belongs to the first message");
        assert!(
            load_pos > second_bo_pos,
            "Load belongs to the second message"
        );
    }

    #[test]
    fn fd_frame_len_rounds_up_to_valid_lengths() {
        assert_eq!(fd_frame_len(1), 8);
        assert_eq!(fd_frame_len(8), 8);
        assert_eq!(fd_frame_len(9), 12);
        assert_eq!(fd_frame_len(17), 20);
        assert_eq!(fd_frame_len(33), 48);
        assert_eq!(fd_frame_len(65), 64);
    }
}
