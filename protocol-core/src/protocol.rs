//! Protocol selection and dispatch for DAQ command generation.
//!
//! Everything downstream of A2L parsing treats DAQ initialisation as a protocol-agnostic byte-replay.
//! This module is the single place where the measurement protocol is chosen and dispatched:
//!
//! - [`Protocol`] - the selectable protocol
//! - [`build_init_commands`] / [`build_stop_commands`] - dispatch to the concrete implementation
//! - [`ResponseFormat`] - where in the frame a command response carries its verdict and error code,
//!   consumed both by [`classify_response`] at runtime and by the t-script generator
//! - [`classify_response`] - what an incoming frame on the slave CAN ID means for a pending command.
//!
//! Adding a protocol means implementing these three entry points for it;

use crate::types::{A2lProject, DaqList};

/// The measurement protocol used to drive DAQ on the ECU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Protocol {
    #[default]
    Xcp,
    Ccp,
}

impl Protocol {
    /// Every protocol in canonical, preferred-first order.
    pub const ALL: [Protocol; 2] = [Protocol::Xcp, Protocol::Ccp];

    /// Human-readable protocol name for labels and log messages.
    pub fn name(self) -> &'static str {
        match self {
            Protocol::Xcp => "XCP",
            Protocol::Ccp => "CCP",
        }
    }

    /// How this protocol marks a command response, see [`ResponseFormat`].
    ///
    /// XCP: PID `0xFF` is a positive response, PID `0xFE` a negative one with the error code in byte 1.
    ///
    /// CCP: every Command Return Message uses PID `0xFF`; byte 1 is the error code, `0x00` meaning acknowledge. Positive
    /// and negative responses therefore share a PID and differ only in byte 1.
    pub fn response_format(self) -> ResponseFormat {
        match self {
            Protocol::Xcp => ResponseFormat {
                positive: &[(0, 0xFF)],
                error: &[(0, 0xFE)],
                error_code_byte: 1,
            },
            Protocol::Ccp => ResponseFormat {
                positive: &[(0, 0xFF), (1, 0x00)],
                error: &[(0, 0xFF)],
                error_code_byte: 1,
            },
        }
    }

    /// What a positive response to an [`ExpectedResponse::FirstPid`] command reveals
    /// about the DAQ list that command addressed.
    ///
    /// XCP: the `START_STOP_DAQ_LIST` (select) response is `[0xFF, FIRST_PID]`.
    ///
    /// CCP: the `GET_DAQ_SIZE` CRM is `[0xFF, err, ctr, DAQ_LIST_SIZE, FIRST_PID]`.
    pub fn daq_list_report(self, data: &[u8]) -> DaqListReport {
        let byte = |i: usize| data.get(i).copied();
        match self {
            Protocol::Xcp => DaqListReport {
                first_pid: byte(1),
                odt_count: None,
            },
            Protocol::Ccp => DaqListReport {
                first_pid: byte(4),
                odt_count: byte(3),
            },
        }
    }

    /// The highest PID a slave may assign to a DAQ ODT.
    pub fn max_daq_pid(self) -> u8 {
        match self {
            Protocol::Xcp => 0xFB,
            Protocol::Ccp => 0xFE,
        }
    }

    /// Where this protocol carries the command counter that ties a response to its
    /// command, or `None` if it has no such counter.
    ///
    /// CCP numbers every CRO in byte 1 and the slave echoes it in CRM byte 2. XCP has no command counter.
    pub fn command_counter(self) -> Option<CommandCounter> {
        match self {
            Protocol::Xcp => None,
            Protocol::Ccp => Some(CommandCounter {
                in_command: 1,
                in_response: 2,
            }),
        }
    }

    /// Maps an A2L `IF_DATA` interface tag to the measurement protocol it
    /// declares, or `None` for interfaces that are not a measurement protocol
    pub fn from_if_data_tag(tag: &str) -> Option<Protocol> {
        if tag.contains("XCP") {
            Some(Protocol::Xcp)
        } else if tag.contains("CCP") {
            Some(Protocol::Ccp)
        } else {
            None
        }
    }
}

