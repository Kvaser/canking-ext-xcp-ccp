//! Locked state for the service extension

use std::sync::{LazyLock, RwLock};

use crate::bindings::canking::data_processor::types::{
    CanFdFrameInfo, CanFdOutFrame, CanFrameInfo, CanOutFrame, ChannelInfo, FrameData,
    FrameDefinition, OutFrame,
};
use crate::channels::choose_channel;
use crate::config::AppliedConfig;
use crate::decode::{Decoder, PidUpdate, SignalMeta};
use crate::definitions;
use protocol_core::protocol::{ExpectedResponse, Protocol as DaqProtocol};

/// One ready-to-send DAQ init command frame together with what its response is expected to carry.
pub(crate) struct CommandFrame {
    pub(crate) id: u32,
    pub(crate) data: Vec<u8>,
    pub(crate) is_extended_id: bool,
    pub(crate) response: ExpectedResponse,
    pub(crate) label: String,
    /// Whether an unsupported/negative response to this command is expected and tolerated.
    pub(crate) optional: bool,
}

/// Everything derived from the current config, plus the progress of the measurement currently running.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) protocol: DaqProtocol,
    pub(crate) decoder: Option<Decoder>,
    pub(crate) init_frames: Vec<CommandFrame>,
    pub(crate) stop_frames: Vec<CommandFrame>,
    pub(crate) output_channel: Option<i32>,
    pub(crate) start_daq: bool,
    pub(crate) expect_fd: bool,
    pub(crate) fd_brs: bool,
    pub(crate) channels: Vec<ChannelInfo>,

    // Private measurement fields
    /// The decoder's signal metadata
    signal_metadata: Vec<SignalMeta>,
    frame_definitions: Vec<FrameDefinition>,
    answered: usize,
}

impl State {
    pub(crate) fn apply_config(&mut self, applied: AppliedConfig) {
        self.protocol = applied.protocol;
        self.expect_fd = applied.expect_fd;
        self.fd_brs = applied.fd_brs;
        self.set_decoder(applied.decoder, applied.signal_metadata);
        self.init_frames = applied.init_frames;
        self.stop_frames = applied.stop_frames;
        self.reset_measurement();
        self.start_daq = applied.start_daq;
    }

    /// Sets the decoder and its frame definitions. `expect_fd`/`fd_brs` must already reflect the
    /// config this decoder was built from.
    pub(crate) fn set_decoder(&mut self, decoder: Decoder, metadata: Vec<SignalMeta>) {
        self.signal_metadata = metadata;
        self.decoder = Some(decoder);
        self.refresh_frame_definitions();
    }

    pub(crate) fn frame_definitions(&self) -> &[FrameDefinition] {
        &self.frame_definitions
    }

    /// Apply a slave's first-PID report to one DAQ list, keeping the metadata and frame
    /// definitions in step with the decoder.
    pub(crate) fn apply_first_pid(&mut self, daq_index: usize, first_pid: u8) -> PidUpdate {
        let Some(decoder) = self.decoder.as_mut() else {
            return PidUpdate::NoSuchList;
        };
        let update = decoder.set_first_pid(daq_index, first_pid);
        if matches!(update, PidUpdate::Applied { .. }) {
            self.decoder
                .as_ref()
                .expect("decoder was just set")
                .restamp_pids(&mut self.signal_metadata);
            self.refresh_frame_definitions();
        }
        update
    }

    /// Rebuild `frame_definitions` from the current decoder and signal metadata.
    fn refresh_frame_definitions(&mut self) {
        let decoder = self
            .decoder
            .as_ref()
            .expect("decoder must be set before refreshing frame definitions");
        self.frame_definitions = definitions::build_frame_definitions(
            decoder,
            &self.signal_metadata,
            self.expect_fd,
            self.fd_brs,
        );
    }

