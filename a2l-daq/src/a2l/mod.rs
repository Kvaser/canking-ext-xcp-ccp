//! A2L file parsing. This module is the main entry point for parsing an A2L file into a structured `A2lProject`.
//!
//! `tokenizer` turns source text into a token stream and offers the quick scan functions that avoid a full parse;
//!
//! `datatype` is the single source of truth for A2L base datatypes.
//!
//! `measurement`, `xcp`, `ccp`, `compu_method`, and `common` each parse one A2L block family;
//!
//! `virtual_signal` resolves virtual measurements against real ones.
//!
//!  `formula` is the expression compiler that compu_method needs.
//!
//! `summary` is a UI-facing view over a parse result.
//!
//! `encoding` decodes raw file bytes.

use std::collections::HashSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

mod ccp;
mod common;
mod compu_method;
mod datatype;
mod encoding;
mod formula;
mod group;
mod measurement;
mod summary;
mod tokenizer;
mod virtual_signal;
mod xcp;

// Re export the protocol-core types
pub use protocol_core::protocol::Protocol;
pub use protocol_core::types::{
    A2lProject, ByteOrder, CanFdInfo, CanIdSource, CcpData, CcpInfo, CcpSource, CompuMethod,
    Conversion, DaqInfo, DaqListDef, DaqMode, EventEntry, EventInfo, ParsedSignal,
    ProtocolLayerInfo, VTabEntry, XcpData, XcpInfo,
};

pub use encoding::decode_a2l_bytes;
pub use summary::{inspect, A2lSummary, GroupInfo, SignalSummary, TransportInfo};
pub use virtual_signal::{RejectedVirtual, RejectedVirtualReason};

use tokenizer::{scan_events, scan_protocols, scan_signal_names, tokenize_checked};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A2lParseError {
    EmptyInput,
}

impl Display for A2lParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => f.write_str("A2L input is empty"),
        }
    }
}

impl Error for A2lParseError {}

// ------------------------------------- Public Methods -------------------------------------

/// Returns the names of all MEASUREMENT signals without doing a full parse.
pub fn list_signal_names(content: &str) -> Result<Vec<String>, A2lParseError> {
    Ok(scan_signal_names(&tokenize_checked(content)?))
}

/// Returns every XCP EVENT channel declared in the file without a full parse.
pub fn list_events(content: &str) -> Result<Vec<EventEntry>, A2lParseError> {
    Ok(scan_events(&tokenize_checked(content)?))
}

/// Returns the measurement protocols declared via `IF_DATA` blocks without a full parse.
pub fn list_protocols(content: &str) -> Result<Vec<Protocol>, A2lParseError> {
    Ok(scan_protocols(&tokenize_checked(content)?))
}

/// The event channels `protocol` can trigger DAQ sampling on.
///
/// XCP declares them as `EVENT` blocks, CCP as `RASTER` blocks; both land in an [`EventEntry`]
/// Returns an empty slice when the A2L declares no transport for `protocol`.
pub fn event_channels(project: &A2lProject, protocol: Protocol) -> &[EventEntry] {
    match protocol {
        Protocol::Xcp => project
            .xcp_data
            .as_ref()
            .map(|x| x.event_info.events.as_slice())
            .unwrap_or_default(),
        Protocol::Ccp => project
            .ccp_data
            .as_ref()
            .map(|c| c.rasters.as_slice())
            .unwrap_or_default(),
    }
}

/// Parses an A2L file into a structured `A2lProject`.
pub fn parse_a2l(content: &str, selected_signals: &[String]) -> Result<A2lProject, A2lParseError> {
    Ok(parse_a2l_with_rejects(content, selected_signals)?.0)
}

pub fn parse_a2l_with_rejects(
    content: &str,
    selected_signals: &[String],
) -> Result<(A2lProject, Vec<RejectedVirtual>), A2lParseError> {
    let tokens = tokenize_checked(content)?;
    Ok(parse_tokens(&tokens, selected_refs(selected_signals)))
}

/// Trims and drops empty entries from the caller's selected-signal list.
fn selected_refs(selected_signals: &[String]) -> Vec<&str> {
    selected_signals
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parses an already-tokenized A2L file into a structured `A2lProject`,
/// together with the virtual measurements that had to be dropped from it.
fn parse_tokens(tokens: &[String], selected: Vec<&str>) -> (A2lProject, Vec<RejectedVirtual>) {
    let mut signals = measurement::parse_measurements(tokens, &selected);
    let mut borrowed: HashSet<String> = HashSet::new();

    if !selected.is_empty() {
        let have: HashSet<String> = signals.iter().map(|s| s.name.clone()).collect();
        let missing: Vec<String> = signals
            .iter()
            .flat_map(|s| s.virtual_inputs.iter())
            .filter(|name| !have.contains(*name))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let refs: Vec<&str> = missing.iter().map(String::as_str).collect();
            for extra in measurement::parse_measurements(tokens, &refs) {
                borrowed.insert(extra.name.clone());
                signals.push(extra);
            }
        }
    }

    let xcp_data = xcp::parse_if_data_xcp(tokens);
    let ccp_data = ccp::parse_if_data_ccp(tokens);
    let compu_methods = compu_method::parse_compu_methods(tokens);
    let default_byte_order = common::parse_mod_common(tokens);

    if let Some(ref bo) = default_byte_order {
        for signal in &mut signals {
            if signal.byte_order.is_none() {
                signal.byte_order = Some(bo.clone());
            }
        }
    }

    let mut project = A2lProject {
        signals,
        xcp_data,
        ccp_data,
        compu_methods,
        default_byte_order,
    };

    // A virtual measurement borrows its input's address and datatype
    let rejected = virtual_signal::resolve_virtual_signals(&mut project);

    project.signals.retain(|s| !borrowed.contains(&s.name));

    // Keep only the conversion methods referenced by the kept signals
    let referenced: HashSet<String> = project
        .signals
        .iter()
        .filter_map(|s| s.conversion.as_ref().map(|c| c.identifier.clone()))
        .collect();
    project
        .compu_methods
        .retain(|name, _| referenced.contains(name.as_str()));

    (project, rejected)
}