/// The initialisation phase a command belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Establish the session (XCP: CONNECT, GET_STATUS).
    Connect,
    /// Bring DAQ configuration to a known state: stop any running DAQ, then  clear (static) or free + allocate (dynamic) the lists.
    Prepare,
    /// Fill ODTs with the entries to sample.
    WriteEntries,
    /// Bind each list to its event channel, timestamp bit, prescaler, priority.
    SetMode,
    /// Select every list and start them synchronously.
    Start,
    /// Tear down a running session: stop all lists, release them, disconnect.
    Stop,
}

impl Phase {
    pub fn tag(self) -> &'static str {
        match self {
            Phase::Connect => "connect",
            Phase::Prepare => "prepare",
            Phase::WriteEntries => "write-entries",
            Phase::SetMode => "set-mode",
            Phase::Start => "start",
            Phase::Stop => "stop",
        }
    }
}

/// What the slave's positive response to a command carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedResponse {
    Ack,
    FirstPid { daq_index: usize },
}

/// What a slave told us about one DAQ list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DaqListReport {
    pub first_pid: Option<u8>,
    pub odt_count: Option<u8>,
}

/// Byte positions of a protocol's command counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandCounter {
    pub in_command: usize,
    pub in_response: usize,
}

/// One command frame: phase, raw payload bytes, human-readable label and the expected response.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub phase: Phase,
    pub bytes: Vec<u8>,
    pub label: String,
    pub response: ExpectedResponse,
    pub optional: bool,
}

impl Command {
    /// A mandatory command expecting a plain acknowledge.
    pub(crate) fn ack(phase: Phase, bytes: Vec<u8>, label: String) -> Self {
        Command {
            phase,
            bytes,
            label,
            response: ExpectedResponse::Ack,
            optional: false,
        }
    }
}

/// Bytes a frame must carry to match, as `(index, value)` pairs.
pub type BytePattern = &'static [(usize, u8)];

/// Where a protocol carries the verdict of a command response inside the frame.
#[derive(Debug, Clone, Copy)]
pub struct ResponseFormat {
    pub positive: BytePattern,
    pub error: BytePattern,
    pub error_code_byte: usize,
}

/// What an incoming frame on the slave CAN ID means for a pending command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    /// A positive command response; the init sequence may advance.
    Positive,
    /// A negative command response carrying an error code.
    Error { code: u8 },
    /// Not a command response (e.g. a DAQ data frame).
    Unrelated,
}

/// Build the full DAQ initialisation command sequence for `protocol`.
pub fn build_init_commands(
    protocol: Protocol,
    project: &A2lProject,
    daq_lists: &[DaqList],
) -> Vec<Command> {
    match protocol {
        Protocol::Xcp => crate::xcp::build_init_commands(project, daq_lists),
        Protocol::Ccp => crate::ccp::build_init_commands(project, daq_lists),
    }
}

/// Build the shutdown sequence for a running DAQ session for `protocol`.
pub fn build_stop_commands(
    protocol: Protocol,
    project: &A2lProject,
    daq_lists: &[DaqList],
) -> Vec<Command> {
    match protocol {
        Protocol::Xcp => crate::xcp::build_stop_commands(project, daq_lists),
        Protocol::Ccp => crate::ccp::build_stop_commands(project, daq_lists),
    }
}

/// Classify a frame received on the slave CAN ID.
pub fn classify_response(protocol: Protocol, data: &[u8]) -> ResponseType {
    let format = protocol.response_format();
    let matches = |pattern: BytePattern| pattern.iter().all(|(i, v)| data.get(*i) == Some(v));
    if matches(format.positive) {
        ResponseType::Positive
    } else if matches(format.error) {
        ResponseType::Error {
            code: data.get(format.error_code_byte).copied().unwrap_or(0),
        }
    } else {
        ResponseType::Unrelated
    }
}

