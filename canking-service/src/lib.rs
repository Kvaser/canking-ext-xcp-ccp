//! Wasm component implementation of the CanKing `data-processor`

#![warn(unreachable_pub)]

// Generated bindings are `pub` inside a private module by construction.
#[allow(unreachable_pub)]
mod bindings {
    wit_bindgen::generate!({
        path: "wit/canking-data-processor@1.2.0.wit",
    });

    use super::DaqComponent;
    export!(DaqComponent);
}

mod calibration;
mod channels;
mod config;
mod decode;
mod definitions;
mod logging;
mod state;

use crate::calibration::consume_response;
use crate::channels::validate_channel;
use crate::config::{apply_config, Config, ConfigError};
use crate::decode::Decoder;
use crate::definitions::{log_frame_definitions, PID_SIGNAL_NAME};
use crate::logging::log_msg;
use crate::state::STATE;
use protocol_core::protocol::{self, ResponseType};

use bindings::{
    canking::data_processor::{
        canking::{log, queue_out_frames, send_frame, set_error, set_frame_signal_values},
        types::{
            CankingSetupInfo, CodeMaskFilter, DataSubscription, Direction, FrameDefinition,
            FrameIdFilter, FramesSubscription, LogSeverity, ProcessFrame, SignalValue,
        },
    },
    exports::canking::data_processor::extension_base::ExtensionInfo,
};
const INIT_FRAME_SEPARATION_MS: u32 = 20;
const INIT_FIRST_FRAME_DELAY_MS: u32 = 1000;

struct DaqComponent;

impl bindings::exports::canking::data_processor::extension_base::Guest for DaqComponent {
    fn get_extension_info() -> ExtensionInfo {
        let default_config =
            serde_json::to_string_pretty(&Config::sample()).unwrap_or_else(|_| "{}".to_string());
        ExtensionInfo {
            display_name: "DAQ Decoder".to_string(),
            description: env!("CARGO_PKG_DESCRIPTION").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: env!("CARGO_PKG_AUTHORS").to_string(),
            default_config,
        }
    }

    fn on_config_change(new_config: String) {
        let result = serde_json::from_str::<Config>(&new_config)
            .map_err(ConfigError::InvalidJson)
            .and_then(|cfg| apply_config(&cfg));

        let applied = match result {
            Ok(applied) => applied,
            Err(e) => {
                log(&e.to_string(), LogSeverity::Error);
                set_error(&e.to_string());
                return;
            }
        };

        if applied.start_daq && applied.command_tx_id.is_none() {
            log_msg(
                "start_daq set but config has no command CAN id \
                 (DAQ master / CCP CRO); will decode frames but not start DAQ",
                LogSeverity::Warning,
            );
        }

        STATE.write().unwrap().apply_config(applied);
        log_msg("config applied", LogSeverity::Debug);
    }

