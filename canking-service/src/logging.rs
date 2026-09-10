//! Logging

use crate::bindings::canking::data_processor::canking::log;
use crate::bindings::canking::data_processor::types::LogSeverity;

/// Log a message tagged with the extension's name.
pub(crate) fn log_msg(msg: impl std::fmt::Display, severity: LogSeverity) {
    log(&format!("DAQ Decoder: {msg}"), severity);
}