/// Encode a 16-bit command parameter in the slave's byte order.
pub(crate) fn word(v: u16, big_endian: bool) -> [u8; 2] {
    if big_endian {
        v.to_be_bytes()
    } else {
        v.to_le_bytes()
    }
}

/// Encode a 32-bit command parameter in the slave's byte order.
pub(crate) fn dword(v: u32, big_endian: bool) -> [u8; 4] {
    if big_endian {
        v.to_be_bytes()
    } else {
        v.to_le_bytes()
    }
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CanIdSource, CcpData, EventEntry, EventInfo, Odt, OdtEntry, ParsedSignal, XcpData,
    };

    /// A `protocol` project with event "E" on channel 3, and one list sampling `A` (address extension 3) and `B` on it.
    fn fixture(protocol: Protocol) -> (A2lProject, Vec<DaqList>) {
        let events = vec![EventEntry {
            channel: 3,
            name: "E".into(),
            ..Default::default()
        }];
        let mut project = A2lProject {
            signals: vec![ParsedSignal {
                name: "A".into(),
                address_extension: 3,
                ..Default::default()
            }],
            ..Default::default()
        };
        match protocol {
            Protocol::Xcp => {
                project.xcp_data = Some(XcpData {
                    event_info: EventInfo { events },
                    ..Default::default()
                })
            }
            Protocol::Ccp => {
                project.ccp_data = Some(CcpData {
                    rasters: events,
                    ..Default::default()
                })
            }
        }
        let entry = |name: &str, ecu_address| OdtEntry {
            names: vec![name.into()],
            ecu_address,
            size: 4,
        };
        let daq = vec![DaqList {
            prescaler: 1,
            id: 0,
            first_pid: 0,
            odts: vec![Odt {
                entries: vec![entry("A", 0x1000), entry("B", 0x2000)],
            }],
            event: Some("E".into()),
            can_id: CanIdSource::Default,
        }];
        (project, daq)
    }

    #[test]
    fn init_phases_appear_in_order_and_stop_ends_with_disconnect() {
        for protocol in Protocol::ALL {
            let (project, daq) = fixture(protocol);
            let cmds = build_init_commands(protocol, &project, &daq);
            let order = [
                Phase::Connect,
                Phase::Prepare,
                Phase::WriteEntries,
                Phase::SetMode,
                Phase::Start,
            ];
            let mut last = 0;
            for c in &cmds {
                let pos = order.iter().position(|p| *p == c.phase).unwrap();
                assert!(
                    pos >= last,
                    "{protocol:?}: phase went backwards at {}",
                    c.label
                );
                last = pos;
            }
            for phase in order {
                assert!(
                    cmds.iter().any(|c| c.phase == phase),
                    "{protocol:?}: missing {phase:?}"
                );
            }

            let stop = build_stop_commands(protocol, &project, &daq);
            assert!(stop.iter().all(|c| c.phase == Phase::Stop), "{protocol:?}");
            let disconnect = match protocol {
                Protocol::Xcp => 0xFE,
                Protocol::Ccp => 0x07,
            };
            assert_eq!(stop.last().unwrap().bytes[0], disconnect, "{protocol:?}");
        }
    }

    #[test]
    fn get_seed_and_unlock_are_optional_connect_phase_commands() {
        for protocol in Protocol::ALL {
            let (project, daq) = fixture(protocol);
            let cmds = build_init_commands(protocol, &project, &daq);
            // resource=DAQ, then a 6-byte zero key; CCP's are the third and fourth 8-byte CROs `[cmd, ctr, ...]`.
            let (seed, unlock): (&[u8], &[u8]) = match protocol {
                Protocol::Xcp => (&[0xF8, 0x00, 0x04], &[0xF7, 0x06, 0, 0, 0, 0, 0, 0]),
                Protocol::Ccp => (
                    &[0x12, 2, 0x02, 0, 0, 0, 0, 0],
                    &[0x13, 3, 0, 0, 0, 0, 0, 0],
                ),
            };
            for (label, bytes) in [("GET_SEED", seed), ("UNLOCK", unlock)] {
                let c = cmds.iter().find(|c| c.label.starts_with(label)).unwrap();
                assert_eq!(c.phase, Phase::Connect, "{protocol:?} {label}");
                assert_eq!(c.bytes, bytes, "{protocol:?} {label}");
                assert!(
                    c.optional,
                    "{protocol:?}: {label} must not abort init if unsupported"
                );
            }
            assert_eq!(
                cmds.iter().filter(|c| c.optional).count(),
                2,
                "{protocol:?}: every other command stays mandatory"
            );
        }
    }

    #[test]
    fn write_daq_carries_the_signal_address_extension() {
        for protocol in Protocol::ALL {
            let (project, daq) = fixture(protocol);
            let cmds = build_init_commands(protocol, &project, &daq);
            let writes: Vec<&[u8]> = cmds
                .iter()
                .filter(|c| c.label.starts_with("WRITE_DAQ"))
                .map(|c| c.bytes.as_slice())
                .collect();
            assert_eq!(
                writes[0][3], 3,
                "{protocol:?} A: extension from its MEASUREMENT"
            );
            assert_eq!(
                writes.last().unwrap()[3],
                0,
                "{protocol:?} B: undeclared means 0"
            );
        }
    }

    #[test]
    fn ccp_response_classification() {
        let p = Protocol::Ccp;
        assert_eq!(
            classify_response(p, &[0xFF, 0x00, 0x01]),
            ResponseType::Positive
        );
        assert_eq!(
            classify_response(p, &[0xFF, 0x33, 0x01]),
            ResponseType::Error { code: 0x33 }
        );
        assert_eq!(classify_response(p, &[0xFE, 0x00]), ResponseType::Unrelated);
        assert_eq!(classify_response(p, &[0x00, 0x12]), ResponseType::Unrelated);
        assert_eq!(classify_response(p, &[]), ResponseType::Unrelated);
    }

    #[test]
    fn xcp_response_classification() {
        let p = Protocol::Xcp;
        assert_eq!(classify_response(p, &[0xFF, 0x00]), ResponseType::Positive);
        assert_eq!(
            classify_response(p, &[0xFE, 0x22]),
            ResponseType::Error { code: 0x22 }
        );
        assert_eq!(
            classify_response(p, &[0xFE]),
            ResponseType::Error { code: 0 }
        );
        assert_eq!(classify_response(p, &[0x00, 0x12]), ResponseType::Unrelated);
        assert_eq!(classify_response(p, &[]), ResponseType::Unrelated);
    }

    #[test]
    fn truncated_ccp_return_message_is_not_an_acknowledge() {
        assert_eq!(
            classify_response(Protocol::Ccp, &[0xFF]),
            ResponseType::Error { code: 0 }
        );
    }

    #[test]
    fn if_data_tag_maps_to_protocol() {
        assert_eq!(Protocol::from_if_data_tag("XCP"), Some(Protocol::Xcp));
        assert_eq!(
            Protocol::from_if_data_tag("XCP_ON_CAN"),
            Some(Protocol::Xcp)
        );
        assert_eq!(
            Protocol::from_if_data_tag("ASAP1B_CCP"),
            Some(Protocol::Ccp)
        );
        assert_eq!(Protocol::from_if_data_tag("CANAPE_EXT"), None);
        assert_eq!(Protocol::from_if_data_tag("ETK"), None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn protocol_serializes_as_plain_string_and_defaults_to_xcp() {
        assert_eq!(serde_json::to_string(&Protocol::Xcp).unwrap(), "\"Xcp\"");
        assert_eq!(serde_json::to_string(&Protocol::Ccp).unwrap(), "\"Ccp\"");
        let back: Protocol = serde_json::from_str("\"Ccp\"").unwrap();
        assert_eq!(back, Protocol::Ccp);
        assert_eq!(Protocol::default(), Protocol::Xcp);
    }
}
