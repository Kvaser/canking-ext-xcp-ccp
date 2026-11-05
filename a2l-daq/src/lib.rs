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
    CoreOutput, Diagnostic, apply_assignments, apply_can_id_overrides, build, dropped_signals,
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