    fn on_setup_change(new_setup: CankingSetupInfo) {
        if new_setup.signals.is_empty() {
            log_msg("setup change: no signals in setup", LogSeverity::Debug);
        } else {
            let listing = new_setup
                .signals
                .iter()
                .map(|s| {
                    format!(
                        "handle {} = '{}' (channel {})",
                        s.signal_handle, s.qualified_name, s.channel_number
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            log_msg(
                format!(
                    "setup change: {} signal(s): {listing}",
                    new_setup.signals.len()
                ),
                LogSeverity::Debug,
            );
        }

        let mut state = STATE.write().unwrap();
        let channel = state.note_available_channel(new_setup.channels);
        if let Some(ch) = &channel {
            log_msg(
                format!(
                    "using channel {} ('{}') for DAQ commands",
                    ch.channel_number, ch.name
                ),
                LogSeverity::Debug,
            );
        } else {
            log_msg(
                "no channel available to send DAQ commands",
                LogSeverity::Warning,
            );
        }
    }

    fn on_start_measurement() {
        let mut state = STATE.write().unwrap();
        state.reset_measurement();

        if let Some(corrections) = state.decoder.as_ref().map(Decoder::corrections) {
            if !corrections.is_empty() {
                let listing = corrections
                    .iter()
                    .map(|(id, configured, in_force)| {
                        format!("list {id}: 0x{configured:02X} -> 0x{in_force:02X}")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                log_msg(
                    format!("decoding with the PIDs learned from the ECU ({listing})"),
                    LogSeverity::Information,
                );
            }
        }

        match state.commit_output_channel() {
            Some(ch) => validate_channel(state.expect_fd, &ch),
            None => {
                if state.start_daq {
                    log_msg(
                        "no channel available to send DAQ commands; \
                         DAQ will not be started",
                        LogSeverity::Warning,
                    );
                }
            }
        }

        // Queue the DAQ init/start sequence
        if let Some((channel, frames)) = state.frames_to_queue_on_start() {
            log_msg(
                format!("queueing {} DAQ init frames", frames.len()),
                LogSeverity::Information,
            );
            queue_out_frames(
                channel,
                &frames,
                INIT_FRAME_SEPARATION_MS,
                INIT_FIRST_FRAME_DELAY_MS,
            );
        }
        log_msg("measurement started", LogSeverity::Debug);
    }

    fn on_stop_measurement() {
        let mut state = STATE.write().unwrap();

        let missing = state.unanswered();
        if state.start_daq && missing > 0 && !state.init_frames.is_empty() {
            let consequence = if state.protocol.command_counter().is_some() {
                "those commands went unacknowledged"
            } else {
                "those commands went unacknowledged, and any response after the first \
                 missing one was attributed to the wrong command"
            };
            log_msg(
                format!(
                    "{missing} of {} DAQ init commands were not answered; {consequence}",
                    state.init_frames.len()
                ),
                LogSeverity::Warning,
            );
        }

        // Send the DAQ stop sequence.
        if state.start_daq && !state.stop_frames.is_empty() && state.output_channel.is_none() {
            log_msg(
                "no channel available to send DAQ stop frames",
                LogSeverity::Warning,
            );
        }
        if let Some((channel, frames)) = state.frames_to_send_on_stop() {
            log_msg(
                format!("sending {} DAQ stop frames", frames.len()),
                LogSeverity::Information,
            );
            for frame in frames {
                send_frame(channel, &frame);
            }
        }

        state.reset_measurement();
    }

    fn on_command(command_id: String, _command_args: String) -> Result<String, String> {
        //TODO
        log(
            &format!("on_command: {command_id} (args ignored)"),
            LogSeverity::Debug,
        );
        Ok(String::new())
    }
}

impl bindings::Guest for DaqComponent {
    fn get_subscription() -> Option<DataSubscription> {
        let state = STATE.read().unwrap();

        let decoder = state.decoder.as_ref()?;
        let id_filters = decoder
            .subscription_can_ids()
            .into_iter()
            .map(|code| FrameIdFilter {
                protocol: None,
                is_extended_id: None,
                direction: Some(Direction::Rx),
                code_mask_filter: Some(CodeMaskFilter {
                    code,
                    mask: 0x1FFF_FFFF,
                }),
            })
            .collect();

        Some(DataSubscription {
            frames: Some(FramesSubscription {
                include_error_frames: false,
                id_filters,
            }),
            signals: None,
        })
    }

    fn get_frame_property_names() -> Vec<String> {
        Vec::new()
    }

    // Define signals
    fn get_frame_definitions() -> Vec<FrameDefinition> {
        let defs = STATE.read().unwrap().frame_definitions().to_vec();
        log_msg(
            format!(
                "get_frame_definitions -> {} frame(s), {} signal(s)",
                defs.len(),
                defs.iter().map(|d| d.signals.len()).sum::<usize>()
            ),
            LogSeverity::Debug,
        );
        log_frame_definitions(&defs);
        defs
    }

    fn on_frames(frames: Vec<ProcessFrame>) {
        let mut decoded_signals = 0usize;
        let mut saw_command_response = false;
        let proto = STATE.read().unwrap().protocol;

        for frame in &frames {
            let (handle, id, data) = match frame {
                ProcessFrame::CanFrame(f) => (f.frame_handle, f.frame_data.id, &f.frame_data.data),
                ProcessFrame::CanFdFrame(f) => {
                    (f.frame_handle, f.frame_data.id, &f.frame_data.data)
                }
                _ => continue,
            };

            match protocol::classify_response(proto, data) {
                ResponseType::Error { code } => {
                    let optional = STATE.read().unwrap().pending_command_is_optional(data);
                    if optional {
                        log(
                            &format!(
                                "{} error response: 0x{code:02X} (optional command, not supported by this ECU)",
                                proto.name()
                            ),
                            LogSeverity::Debug,
                        );
                    } else {
                        log(
                            &format!("{} error response: 0x{code:02X}", proto.name()),
                            LogSeverity::Warning,
                        );
                    }
                    consume_response(data, false);
                    saw_command_response = true;
                    continue;
                }
                ResponseType::Positive => {
                    consume_response(data, true);
                    saw_command_response = true;
                    continue;
                }
                ResponseType::Unrelated => {}
            };

            // Decode using only a read lock
            let decoded = {
                let state = STATE.read().unwrap();

                let Some(decoder) = state.decoder.as_ref() else {
                    continue;
                };

                decoder.decode(id, data)
            };

            if decoded.is_empty() {
                continue;
            }

            // `decoded` only comes back non-empty for a payload `Decoder::decode` found a PID byte in.
            let Some(&pid_byte) = data.first() else {
                log_msg(
                    format!(
                        "on_frames: decoded {} signal(s) from an empty payload (frame {handle})",
                        decoded.len()
                    ),
                    LogSeverity::Warning,
                );
                continue;
            };

            decoded_signals += decoded.len();

            let mut signal_values: Vec<_> = decoded
                .iter()
                .map(
                    |d| bindings::canking::data_processor::types::FrameSignalValue {
                        name: d.name.to_string(),
                        value: d.value,
                    },
                )
                .collect();
            signal_values.push(bindings::canking::data_processor::types::FrameSignalValue {
                name: PID_SIGNAL_NAME.to_string(),
                value: pid_byte as f64,
            });

            set_frame_signal_values(handle, &signal_values);
        }
        if saw_command_response {
            return;
        }

        if decoded_signals == 0 {
            log_msg(
                format!(
                    "on_frames got {} frame(s) but decoded 0 signals",
                    frames.len()
                ),
                LogSeverity::Debug,
            );
        }
    }

    fn on_signals(_signals: Vec<SignalValue>) {}
}
