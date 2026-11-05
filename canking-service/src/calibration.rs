//! Calibrating the decoder's PID assignment from the slave's responses to the DAQ init sequence.

use protocol_core::protocol::ExpectedResponse;

use crate::bindings::canking::data_processor::types::LogSeverity;
use crate::decode::PidUpdate;
use crate::logging::log_msg;
use crate::state::{STATE, State};

/// Account for a command response, and calibrate the decoder from it.
pub(crate) fn consume_response(data: &[u8], positive: bool) {
    let mut state = STATE.write().unwrap();
    if !state.start_daq {
        return;
    }
    let proto = state.protocol;

    let Some(index) = state.answered_command(data) else {
        return;
    };
    state.record_answer(index);

    if !positive {
        return;
    }
    let Some(frame) = state.init_frames.get(index) else {
        return;
    };
    let ExpectedResponse::FirstPid { daq_index } = frame.response else {
        return;
    };
    let label = frame.label.clone();

    let report = proto.daq_list_report(data);
    let odts = state.decoder.as_ref().and_then(|d| d.odt_count(daq_index));

    if let (Some(reported), Some(configured)) = (report.odt_count, odts)
        && (reported as usize) < configured
    {
        log_msg(
            format!(
                "{label} reports {reported} ODT(s) but the configuration \
                     fills {configured}; the surplus will not be measured"
            ),
            LogSeverity::Warning,
        );
    }

    let Some(first_pid) = report.first_pid else {
        return;
    };

    // PID cannot be assigned outside the range the protocol allows.
    let last_pid = first_pid as usize + odts.unwrap_or(1).saturating_sub(1);
    if last_pid > proto.max_daq_pid() as usize {
        log_msg(
            format!(
                "{label} reports a PID range 0x{first_pid:02X}-0x{last_pid:02X} that runs past \
                 the highest PID {} allows (0x{:02X}); ignoring it and keeping the configured \
                 assignment",
                proto.name(),
                proto.max_daq_pid()
            ),
            LogSeverity::Warning,
        );
        return;
    }

    match state.apply_first_pid(daq_index, first_pid) {
        PidUpdate::Applied { previous } => {
            let id = state
                .decoder
                .as_ref()
                .and_then(|d| d.list_pid(daq_index))
                .map(|(id, _)| id.to_string())
                .unwrap_or_else(|| daq_index.to_string());
            log_msg(
                format!(
                    "DAQ list {id} starts at PID 0x{first_pid:02X}, not the \
                     configured 0x{previous:02X}; decoder re-keyed to match the ECU"
                ),
                LogSeverity::Information,
            );
        }
        PidUpdate::Unchanged | PidUpdate::NoSuchList => {}
    }

    // The last init command has been answered
    if state.unanswered() == 0 {
        warn_about_pid_conflicts(&state);
    }
}

/// Warn about DAQ lists left decoding under overlapping PID ranges.
fn warn_about_pid_conflicts(state: &State) {
    let Some(conflicts) = state.decoder.as_ref().map(|d| d.pid_conflicts()) else {
        return;
    };
    if conflicts.is_empty() {
        return;
    }
    let listing = conflicts
        .iter()
        .map(|(a, b)| format!("{a} and {b}"))
        .collect::<Vec<_>>()
        .join(", ");
    log_msg(
        format!(
            "DAQ lists {listing} decode under overlapping PIDs; their values will be \
             unreliable. The slave's first-PID reports were most likely misattributed"
        ),
        LogSeverity::Warning,
    );
}
