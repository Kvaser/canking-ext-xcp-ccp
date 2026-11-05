//! A2L in, DAQ setup out: parses the measurement metadata of an A2L file, packs the selected signals into DAQ
//! lists, and renders a DBC, the data processor's JSON config and a Memorator t-script from them.
//!
//! The data model, the protocol commands and the frame layout live in `protocol-core`, shared with the data processor.
//! This crate adds the parser ([`a2l`]), the packer ([`daq`]), and the generators ([`dbc`], `tscript`).
//! [`pipeline`] chains them the way every host (CLI, wasm component, tests) does:
//!
//! ```
//! let a2l = r#"
//! /begin MEASUREMENT Rpm "Engine speed" UWORD NO_COMPU_METHOD 0 0 0 8000 ECU_ADDRESS 0x1000 /end MEASUREMENT
//! /begin IF_DATA XCP
//!   /begin XCP_ON_CAN 0x0105 CAN_ID_MASTER 0x0455 CAN_ID_SLAVE 0x0456 BAUDRATE 500000 /end XCP_ON_CAN
//!   /begin DAQ DYNAMIC
//!     /begin DAQ_LIST 0x0000 MAX_ODT 0x05 MAX_ODT_ENTRIES 0x07 /end DAQ_LIST
//!     /begin EVENT "Task10ms" "10ms" 0x0000 DAQ 0x01 0x0A 0x06 0xFF /end EVENT
//!   /end DAQ
//! /end IF_DATA
//! "#;
//! let selected = ["Rpm".to_string()];
//! let mut project = a2l_daq::parse_a2l(a2l, &selected).unwrap();
//! let assignments = [("Task10ms".to_string(), selected.to_vec())];
//! let out = a2l_daq::build(&mut project, &assignments, a2l_daq::Protocol::Xcp, "ecu.a2l").unwrap();
//!
//! assert_eq!(out.daq_lists.len(), 1);
//! assert!(out.dbc.contains("BO_ 1110 DAQ"), "one DAQ message on CAN_ID_SLAVE 0x456");
//! assert!(out.warnings.is_empty());
//! ```

pub mod a2l;
pub mod daq;
pub mod dbc;
pub mod pipeline;
pub mod traceability;

// Core library
pub use protocol_core::layout;
pub use protocol_core::protocol;
pub use protocol_core::protocol::Protocol;
pub use protocol_core::types::{
    A2lProject, ByteOrder, CanFdInfo, CanIdSource, CcpData, CcpInfo, CcpSource, CompuMethod,
    Conversion, DaqInfo, DaqList, DaqListDef, DaqMode, EventEntry, EventInfo, Odt, OdtEntry,
    ParsedSignal, ProtocolLayerInfo, VTabEntry, XcpData, XcpInfo,
};
pub use protocol_core::xcp;

// Re-exports
pub use a2l::{
    A2lParseError, A2lSummary, GroupInfo, RejectedVirtual, RejectedVirtualReason, SignalSummary,
    TransportInfo, UnreadKeyword, decode_a2l_bytes, event_channels, inspect, list_events,
    list_protocols, list_signal_names, parse_a2l, parse_a2l_with_rejects, unread_keywords,
};
pub use daq::build_daq_list;
pub use dbc::{DbcError, create_dbc_file, sanitize_name};
pub use pipeline::{
    CoreOutput, Diagnostic, apply_assignments, apply_can_id_overrides, apply_prescalers, build,
    dropped_signals,
};
pub use traceability::traceability_lines;
pub use xcp::{XcpCommand, XcpPhase, XcpResponse, build_init_commands};

// Data processor configuration (shared JSON contract, lives in protocol-core)
#[cfg(feature = "serde")]
pub use protocol_core::config::{self, Config};

// Wasm only
#[cfg(all(feature = "component", target_arch = "wasm32"))]
mod component;

// T-script generation
#[cfg(feature = "tscript")]
pub mod tscript;

#[cfg(feature = "tscript")]
pub use tscript::{
    Accessor, BusDefaults, CanChannel, CompareOp, Comparison, DEFAULT_BITRATE,
    DEFAULT_FD_DATA_BITRATE, DEFAULT_INIT_TIMEOUT_MS, DaqInit, DecoderType, MessageRef,
    ResolvedTrigger, SignalDecoder, SignalHook, TScriptConfig, TriggerAction, TriggerCondition,
    TriggerError, TriggerExprError, TriggerParts, TriggerSignal, TriggerTerm, build_daq_init,
    build_trigger, bus_defaults, dbc_trigger_signals, decoder_functions, decoders_for,
    expand_signal_indices, generate_tscript, helper_typedef, inline_trigger_signals, parse_terms,
    resolve_signal_hooks, resolve_terms, resolve_trigger,
};