    /// Record the channels reported at setup time and pick one for DAQ commands.
    pub(crate) fn note_available_channel(
        &mut self,
        channels: Vec<ChannelInfo>,
    ) -> Option<ChannelInfo> {
        self.channels = channels;
        self.choose_and_store_channel()
    }

    /// Re-pick the channel to send DAQ commands on, for the caller to validate before starting.
    pub(crate) fn commit_output_channel(&mut self) -> Option<ChannelInfo> {
        self.choose_and_store_channel()
    }

    fn choose_and_store_channel(&mut self) -> Option<ChannelInfo> {
        let channel = choose_channel(&self.channels, self.expect_fd);
        self.output_channel = channel.as_ref().map(|c| c.channel_number);
        channel
    }

    /// The DAQ init frames to queue when starting a measurement, and the channel to send them
    /// on. `None` unless DAQ start is configured, there are init frames, and a channel is set.
    pub(crate) fn frames_to_queue_on_start(&self) -> Option<(i32, Vec<OutFrame>)> {
        if !self.start_daq || self.init_frames.is_empty() {
            return None;
        }
        let channel = self.output_channel?;
        Some((channel, self.out_frames(&self.init_frames)))
    }

    /// The DAQ stop frames to send when stopping a measurement, and the channel to send them on. `None` unless DAQ start is
    /// configured, there are stop frames, and a channel is set.
    pub(crate) fn frames_to_send_on_stop(&self) -> Option<(i32, Vec<OutFrame>)> {
        if !self.start_daq || self.stop_frames.is_empty() {
            return None;
        }
        let channel = self.output_channel?;
        Some((channel, self.out_frames(&self.stop_frames)))
    }

    /// Forget everything gathered during a measurement.
    pub(crate) fn reset_measurement(&mut self) {
        self.answered = 0;
    }

    /// Record that the init command at `index` has been answered.
    pub(crate) fn record_answer(&mut self, index: usize) {
        self.answered = index + 1;
    }

    /// How many init commands the slave has not answered.
    pub(crate) fn unanswered(&self) -> usize {
        self.init_frames.len() - self.answered
    }

    /// The init command an incoming response answers.
    /// For protocols without a command counter, the init commands are answered in order.
    pub(crate) fn answered_command(&self, data: &[u8]) -> Option<usize> {
        match self.protocol.command_counter() {
            Some(counter) => {
                let echoed = data.get(counter.in_response).copied()?;
                self.init_frames
                    .iter()
                    .enumerate()
                    .skip(self.answered)
                    .find(|(_, f)| f.data.get(counter.in_command) == Some(&echoed))
                    .map(|(index, _)| index)
            }
            None => (self.answered < self.init_frames.len()).then_some(self.answered),
        }
    }

    /// Whether the init command `data` answers is marked optional
    /// meaning an error response to it is expected and should not be logged as a failure.
    pub(crate) fn pending_command_is_optional(&self, data: &[u8]) -> bool {
        self.answered_command(data)
            .and_then(|i| self.init_frames.get(i))
            .is_some_and(|f| f.optional)
    }

    /// Command frames as host out-frames, CAN or CAN FD depending on the A2L transport.
    fn out_frames(&self, frames: &[CommandFrame]) -> Vec<OutFrame> {
        frames
            .iter()
            .map(|f| {
                let frame_data = FrameData {
                    id: f.id,
                    data_len: f.data.len() as u16,
                    data: f.data.clone(),
                };
                if self.expect_fd {
                    OutFrame::CanFdFrame(CanFdOutFrame {
                        frame_data,
                        can_fd_info: CanFdFrameInfo {
                            is_extended_id: f.is_extended_id,
                            is_brs: self.fd_brs,
                            is_esi: false,
                        },
                    })
                } else {
                    OutFrame::CanFrame(CanOutFrame {
                        frame_data,
                        can_info: CanFrameInfo {
                            is_extended_id: f.is_extended_id,
                            is_rtr: false,
                        },
                    })
                }
            })
            .collect()
    }
}

