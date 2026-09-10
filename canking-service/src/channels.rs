//! Picking and validating which CAN channel to send DAQ init commands on.

use crate::bindings::canking::data_processor::types::{ChannelInfo, LogSeverity, Protocol};
use crate::logging::log_msg;

/// The channel to send DAQ init commands on: the first non-silent channel
/// matching the A2L transport, else the first non-silent CAN/CAN-FD channel,
/// else the found channel.
pub(crate) fn choose_channel(channels: &[ChannelInfo], want_fd: bool) -> Option<ChannelInfo> {
    let wanted = if want_fd {
        Protocol::CanFd
    } else {
        Protocol::Can
    };
    channels
        .iter()
        .find(|c| !c.is_silent && c.protocol == wanted)
        .or_else(|| {
            channels
                .iter()
                .find(|c| !c.is_silent && matches!(c.protocol, Protocol::Can | Protocol::CanFd))
        })
        .or_else(|| channels.first())
        .cloned()
}

/// Warn about a chosen channel that can't actually carry DAQ traffic.
pub(crate) fn validate_channel(expect_fd: bool, channel: &ChannelInfo) {
    if channel.is_silent {
        log_msg(
            format!(
                "channel '{}' is silent; Init commands cannot be sent on it",
                channel.name
            ),
            LogSeverity::Warning,
        );
    }

    match (expect_fd, &channel.protocol) {
        (true, Protocol::Can) => log_msg(
            format!(
                "the A2L transport is CAN FD but channel '{}' is classic CAN; \
                 Init commands and DAQ frames will not work on this channel",
                channel.name
            ),
            LogSeverity::Warning,
        ),
        (false, Protocol::CanFd) => log_msg(
            format!(
                "the A2L transport is classic CAN but channel '{}' is CAN FD; \
                 make sure the ECU tolerates this",
                channel.name
            ),
            LogSeverity::Warning,
        ),
        _ => {}
    }
}
