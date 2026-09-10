//! The JSON configuration the extension is built from, and turning it into runtime state.
//!
//! The [`Config`] type is the shared contract between the GUI and this extension.

use std::fmt;

use crate::decode::{Decoder, SignalMeta};
use crate::state::CommandFrame;
use protocol_core::protocol::{self, Protocol};

pub(crate) use protocol_core::config::Config;

/// Everything a config change produces.
pub(crate) struct AppliedConfig {
    pub(crate) protocol: Protocol,
    pub(crate) decoder: Decoder,
    pub(crate) signal_metadata: Vec<SignalMeta>,
    pub(crate) expect_fd: bool,
    pub(crate) fd_brs: bool,
    pub(crate) init_frames: Vec<CommandFrame>,
    pub(crate) stop_frames: Vec<CommandFrame>,
    pub(crate) start_daq: bool,
    pub(crate) command_tx_id: Option<u32>,
}

/// Why a config could not be turned into runtime state.
#[derive(Debug)]
pub(crate) enum ConfigError {
    InvalidJson(serde_json::Error),
    NoCanId,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidJson(e) => write!(f, "Invalid DAQ Decoder config: {e}"),
            ConfigError::NoCanId => write!(
                f,
                "DAQ Decoder config has no DAQ CAN id, cannot decode DAQ frames"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Turn a parsed config into runtime state
pub(crate) fn apply_config(cfg: &Config) -> Result<AppliedConfig, ConfigError> {
    let decoder =
        Decoder::build(&cfg.project, &cfg.daq_lists, cfg.protocol).ok_or(ConfigError::NoCanId)?;

    let xcp_info = cfg
        .project
        .xcp_data
        .as_ref()
        .and_then(|x| x.xcp_info.as_ref());
    let ccp_transport = cfg.project.ccp_data.as_ref().map(|c| &c.info);

    // The CAN id init commands are sent on: DAQ master, or CCP CRO.
    let command_tx_id = match cfg.protocol {
        Protocol::Xcp => xcp_info.and_then(|c| c.can_id_master),
        Protocol::Ccp => ccp_transport.map(|t| t.cro_id),
    };

    // CAN-FD is XCP-only; base CCP is classic CAN.
    let can_fd = xcp_info.and_then(|c| c.can_fd.as_ref());
    let expect_fd = can_fd.is_some();
    let fd_brs = can_fd.is_some_and(|fd| fd.data_baudrate.is_some());

    let (init_frames, stop_frames) = match command_tx_id {
        Some(id) => {
            let init = protocol::build_init_commands(cfg.protocol, &cfg.project, &cfg.daq_lists);
            let stop = protocol::build_stop_commands(cfg.protocol, &cfg.project, &cfg.daq_lists);
            (command_frames(id, init), command_frames(id, stop))
        }
        None => (Vec::new(), Vec::new()),
    };

    let signal_metadata = decoder.signal_metadata(&cfg.project);

    Ok(AppliedConfig {
        protocol: cfg.protocol,
        decoder,
        signal_metadata,
        expect_fd,
        fd_brs,
        init_frames,
        stop_frames,
        start_daq: cfg.start_daq,
        command_tx_id,
    })
}

/// Turn a protocol command sequence into ready-to-send frames, all sent on `id`.
fn command_frames(id: u32, cmds: Vec<protocol::Command>) -> Vec<CommandFrame> {
    cmds.into_iter()
        .map(|cmd| CommandFrame {
            id,
            data: cmd.bytes,
            is_extended_id: id > 0x7FF,
            response: cmd.response,
            label: cmd.label,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_config_applies_cleanly() {
        let cfg = Config::sample();
        let applied = apply_config(&cfg).expect("Config::sample must always apply");
        assert_eq!(applied.protocol, Protocol::Xcp);
        assert!(applied.expect_fd, "Config::sample declares can_fd");
        assert!(
            applied.fd_brs,
            "Config::sample's can_fd sets a data_baudrate"
        );
        assert!(applied.command_tx_id.is_some());
        assert!(!applied.init_frames.is_empty());
        assert!(!applied.stop_frames.is_empty());
        assert_eq!(applied.signal_metadata.len(), 1);
    }

    #[test]
    fn no_can_id_slave_is_reported() {
        let mut cfg = Config::sample();
        cfg.project
            .xcp_data
            .as_mut()
            .unwrap()
            .xcp_info
            .as_mut()
            .unwrap()
            .can_id_slave = None;

        assert!(matches!(apply_config(&cfg), Err(ConfigError::NoCanId)));
    }

    #[test]
    fn no_command_can_id_is_not_an_error() {
        let mut cfg = Config::sample();
        cfg.project
            .xcp_data
            .as_mut()
            .unwrap()
            .xcp_info
            .as_mut()
            .unwrap()
            .can_id_master = None;

        let applied = apply_config(&cfg).expect("missing command CAN id is not fatal");
        assert!(applied.command_tx_id.is_none());
        assert!(applied.init_frames.is_empty());
        assert!(applied.stop_frames.is_empty());
    }
}