// -------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::{
        decode_a2l_bytes, inspect, list_events, list_protocols, parse_a2l, parse_a2l_with_rejects,
        A2lParseError, A2lProject, Conversion, DaqMode, Protocol, RejectedVirtualReason,
    };
    const CCP_A2L: &[u8] = include_bytes!("../../data/CCP/SampleECU.a2l");

    /// A minimal file declaring both an XCP and a CCP transport, for protocol-detection tests.
    const DUAL_PROTOCOL_A2L: &str = r#"
      /begin IF_DATA ASAP1B_CCP
        /begin TP_BLOB
          0x0201 0x0206 0x00000455 0x00000456 0x0100 0x0002
        /end TP_BLOB
      /end IF_DATA
      /begin IF_DATA XCP
        /begin DAQ
          DYNAMIC
        /end DAQ
      /end IF_DATA
        "#;

    #[test]
    fn virtual_resolves_against_the_input_its_formula_names() {
        let content = r#"
        /begin MEASUREMENT First "unused input" UWORD First_CM 0 0 0 65535 ECU_ADDRESS 0x1000 /end MEASUREMENT
        /begin MEASUREMENT Second "the real input" UWORD Second_CM 0 0 0 65535 ECU_ADDRESS 0x2000 /end MEASUREMENT
        /begin MEASUREMENT Picked "second input only" FLOAT32_IEEE Picked_CM 0 0 0 100
          /begin VIRTUAL First Second /end VIRTUAL
        /end MEASUREMENT
        /begin COMPU_METHOD Second_CM "" RAT_FUNC "%.3" "V" COEFFS 0 4 0 0 0 1 /end COMPU_METHOD
        /begin COMPU_METHOD Picked_CM "" FORM "%.3" "V" /begin FORMULA "X2*10" /end FORMULA /end COMPU_METHOD
        "#;

        let (project, rejected) = parse_a2l_with_rejects(content, &["Picked".to_string()]).unwrap();
        assert!(rejected.is_empty(), "rejected: {rejected:?}");

        let picked = project
            .signals
            .iter()
            .find(|s| s.name == "Picked")
            .expect("Picked resolved");
        assert_eq!(picked.ecu_address, 0x2000, "borrows Second, not First");
        assert_eq!(picked.virtual_inputs, ["Second"], "First is dropped");

        let cm = project
            .compu_methods
            .get(&picked.conversion.as_ref().expect("conversion").identifier)
            .expect("resolved conversion method");
        assert_eq!((cm.factor, cm.offset), (2.5, 0.0));
    }

    #[test]
    fn virtual_whose_formula_names_a_missing_input_is_rejected() {
        let content = r#"
        /begin MEASUREMENT Only "the one input" UWORD Only_CM 0 0 0 65535 ECU_ADDRESS 0x1000 /end MEASUREMENT
        /begin MEASUREMENT Bad "names X2 with one input" FLOAT32_IEEE Bad_CM 0 0 0 100
          /begin VIRTUAL Only /end VIRTUAL
        /end MEASUREMENT
        /begin COMPU_METHOD Bad_CM "" FORM "%.3" "V" /begin FORMULA "X2*10" /end FORMULA /end COMPU_METHOD
        "#;

        let (_, rejected) = parse_a2l_with_rejects(content, &["Bad".to_string()]).unwrap();
        assert_eq!(
            rejected
                .iter()
                .map(|r| r.reason.clone())
                .collect::<Vec<_>>(),
            [RejectedVirtualReason::InputOutOfRange {
                index: 2,
                available: 1,
            }]
        );
    }

    #[test]
    fn a_measurement_annotated_with_line_comments_still_parses() {
        let content = r#"
        /begin MEASUREMENT Rpm "Engine speed"    // technical name, description
          UWORD Rpm_CM 0 0                       // datatype, conversion, resolution, accuracy
          0 8000                                 // lower limit, upper limit
          ECU_ADDRESS 0x1000
          // BIT_MASK 0x00FF  <- commented out, must not take effect
        /end MEASUREMENT
        "#;
        let project = parse_a2l(content, &[]).unwrap();
        assert_eq!(project.signals.len(), 1, "the comments must not hide it");
        assert_eq!(project.signals[0].ecu_address, 0x1000);
        assert_eq!(project.signals[0].upper_limit, 8000.0);
        assert_eq!(
            project.signals[0].bit_mask, None,
            "a commented-out keyword must not be parsed"
        );
    }

    #[test]
    fn event_channels_follow_the_protocol() {
        use super::event_channels;
        let content = r#"
        /begin IF_DATA XCP
          /begin DAQ
            DYNAMIC
            /begin EVENT "Task10ms" "10ms" 0x0000 DAQ 0x01 0x0A 0x06 0xFF /end EVENT
          /end DAQ
        /end IF_DATA
        /begin IF_DATA ASAP1B_CCP
          /begin RASTER "CCP_10ms List" "10ms" 0x0 0x4 0x1 /end RASTER
          /begin TP_BLOB 0x0201 0x0206 0x455 0x456 0x0100 0x0002 /end TP_BLOB
        /end IF_DATA
        "#;
        let project = parse_a2l(content, &[]).unwrap();

        let xcp: Vec<&str> = event_channels(&project, Protocol::Xcp)
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        let ccp: Vec<&str> = event_channels(&project, Protocol::Ccp)
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(xcp, ["Task10ms"]);
        assert_eq!(ccp, ["CCP_10ms List"], "CCP samples on rasters, not EVENTs");

        assert!(event_channels(&A2lProject::default(), Protocol::Ccp).is_empty());
    }

    #[test]
    fn a_ccp_only_file_still_reports_its_rasters() {
        use super::event_channels;
        let ccp_only = decode_a2l_bytes(CCP_A2L);
        let project = parse_a2l(&ccp_only, &[]).unwrap();
        assert!(project.xcp_data.is_none());
        let names: Vec<&str> = event_channels(&project, Protocol::Ccp)
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, ["Raster_10ms", "Raster_100ms"]);
    }

    #[test]
    fn detects_both_protocols_in_canonical_order() {
        let content = r#"
      /begin IF_DATA ASAP1B_CCP
        /begin TP_BLOB 0x201 /end TP_BLOB
      /end IF_DATA
      /begin IF_DATA XCP
        /begin DAQ DYNAMIC /end DAQ
      /end IF_DATA
        "#;
        assert_eq!(
            inspect(content).unwrap().protocols,
            [Protocol::Xcp, Protocol::Ccp]
        );
        assert_eq!(
            list_protocols(content).unwrap(),
            [Protocol::Xcp, Protocol::Ccp]
        );
    }

    #[test]
    fn detects_no_protocols_without_if_data() {
        let content = r#"
      /begin MEASUREMENT Engine_Speed "rpm"
        SWORD CM_N 0 0 0 6500
        ECU_ADDRESS 0xBB4
      /end MEASUREMENT
        "#;
        assert!(list_protocols(content).unwrap().is_empty());
    }

    #[test]
    fn detects_protocols_in_real_files() {
        let ccp_only = String::from_utf8_lossy(CCP_A2L);
        assert_eq!(
            inspect(DUAL_PROTOCOL_A2L).unwrap().protocols,
            [Protocol::Xcp, Protocol::Ccp]
        );
        assert_eq!(inspect(&ccp_only).unwrap().protocols, [Protocol::Ccp]);
    }

    #[test]
    fn parses_ccp_transport_and_rasters() {
        let content = r#"
      /begin MOD_COMMON ""
        BYTE_ORDER MSB_LAST
      /end MOD_COMMON
      /begin IF_DATA ASAP1B_CCP
        /begin RASTER
          "CCP_10ms List" "10ms" 0x0 0x4 0x1
        /end RASTER
        /begin RASTER
          "CCP_100ms List" "100ms" 0x1 0x5 0x1
        /end RASTER
        /begin TP_BLOB
          0x0201 0x0206 0x00000455 0x00000456 0x0100 0x0002
          BAUDRATE 500000
          DAQ_MODE BURST
          CONSISTENCY DAQ
          /begin CHECKSUM_PARAM 0x0003 0x000000FF /end CHECKSUM_PARAM
        /end TP_BLOB
      /end IF_DATA
        "#;
        let ccp = parse_a2l(content, &[]).unwrap().ccp_data.expect("ccp_data");
        assert_eq!(ccp.info.version, 0x0201);
        assert_eq!(ccp.info.cro_id, 0x455);
        assert_eq!(ccp.info.dto_id, 0x456);
        assert_eq!(ccp.info.station_address, 0x0100);
        assert_eq!(ccp.info.baudrate, Some(500_000));
        assert!(!ccp.info.bytes_only, "no BYTES_ONLY declared");
        assert_eq!(ccp.rasters.len(), 2);
        assert_eq!(ccp.rasters[0].name, "CCP_10ms List");
        assert_eq!(ccp.rasters[0].channel, 0);
        assert_eq!(ccp.rasters[1].name, "CCP_100ms List");
        assert_eq!(ccp.rasters[1].channel, 1);
    }

    #[test]
    fn parses_ccp_source_blocks() {
        let content = r#"
      /begin IF_DATA ASAP1B_CCP
        /begin SOURCE
          "CCP_10ms List" 4 1
          /begin QP_BLOB
            0
            LENGTH 30
            CAN_ID_FIXED 0x00000457
            RASTER 0 1
            FIRST_PID 0x12
          /end QP_BLOB
        /end SOURCE
        /begin SOURCE
          "no QP_BLOB here" 4 1
        /end SOURCE
        /begin TP_BLOB 0x0201 0x0206 0x455 0x456 0x0100 0x0002 /end TP_BLOB
      /end IF_DATA
        "#;
        let ccp = parse_a2l(content, &[]).unwrap().ccp_data.expect("ccp_data");
        assert_eq!(ccp.sources.len(), 1, "a SOURCE without QP_BLOB is skipped");
        let source = &ccp.sources[0];
        assert_eq!(source.name, "CCP_10ms List");
        assert_eq!(source.daq_list, 0);
        assert_eq!(source.rasters, [0, 1], "one RASTER tag, two values");
        assert_eq!(source.length, Some(30));
        assert_eq!(source.can_id, Some(0x457));
        assert_eq!(source.first_pid, Some(0x12));
    }

    #[test]
    fn parses_daq_list_first_pid() {
        let content = r#"
      /begin IF_DATA XCP
        /begin DAQ
          STATIC
          /begin DAQ_LIST
            0x1 MAX_ODT 0x5 MAX_ODT_ENTRIES 0x7 FIRST_PID 0x20 EVENT_FIXED 0x3
          /end DAQ_LIST
          /begin DAQ_LIST
            0x2 MAX_ODT 0x5
          /end DAQ_LIST
        /end DAQ
      /end IF_DATA
        "#;
        let project = parse_a2l(content, &[]).unwrap();
        let lists = &project.xcp_data.as_ref().unwrap().daq_info.daq_lists;
        assert_eq!(lists[0].first_pid, Some(0x20));
        assert_eq!(
            lists[0].event_fixed,
            Some(3),
            "FIRST_PID does not shadow it"
        );
        assert_eq!(lists[1].first_pid, None);
    }

    #[test]
    fn parses_ccp_file() {
        let ccp_only = String::from_utf8_lossy(CCP_A2L);
        let sample = parse_a2l(&ccp_only, &[]).unwrap();
        assert!(sample.xcp_data.is_none());
        let ccp = sample.ccp_data.expect("sample CCP transport");
        let names: Vec<&str> = ccp.rasters.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Raster_10ms", "Raster_100ms"]);
        assert_eq!(ccp.rasters[0].channel, 1);
        assert_eq!(ccp.rasters[1].channel, 2);
        assert!(ccp.info.bytes_only, "sample TP_BLOB declares BYTES_ONLY");
        let summary = inspect(&ccp_only).unwrap();
        assert_eq!(summary.events.len(), 2);
        assert_eq!(summary.events[0].name, "Raster_10ms");

        let both = parse_a2l(DUAL_PROTOCOL_A2L, &[]).unwrap();
        assert!(both.xcp_data.is_some());
        let ccp = both.ccp_data.expect("dual-protocol CCP transport");
        assert_eq!(ccp.info.cro_id, 0x455);
        assert_eq!(ccp.info.dto_id, 0x456);
        assert_eq!(ccp.info.station_address, 0x0100);
    }

    #[test]
    fn virtual_measurement_borrows_its_input() {
        let ccp_only = decode_a2l_bytes(CCP_A2L);
        let project = parse_a2l(&ccp_only, &["v_10".to_string()]).unwrap();
        assert_eq!(
            project.signals.len(),
            1,
            "the input is borrowed, not measured"
        );

        let v10 = &project.signals[0];
        assert_eq!(v10.name, "v_10");
        assert_eq!(v10.ecu_address, 0x3F000160, "address of v_10V_Raw");
        assert_eq!(v10.size, 2, "UWORD input, not the declared FLOAT32_IEEE");
        assert!(!v10.is_float);
        assert_eq!(v10.bit_mask, Some(0xFFFF));

        let cm = project
            .compu_methods
            .get(&v10.conversion.as_ref().expect("conversion").identifier)
            .expect("resolved conversion method");
        assert!((cm.factor - 15.0 / 16383.0).abs() < 1e-15, "X1*15/16383");
        assert_eq!(cm.offset, 0.0);
        assert_eq!(cm.unit, "volts", "unit from VIRTUAL_CONVERSION");

        let summary = inspect(&ccp_only).unwrap();
        let v5 = summary
            .signals
            .iter()
            .find(|s| s.name == "v_5")
            .expect("v_5 in summary");
        assert_eq!(v5.ecu_address, 0x3F000162);
        assert_eq!(v5.size, 2);
        assert_eq!(v5.unit, "volts");
        assert_eq!(v5.virtual_input.as_deref(), Some("v_5V_Raw"));
        assert!(v5.virtual_reject.is_none(), "v_5 resolved fine");
    }

    #[test]
    fn multi_input_virtuals_are_listed_as_unmeasurable() {
        let ccp_only = decode_a2l_bytes(CCP_A2L);
        let summary = inspect(&ccp_only).unwrap();

        let secs = summary
            .signals
            .iter()
            .find(|s| s.name == "RunTimeSeconds")
            .expect("RunTimeSeconds listed even though it cannot be measured");
        assert_eq!(
            secs.virtual_reject
                .as_ref()
                .map(RejectedVirtualReason::label),
            Some("multiple inputs")
        );

        let hours = summary
            .signals
            .iter()
            .find(|s| s.name == "RunHours")
            .expect("RunHours in summary");
        assert!(hours.virtual_reject.is_none());
        assert_ne!(hours.ecu_address, 0);

        let project = parse_a2l(&ccp_only, &["RunTimeSeconds".to_string()]).unwrap();
        assert!(project.signals.is_empty());
    }

    #[test]
    fn inspect_returns_signals_and_events_in_one_pass() {
        let content = r#"
      /begin MEASUREMENT Engine_Speed "rpm"
        SWORD CM_N 0 0 0 6500
        ECU_ADDRESS 0xBB4
      /end MEASUREMENT
      /begin MEASUREMENT Coolant_Temp "degC"
        SWORD CM_T 0 0 0 150
        ECU_ADDRESS 0xBB8
      /end MEASUREMENT
      /begin IF_DATA XCP
        /begin DAQ
          DYNAMIC 0x1 0x2 0x0
          /begin EVENT
            "5ms" "5ms" 0x0 DAQ 0xFF 0x5 0x6 0xFF
          /end EVENT
          /begin EVENT
            "extEvent" "extEvent" 0x1 DAQ 0xFF 0x1 0xA 0xFF
          /end EVENT
        /end DAQ
      /end IF_DATA
        "#;

        let summary = inspect(content).unwrap();
        let names: Vec<&str> = summary.signals.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Engine_Speed", "Coolant_Temp"]);
        assert_eq!(summary.signals[0].description, "rpm");
        assert_eq!(summary.signals[0].ecu_address, 0xBB4);
        assert_eq!(summary.events.len(), 2);
        assert_eq!(summary.events[0].name, "5ms");
        assert_eq!(summary.events[0].channel, 0);
        assert_eq!(summary.events[0].time_cycle, 5);
        assert_eq!(summary.events[1].name, "extEvent");
        assert_eq!(summary.events[1].channel, 1);

        assert_eq!(list_events(content).unwrap(), summary.events);
    }

    #[test]
    fn inspect_reports_measurement_groups() {
        let content = r#"
      /begin MEASUREMENT Engine_Speed "rpm"
        SWORD CM_N 0 0 0 6500
        ECU_ADDRESS 0xBB4
      /end MEASUREMENT
      /begin MEASUREMENT Coolant_Temp "degC"
        SWORD CM_T 0 0 0 150
        ECU_ADDRESS 0xBB8
      /end MEASUREMENT
      /begin GROUP Engine "Engine related measurements"
        ROOT
        /begin SUB_GROUP Engine_Sensors /end SUB_GROUP
      /end GROUP
      /begin GROUP Engine_Sensors "Engine sensor measurements"
        /begin REF_MEASUREMENT Engine_Speed Coolant_Temp /end REF_MEASUREMENT
      /end GROUP
        "#;

        let summary = inspect(content).unwrap();
        assert_eq!(summary.groups.len(), 2);
        assert!(summary.groups[0].root);
        assert_eq!(summary.groups[0].sub_groups, ["Engine_Sensors"]);

        let engine_speed = summary
            .signals
            .iter()
            .find(|s| s.name == "Engine_Speed")
            .expect("Engine_Speed in summary");
        assert_eq!(engine_speed.groups, ["Engine_Sensors"]);
    }

    #[test]
    fn inspect_rejects_empty_input() {
        assert_eq!(inspect("   "), Err(A2lParseError::EmptyInput));
        assert_eq!(list_events(""), Err(A2lParseError::EmptyInput));
    }

    #[test]
    fn parse_keeps_only_referenced_compu_methods() {
        let content = r#"
      /begin MEASUREMENT Engine_Speed "rpm"
        SWORD CM_N 0 0 0 6500
        ECU_ADDRESS 0xBB4
      /end MEASUREMENT
      /begin MEASUREMENT Coolant_Temp "degC"
        SWORD CM_T 0 0 0 150
        ECU_ADDRESS 0xBB8
      /end MEASUREMENT
      /begin COMPU_METHOD CM_N "engine speed conversion"
        RAT_FUNC "%5.0" "rpm"
        COEFFS 0 1 0 0 0 2
      /end COMPU_METHOD
      /begin COMPU_METHOD CM_T "coolant temp conversion"
        RAT_FUNC "%5.1" "degC"
        COEFFS 0 1 0 0 0 10
      /end COMPU_METHOD
      /begin COMPU_METHOD CM_UNUSED "not referenced by any signal"
        RAT_FUNC "%5.1" "V"
        COEFFS 0 1 0 0 0 1
      /end COMPU_METHOD
        "#;

        let project = parse_a2l(content, &["Engine_Speed".into()]).unwrap();
        assert_eq!(project.signals.len(), 1);
        assert!(project.compu_methods.contains_key("CM_N"));
        assert!(!project.compu_methods.contains_key("CM_T"));
        assert!(!project.compu_methods.contains_key("CM_UNUSED"));

        let project = parse_a2l(content, &[]).unwrap();
        assert_eq!(project.compu_methods.len(), 2);
        assert!(!project.compu_methods.contains_key("CM_UNUSED"));
    }

    #[test]
    fn tab_verb_method_resolves_compu_vtab() {
        let content = r#"
      /begin MEASUREMENT EngState_EngSt "engine state"
        UBYTE EngState_EngSt_Nr_CM_EngSt_frac 0 0 0 7
        ECU_ADDRESS 0x1000
      /end MEASUREMENT
      /begin COMPU_METHOD EngState_EngSt_Nr_CM_EngSt_frac
        ""
        TAB_VERB
        "%1.0"
        "-"
        COMPU_TAB_REF EngSt
      /end COMPU_METHOD
      /begin COMPU_VTAB 	EngSt
        "EngSt"
        TAB_VERB
        3
        0 "ENGST_DEFAULT"
        1 "ENGST_START"
        2 "ENGST_CUT_IN"
        DEFAULT_VALUE "UNKNOWN"
      /end COMPU_VTAB
        "#;

        let project = parse_a2l(content, &[]).unwrap();
        let cm = &project.compu_methods["EngState_EngSt_Nr_CM_EngSt_frac"];
        assert_eq!(cm.factor, 1.0);
        assert_eq!(cm.offset, 0.0);
        assert_eq!(cm.unit, "-");
        assert_eq!(cm.text_table.len(), 3);
        assert_eq!(cm.text_table[1].lower, 1.0);
        assert_eq!(cm.text_table[1].upper, 1.0);
        assert_eq!(cm.text_table[1].text, "ENGST_START");
        assert_eq!(cm.default_text.as_deref(), Some("UNKNOWN"));
    }

    #[test]
    fn tab_verb_method_resolves_compu_vtab_range() {
        let content = r#"
      /begin MEASUREMENT Gear "gear"
        UBYTE CM_Gear 0 0 0 255
        ECU_ADDRESS 0x1000
      /end MEASUREMENT
      /begin COMPU_METHOD CM_Gear
        "" TAB_VERB "%1.0" ""
        COMPU_TAB_REF Gear_Ranges
      /end COMPU_METHOD
      /begin COMPU_VTAB_RANGE Gear_Ranges
        "gear ranges"
        2
        0 0 "Neutral"
        1 6 "Forward"
        DEFAULT_VALUE "Invalid"
      /end COMPU_VTAB_RANGE
        "#;

        let project = parse_a2l(content, &[]).unwrap();
        let cm = &project.compu_methods["CM_Gear"];
        assert_eq!(cm.text_table.len(), 2);
        assert_eq!(cm.text_table[0].lower, 0.0);
        assert_eq!(cm.text_table[0].upper, 0.0);
        assert_eq!(cm.text_table[1].lower, 1.0);
        assert_eq!(cm.text_table[1].upper, 6.0);
        assert_eq!(cm.text_table[1].text, "Forward");
        assert_eq!(cm.default_text.as_deref(), Some("Invalid"));
    }

    #[test]
    fn rat_func_linear_coeffs_are_inverted_to_raw_to_physical() {
        let content = r#"
      /begin MEASUREMENT N "engine speed"
        UWORD CM_N 0 0 0 16000
        ECU_ADDRESS 0x1000
      /end MEASUREMENT
      /begin COMPU_METHOD CM_N
        "" RAT_FUNC "%5.2" "rpm"
        COEFFS 0 4 -8 0 0 1
      /end COMPU_METHOD
        "#;

        let project = parse_a2l(content, &[]).unwrap();
        let cm = &project.compu_methods["CM_N"];
        assert_eq!(cm.factor, 0.25);
        assert_eq!(cm.offset, 2.0);
        assert_eq!(cm.unit, "rpm");
    }

    #[test]
    fn parses_static_daq_mode_and_event_fixed() {
        let content = r#"
      /begin IF_DATA XCP
        /begin DAQ
          STATIC 0x1 0x1 0x0 OPTIMISATION_TYPE_DEFAULT
          /begin DAQ_LIST
            0x0
            DAQ_LIST_TYPE DAQ
            MAX_ODT 0x10
            MAX_ODT_ENTRIES 0x17
            EVENT_FIXED 0x0000
          /end DAQ_LIST
          /begin EVENT
            "One_Event_Per_Rotation" "Rev" 0x0 DAQ 0x1 0x0 0x6 0xFF
          /end EVENT
        /end DAQ
      /end IF_DATA
        "#;

        let project = parse_a2l(content, &[]).unwrap();
        let daq_info = &project.xcp_data.as_ref().unwrap().daq_info;
        assert_eq!(daq_info.mode, DaqMode::Static);
        assert!(daq_info.mode.is_static());
        assert_eq!(daq_info.daq_lists.len(), 1);
        assert_eq!(daq_info.daq_lists[0].nr_odt, 0x10);
        assert_eq!(daq_info.daq_lists[0].max_odt_entries, 0x17);
        assert_eq!(daq_info.daq_lists[0].event_fixed, Some(0));
    }

    #[test]
    fn parses_can_fd_block() {
        let content = r#"
      /begin IF_DATA XCP
        /begin XCP_ON_CAN
          0x0105
          CAN_ID_MASTER 0x0455
          CAN_ID_SLAVE 0x0456
          BAUDRATE 1000000
          /begin CAN_FD
            MAX_DLC 24
            CAN_FD_DATA_TRANSFER_BAUDRATE 2000000
            SAMPLE_POINT 80
            SJW 7
            SYNC_EDGE SINGLE
            TRANSCEIVER_DELAY_COMPENSATION OFF
          /end CAN_FD
          /begin DAQ_LIST_CAN_ID
            0x0000 FIXED 0x0456
          /end DAQ_LIST_CAN_ID
        /end XCP_ON_CAN
      /end IF_DATA
        "#;
        let project = parse_a2l(content, &[]).unwrap();
        let xcp = project.xcp_data.unwrap().xcp_info.unwrap();
        assert_eq!(xcp.can_id_slave, Some(0x456));
        assert_eq!(xcp.baudrate, Some(1_000_000));
        let fd = xcp.can_fd.expect("CAN_FD block should be parsed");
        assert_eq!(fd.max_dlc, 24);
        assert_eq!(fd.data_baudrate, Some(2_000_000));
    }

    #[test]
    fn no_can_fd_block_means_classic_can() {
        let content = r#"
      /begin IF_DATA XCP
        /begin XCP_ON_CAN
          0x0105
          CAN_ID_SLAVE 0x0456
          BAUDRATE 500000
        /end XCP_ON_CAN
      /end IF_DATA
        "#;
        let project = parse_a2l(content, &[]).unwrap();
        let xcp = project.xcp_data.unwrap().xcp_info.unwrap();
        assert_eq!(xcp.can_fd, None);
    }

    #[test]
    fn parses_timestamp_supported() {
        let content = r#"
      /begin IF_DATA XCP
        /begin DAQ
          STATIC
          /begin TIMESTAMP_SUPPORTED
            0x0001 SIZE_DWORD UNIT_1MS TIMESTAMP_FIXED
          /end TIMESTAMP_SUPPORTED
          /begin DAQ_LIST
            0x0000 MAX_ODT 0x10 MAX_ODT_ENTRIES 0x17 EVENT_FIXED 0x0000
          /end DAQ_LIST
        /end DAQ
      /end IF_DATA
        "#;
        let project = parse_a2l(content, &[]).unwrap();
        let daq = &project.xcp_data.unwrap().daq_info;
        assert_eq!(daq.timestamp_size, 4);
        assert!(daq.timestamp_fixed);
    }

    #[test]
    fn defaults_to_dynamic_mode_without_static_keyword() {
        let content = r#"
      /begin IF_DATA XCP
        /begin DAQ
          DYNAMIC 0x1 0x1 0x0
          /begin DAQ_LIST
            0x0
            MAX_ODT 0x5
          /end DAQ_LIST
        /end DAQ
      /end IF_DATA
        "#;
        let project = parse_a2l(content, &[]).unwrap();
        let daq_info = &project.xcp_data.as_ref().unwrap().daq_info;
        assert_eq!(daq_info.mode, DaqMode::Dynamic);
        assert_eq!(daq_info.daq_lists[0].event_fixed, None);
    }

    #[test]
    fn parses_selected_measurement_metadata() {
        let content = r#"
        /begin MEASUREMENT CDF20.MEASURE.N "speed "
            SWORD CM_N 0 0 0 6500
            BIT_MASK 0xFFFF
            ECU_ADDRESS 0xBB4
            FORMAT "%.4"
        /end MEASUREMENT

        /begin MEASUREMENT Air_AirMassNominalStroke_mg
            "Nominal air charge mass per stroke"
            FLOAT32_IEEE
            Air_AirMassNominalStroke_mg_CM_float32_t_mgPerstr
            0
            0
            0
            200
            ECU_ADDRESS 0x200036A4
            FORMAT "%4.1"
        /end MEASUREMENT

        /begin MEASUREMENT mpfl_MpropFreqMapValue
            "Engine speed to frequency conversion map output"
            FLOAT32_IEEE
            CM_8930
            1
            2.32831e-010
            -1e+010
            1e+010
            ECU_ADDRESS 0x1ffb3cb8
            /begin FUNCTION_LIST
                GLOBAL
            /end FUNCTION_LIST
        /end MEASUREMENT
            "#;

        let project = parse_a2l(content, &["Air_AirMassNominalStroke_mg".to_string()]).unwrap();
        assert_eq!(project.signals.len(), 1);
        assert_eq!(project.signals[0].name, "Air_AirMassNominalStroke_mg");
        assert_eq!(project.signals[0].ecu_address, 0x200036A4);
        assert_eq!(project.signals[0].size, 4);
        assert_eq!(
            project.signals[0].conversion,
            Some(Conversion {
                identifier: "Air_AirMassNominalStroke_mg_CM_float32_t_mgPerstr".to_string(),
            })
        );
        assert_eq!(project.signals[0].lower_limit, 0.0);
        assert_eq!(project.signals[0].upper_limit, 200.0);
    }

    #[test]
    fn rejects_empty_input() {
        let result = parse_a2l("", &[]);
        assert_eq!(result, Err(A2lParseError::EmptyInput));
    }

    #[test]
    fn parses_multiple_measurements() {
        let content = r#"
    /begin MEASUREMENT ASAM.M.SCALAR.UBYTE.IDENTICAL
      "Scalar measurement"
      UBYTE CM.IDENTICAL 0 0 0 255
      ECU_ADDRESS 0x13A00
      FORMAT "%5.0"    /* Note: Overwrites the format stated in the computation method */
      DISPLAY_IDENTIFIER DI.ASAM.M.SCALAR.UBYTE.IDENTICAL    /* optional display identifier */
      /begin IF_DATA ETK  KP_BLOB 0x13A00 INTERN 1 RASTER 2 /end IF_DATA
    /end MEASUREMENT

    /begin MEASUREMENT ASAM.M.SCALAR.UBYTE.IDENTICAL.STATUS_STRING
      "Scalar measurement with status string"
      UBYTE CM.LINEAR.IDENT.STATUS_STRING 0 0 0 252   /* restricted limits */
      ECU_ADDRESS 0x13A00
      FORMAT "%5.0"
      DISPLAY_IDENTIFIER DI.ASAM.M.SCALAR.UBYTE.IDENTICAL.STATUS_STRING
      /begin IF_DATA ETK  KP_BLOB 0x13A00 INTERN 1 RASTER 2 /end IF_DATA
    /end MEASUREMENT

    /begin MEASUREMENT ASAM.M.SCALAR.UBYTE.RAT_FUNC.IDENT.STATUS_STRING
      "Scalar measurement with status string"
      UBYTE CM.RAT_FUNC.IDENT.STATUS_STRING 0 0 0 252
      ECU_ADDRESS 0x13A00
      FORMAT "%5.0"
      DISPLAY_IDENTIFIER DI.ASAM.M.SCALAR.UBYTE.IDENTICAL.STATUS_STRING
      /begin IF_DATA ETK  KP_BLOB 0x13A00 INTERN 1 RASTER 4 /end IF_DATA
    /end MEASUREMENT
            "#;

        let project = parse_a2l(content, &[]).unwrap();
        assert_eq!(project.signals.len(), 3);
        assert_eq!(project.signals[0].name, "ASAM.M.SCALAR.UBYTE.IDENTICAL");
        assert_eq!(
            project.signals[1].name,
            "ASAM.M.SCALAR.UBYTE.IDENTICAL.STATUS_STRING"
        );
        assert_eq!(
            project.signals[2].name,
            "ASAM.M.SCALAR.UBYTE.RAT_FUNC.IDENT.STATUS_STRING"
        );
    }

    #[test]
    fn parses_if_data_xcp() {
        let content = r#"
      /begin IF_DATA XCP
        /begin PROTOCOL_LAYER
          0x100 0x20 0x20 0x20 0x20 0x20 0x20 0x20 0x8 0x8 BYTE_ORDER_MSB_LAST ADDRESS_GRANULARITY_BYTE
        /end PROTOCOL_LAYER
        /begin DAQ
          STATIC 0x3 0x2 0x0 OPTIMISATION_TYPE_DEFAULT ADDRESS_EXTENSION_DAQ IDENTIFICATION_FIELD_TYPE_ABSOLUTE GRANULARITY_ODT_ENTRY_SIZE_DAQ_BYTE 0x4 OVERLOAD_INDICATION_EVENT
          PRESCALER_SUPPORTED
          /begin TIMESTAMP_SUPPORTED
            0x1 SIZE_DWORD UNIT_1US
          /end TIMESTAMP_SUPPORTED
          /begin DAQ_LIST
            0x0
            DAQ_LIST_TYPE DAQ
            MAX_ODT 0x5
            MAX_ODT_ENTRIES 0x7
          /end DAQ_LIST
          /begin DAQ_LIST
            0x1
            DAQ_LIST_TYPE DAQ
            MAX_ODT 0x4
            MAX_ODT_ENTRIES 0x7
          /end DAQ_LIST
          /begin DAQ_LIST
            0x2
            DAQ_LIST_TYPE DAQ
            MAX_ODT 0x3
            MAX_ODT_ENTRIES 0x7
          /end DAQ_LIST
          /begin EVENT
            "5ms" "5ms" 0x0 DAQ 0xFF 0x5 0x6 0xFF
          /end EVENT
          /begin EVENT
            "extEvent" "extEvent" 0x1 DAQ 0xFF 0x1 0xA 0xFF
          /end EVENT
        /end DAQ
        /begin XCP_ON_CAN
          0x100
          CAN_ID_BROADCAST 0x52
          CAN_ID_MASTER 0x51
          CAN_ID_SLAVE 0x50
          BAUDRATE 0x7A120
        /end XCP_ON_CAN
      /end IF_DATA
        "#;

        let project = parse_a2l(content, &[]).unwrap();
        let xcp = project
            .xcp_data
            .as_ref()
            .expect("xcp_data should be present");

        // DAQ lists
        let lists = &xcp.daq_info.daq_lists;
        assert_eq!(lists.len(), 3);
        assert_eq!(lists[0].identifier, 0);
        assert_eq!(lists[0].nr_odt, 5);
        assert_eq!(lists[0].max_odt_entries, 7);
        assert_eq!(lists[1].identifier, 1);
        assert_eq!(lists[1].nr_odt, 4);
        assert_eq!(lists[1].max_odt_entries, 7);
        assert_eq!(lists[2].identifier, 2);
        assert_eq!(lists[2].nr_odt, 3);
        assert_eq!(lists[2].max_odt_entries, 7);
        assert_eq!(xcp.daq_info.max_odt_entry_size, 4);

        // Events
        let events = &xcp.event_info.events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name, "5ms");
        assert_eq!(events[0].channel, 0);
        assert_eq!(events[0].time_cycle, 5);
        assert_eq!(events[0].time_unit, 6);
        assert_eq!(events[1].name, "extEvent");
        assert_eq!(events[1].channel, 1);
        assert_eq!(events[1].time_cycle, 1);
        assert_eq!(events[1].time_unit, 0xA);

        // XCP_ON_CAN
        let can = xcp.xcp_info.as_ref().expect("xcp_info should be present");
        assert_eq!(can.version, 0x100);
        assert_eq!(can.can_id_broadcast, Some(0x52));
        assert_eq!(can.can_id_master, Some(0x51));
        assert_eq!(can.can_id_slave, Some(0x50));
        assert_eq!(can.baudrate, Some(0x7A120));
    }
}
