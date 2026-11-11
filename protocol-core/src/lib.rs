//! Shared core for the DAQ pipeline.
//!
//! Everything that more than one crate in this repository needs lives here:
//!
//! - [`ccp`] - the CCP implementation: transport-agnostic DAQ command generation.
//! - [`config`] (feature `serde`) - The JSON contract between the configurator GUI and the CanKing data processor.
//! - [`layout`] - where each DAQ signal lands in its CAN frame, and how to scale it.
//! - [`protocol`] - protocol selection ([`protocol::Protocol`]) and dispatch for DAQ command generation and response classification.
//! - [`types`] - the parsed-A2L and packed-DAQ-list data model.
//! - [`xcp`] - the XCP implementation: transport-agnostic DAQ command generation.
//!
//! ```
//! use protocol_core::{Protocol, protocol::build_init_commands, types::{A2lProject, XcpData}};
//!
//! let project = A2lProject { xcp_data: Some(XcpData::default()), ..Default::default() };
//! let commands = build_init_commands(Protocol::Xcp, &project, &[]);
//! assert_eq!(commands[0].bytes, [0xFF, 0x00], "CONNECT, normal mode");
//! ```

pub mod ccp;
pub mod layout;
pub mod protocol;
pub mod types;
pub mod xcp;

pub use protocol::Protocol;

#[cfg(feature = "serde")]
pub mod config;
#[cfg(feature = "serde")]
pub use config::Config;
