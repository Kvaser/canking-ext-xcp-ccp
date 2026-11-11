//! Signal/event/protocol/transport metadata shaped for the selection UI.

use std::collections::HashMap;

use super::group::parse_groups;
use super::tokenizer::{scan_protocols, tokenize_checked};
use super::unread::{self, UnreadKeyword};
use super::{
    A2lParseError, EventEntry, Protocol, RejectedVirtualReason, event_channels, parse_tokens,
    selected_refs,
};

/// Per-signal metadata
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignalSummary {
    pub name: String,
    pub description: String,
    pub unit: String,
    pub ecu_address: u64,
    pub size: u16,
    pub virtual_input: Option<String>,
    pub virtual_reject: Option<RejectedVirtualReason>,
    pub groups: Vec<String>,
}

/// A MEASUREMENT GROUP from A2L
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GroupInfo {
    pub name: String,
    pub description: String,
    pub root: bool,
    pub sub_groups: Vec<String>,
}

/// Bus parameters an A2L declares for one protocol
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportInfo {
    pub protocol: Protocol,
    pub can_fd: bool,
    pub bitrate: Option<u32>,
    pub data_bitrate: Option<u32>,
    pub prescaler: bool,
}

/// Summary of all signals, events, and protocols declared in an A2L file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct A2lSummary {
    pub signals: Vec<SignalSummary>,
    pub events: Vec<EventEntry>,
    pub protocols: Vec<Protocol>,
    pub transports: Vec<TransportInfo>,
    pub groups: Vec<GroupInfo>,
    pub unread: Vec<UnreadKeyword>,
}

/// Returns signal metadata, event channels, and declared protocols in a single pass.
pub fn inspect(content: &str) -> Result<A2lSummary, A2lParseError> {
    let tokens = tokenize_checked(content)?;
    let protocols = scan_protocols(&tokens);
    let (project, rejected) = parse_tokens(&tokens, selected_refs(&[]));

    // Map each measurement name to the GROUPs that directly reference it.
    let mut membership: HashMap<String, Vec<String>> = HashMap::new();
    let mut groups = Vec::new();
    for (group, measurements) in parse_groups(&tokens) {
        for m in measurements {
            membership.entry(m).or_default().push(group.name.clone());
        }
        groups.push(group);
    }

    // Build a summary of all signals
    let mut signals: Vec<SignalSummary> = project
        .signals
        .iter()
        .map(|s| SignalSummary {
            name: s.name.clone(),
            description: s.long_identifier.trim_matches('"').to_string(),
            unit: s
                .conversion
                .as_ref()
                .and_then(|c| project.compu_methods.get(&c.identifier))
                .map(|cm| cm.unit.clone())
                .unwrap_or_default(),
            ecu_address: s.ecu_address,
            size: s.size,
            virtual_input: s.virtual_inputs.first().cloned(),
            virtual_reject: None,
            groups: membership.get(s.name.as_str()).cloned().unwrap_or_default(),
        })
        .collect();

    // Add rejected virtual signals to the summary with a reason for rejection
    signals.extend(rejected.into_iter().map(|r| {
        SignalSummary {
            groups: membership
                .get(r.signal.name.as_str())
                .cloned()
                .unwrap_or_default(),
            name: r.signal.name,
            description: r.signal.long_identifier.trim_matches('"').to_string(),
            unit: String::new(),
            ecu_address: 0,
            size: r.signal.size,
            virtual_input: r.signal.virtual_inputs.first().cloned(),
            virtual_reject: Some(r.reason),
        }
    }));

    // Build a summary of all declared protocols and their bus parameters
    let transports = protocols
        .iter()
        .map(|&protocol| match protocol {
            Protocol::Xcp => {
                let info = project.xcp_data.as_ref().and_then(|x| x.xcp_info.as_ref());
                let fd = info.and_then(|i| i.can_fd.as_ref());
                TransportInfo {
                    protocol,
                    can_fd: fd.is_some(),
                    bitrate: info.and_then(|i| i.baudrate),
                    data_bitrate: fd.and_then(|f| f.data_baudrate),
                    prescaler: project
                        .xcp_data
                        .as_ref()
                        .is_some_and(|x| x.daq_info.prescaler_supported),
                }
            }
            Protocol::Ccp => TransportInfo {
                protocol,
                can_fd: false,
                bitrate: project.ccp_data.as_ref().and_then(|c| c.info.baudrate),
                data_bitrate: None,
                prescaler: true,
            },
        })
        .collect();

    let events = protocols
        .iter()
        .map(|&p| event_channels(&project, p))
        .find(|channels| !channels.is_empty())
        .unwrap_or_default()
        .to_vec();

    Ok(A2lSummary {
        signals,
        events,
        protocols,
        transports,
        groups,
        unread: unread::scan(&tokens),
    })
}