pub(crate) static STATE: LazyLock<RwLock<State>> = LazyLock::new(|| RwLock::new(State::default()));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::canking::data_processor::types::Protocol as ChannelProtocol;

    fn channel(number: i32, protocol: ChannelProtocol) -> ChannelInfo {
        ChannelInfo {
            name: format!("ch{number}"),
            id: format!("id{number}"),
            channel_number: number,
            is_silent: false,
            protocol,
        }
    }

    fn command_frame() -> CommandFrame {
        CommandFrame {
            id: 0x100,
            data: vec![0x01],
            is_extended_id: false,
            response: ExpectedResponse::Ack,
            label: "test".to_string(),
            optional: false,
        }
    }

    #[test]
    fn pending_command_is_optional_reflects_the_matched_frame() {
        let mut state = State {
            init_frames: vec![
                CommandFrame {
                    optional: false,
                    ..command_frame()
                },
                CommandFrame {
                    optional: true,
                    ..command_frame()
                },
            ],
            ..State::default()
        };

        assert!(
            !state.pending_command_is_optional(&[0xFE]),
            "first command is mandatory"
        );
        state.record_answer(0);
        assert!(
            state.pending_command_is_optional(&[0xFE]),
            "second command is optional"
        );
    }

    #[test]
    fn note_available_channel_picks_by_protocol_without_validating() {
        let mut state = State::default();
        let channels = vec![
            channel(1, ChannelProtocol::Lin),
            channel(2, ChannelProtocol::Can),
        ];

        let chosen = state.note_available_channel(channels);

        assert_eq!(chosen.map(|c| c.channel_number), Some(2));
        assert_eq!(state.output_channel, Some(2));
    }

    #[test]
    fn note_available_channel_is_none_when_nothing_matches() {
        let mut state = State::default();

        assert!(state.note_available_channel(Vec::new()).is_none());
        assert!(state.output_channel.is_none());
    }

    #[test]
    fn commit_output_channel_reselects_against_known_channels() {
        let mut state = State {
            channels: vec![channel(3, ChannelProtocol::Can)],
            ..State::default()
        };

        let chosen = state.commit_output_channel();

        assert_eq!(chosen.map(|c| c.channel_number), Some(3));
        assert_eq!(state.output_channel, Some(3));
    }

    #[test]
    fn frames_to_queue_on_start_requires_start_daq_frames_and_channel() {
        let mut state = State {
            init_frames: vec![command_frame()],
            output_channel: Some(7),
            ..State::default()
        };
        assert!(
            state.frames_to_queue_on_start().is_none(),
            "start_daq is false"
        );

        state.start_daq = true;
        let (channel, frames) = state.frames_to_queue_on_start().expect("ready to queue");
        assert_eq!(channel, 7);
        assert_eq!(frames.len(), 1);

        state.init_frames.clear();
        assert!(state.frames_to_queue_on_start().is_none(), "no init frames");

        state.init_frames = vec![command_frame()];
        state.output_channel = None;
        assert!(
            state.frames_to_queue_on_start().is_none(),
            "no channel committed"
        );
    }

    #[test]
    fn frames_to_send_on_stop_requires_start_daq_frames_and_channel() {
        let mut state = State {
            stop_frames: vec![command_frame()],
            output_channel: Some(7),
            ..State::default()
        };
        assert!(
            state.frames_to_send_on_stop().is_none(),
            "start_daq is false"
        );

        state.start_daq = true;
        let (channel, frames) = state.frames_to_send_on_stop().expect("ready to send");
        assert_eq!(channel, 7);
        assert_eq!(frames.len(), 1);

        state.stop_frames.clear();
        assert!(state.frames_to_send_on_stop().is_none(), "no stop frames");

        state.stop_frames = vec![command_frame()];
        state.output_channel = None;
        assert!(
            state.frames_to_send_on_stop().is_none(),
            "no channel committed"
        );
    }
}
