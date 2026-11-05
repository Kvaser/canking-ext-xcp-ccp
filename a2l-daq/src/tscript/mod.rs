//! Generating the Memorator `.t` script that drives DAQ logging.
//!
//! `emit` renders the script text, `trigger` resolves what the trigger reads,
//! `inline_signals` generates decoder functions for scripts that carry no DBC, and
//! `trigger_expr` parses a textual trigger expression into terms.
//!
//! This module holds the configuration those parts consume, plus the helpers that derive
//! it from a parsed A2L.

mod emit;
mod inline_signals;
mod trigger;
mod trigger_expr;

#[cfg(test)]
mod test_util;

pub use emit::generate_tscript;
pub use inline_signals::{
    DecoderType, SignalDecoder, TriggerError, decoder_functions, decoders_for, helper_typedef,
};
pub use trigger::{
    Accessor, CompareOp, Comparison, TriggerAction, TriggerCondition, TriggerParts, TriggerSignal,
    dbc_trigger_signals, inline_trigger_signals, resolve_trigger,
};
pub use trigger_expr::{
    ResolvedTrigger, TriggerExprError, TriggerTerm, build_trigger, expand_signal_indices,
    parse_terms, resolve_terms,
};

use crate::a2l::A2lProject;
use crate::daq::DaqList;
use protocol_core::protocol::{self, Command, Protocol};

pub const DEFAULT_INIT_TIMEOUT_MS: u32 = 1000;
pub const DEFAULT_BITRATE: u32 = 500_000;
pub const DEFAULT_FD_DATA_BITRATE: u32 = 2_000_000;

/// Everything [`generate_tscript`] needs to emit one `.t` file.
pub struct TScriptConfig {
    /// The CAN channels to configure at `on start`.
    pub channels: Vec<CanChannel>,
    /// How many ms of bus traffic to keep either side of the logger starting/stopping.
    pub pretrigger_ms: u32,
    pub signals: Vec<SignalHook>,
    /// Stop logging this many ms after the trigger fires. `None` leaves the logger running.
    pub timer_ms: Option<u32>,
    /// The DAQ initialisation state machine to drive, when the script is to bring the ECU up itself.
    pub daq_init: Option<DaqInit>,
    pub can_fd: bool,
    pub traceability: Vec<String>,
    pub decoders: Vec<SignalDecoder>,
}

/// The DAQ protocol initialisation the t-script drives before logging.
#[derive(Debug)]
pub struct DaqInit {
    pub protocol: Protocol,
    pub master_id: u32,
    pub slave_id: u32,
    /// How long to wait for a response before resending the current command.
    pub timeout_ms: u32,
    /// The commands to send during initialisation, in order, each with its comment.
    pub commands: Vec<Command>,
    /// The commands to send when logging stops, releasing the DAQ list(s) and disconnecting.
    pub stop_commands: Vec<Command>,
}

/// Assemble the DAQ init state machine for the t-script from a project and its packed DAQ lists,
/// or `Ok(None)` if the A2L lacks the master/slave CAN identifiers needed to drive it.
/// Thin wrapper over the protocol-dispatched `build_init_commands` and `build_stop_commands`.
pub fn build_daq_init(
    protocol: Protocol,
    project: &A2lProject,
    daq_lists: &[DaqList],
    timeout_ms: u32,
) -> Option<DaqInit> {
    let ids = match protocol {
        Protocol::Xcp => project
            .xcp_data
            .as_ref()
            .and_then(|x| x.xcp_info.as_ref())
            .and_then(|info| Some((info.can_id_master?, info.can_id_slave?))),
        Protocol::Ccp => project
            .ccp_data
            .as_ref()
            .map(|c| (c.info.cro_id, c.info.dto_id)),
    };
    let (master_id, slave_id) = ids?;
    Some(DaqInit {
        protocol,
        master_id,
        slave_id,
        timeout_ms,
        commands: protocol::build_init_commands(protocol, project, daq_lists),
        stop_commands: protocol::build_stop_commands(protocol, project, daq_lists),
    })
}

/// Struct representing a CAN channel configuration
pub struct CanChannel {
    pub index: u32,
    pub bitrate: u32,
    pub data_bitrate: Option<u32>,
}

/// Bitrate and (if declared) CAN FD data-phase bitrate, read from the A2L's transport block (`XCP_ON_CAN` or the CCP `TP_BLOB`)
pub struct BusDefaults {
    pub bitrate: u32,
    pub can_fd_data_bitrate: Option<u32>,
}

/// Derive [`BusDefaults`] for `protocol` from `project`.
pub fn bus_defaults(project: &A2lProject, protocol: Protocol) -> BusDefaults {
    let xcp_info = project.xcp_data.as_ref().and_then(|x| x.xcp_info.as_ref());
    let bitrate = match protocol {
        Protocol::Xcp => xcp_info.and_then(|c| c.baudrate),
        Protocol::Ccp => project.ccp_data.as_ref().and_then(|c| c.info.baudrate),
    }
    .unwrap_or(DEFAULT_BITRATE);
    let can_fd_data_bitrate = match protocol {
        Protocol::Xcp => xcp_info.and_then(|c| c.can_fd.as_ref()),
        Protocol::Ccp => None,
    }
    .map(|fd| fd.data_baudrate.unwrap_or(DEFAULT_FD_DATA_BITRATE));
    BusDefaults {
        bitrate,
        can_fd_data_bitrate,
    }
}

/// Struct representing a signal hook configuration for the t-script
pub struct SignalHook {
    pub message: MessageRef,
    pub trigger: Option<TriggerCondition>,
    pub handles_responses: bool,
}

/// How the message hook selects the DAQ frames.
pub enum MessageRef {
    Dbc(String),
    Id { can_id: u32, extended: bool },
}

impl MessageRef {
    /// Render the message part of the `on CanMessage` hook.
    pub(super) fn hook_selector(&self) -> String {
        match self {
            Self::Dbc(name) => name.clone(),
            Self::Id { can_id, extended } => {
                format!("(0x{:X}){}", can_id, if *extended { "x" } else { "" })
            }
        }
    }
}

/// Resolve the `on CanMessage` hooks the script needs, and split `trigger` across them.
///
/// Returns an error naming the missing CAN id if `inline` is requested but the A2L declares none.
pub fn resolve_signal_hooks(
    inline: bool,
    project: &A2lProject,
    daq_lists: &[DaqList],
    protocol: Protocol,
    trigger: Option<(&ResolvedTrigger, TriggerAction)>,
) -> Result<Vec<SignalHook>, String> {
    if !inline {
        let hook_trigger = trigger.map(|(r, action)| TriggerCondition {
            signals: r.signals.clone(),
            comparisons: r.comparisons.clone(),
            action,
        });
        return Ok(vec![SignalHook {
            message: MessageRef::Dbc("DAQ".to_string()),
            trigger: hook_trigger,
            handles_responses: true,
        }]);
    }

    let default_id = crate::layout::daq_can_id(project, protocol)
        .ok_or_else(|| format!("the A2L declares no {} DAQ CAN id", protocol.name()))?;

    let mut ids: Vec<u32> = crate::layout::group_by_can_id(daq_lists, default_id)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    if let Err(pos) = ids.binary_search(&default_id) {
        ids.insert(pos, default_id);
    }

    Ok(ids
        .into_iter()
        .map(|can_id| {
            let hook_trigger = trigger.map(|(r, action)| TriggerCondition {
                signals: signals_for_can_id(&r.signals, &r.decoders, can_id),
                comparisons: r.comparisons.clone(),
                action,
            });
            SignalHook {
                message: MessageRef::Id {
                    can_id,
                    extended: can_id > 0x7FF,
                },
                trigger: hook_trigger,
                handles_responses: can_id == default_id,
            }
        })
        .collect())
}

/// The trigger signals whose decoder reads a frame carried on `can_id`.
fn signals_for_can_id(
    signals: &[TriggerSignal],
    decoders: &[inline_signals::SignalDecoder],
    can_id: u32,
) -> Vec<TriggerSignal> {
    signals
        .iter()
        .filter(|s| {
            decoders
                .iter()
                .any(|d| d.ident == s.ident && d.can_id == can_id)
        })
        .cloned()
        .collect()
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_daq_init_requires_master_and_slave_ids() {
        use crate::a2l::{CanIdSource, DaqInfo, EventInfo, XcpData, XcpInfo};
        use crate::daq::{Odt, OdtEntry};

        let daq = vec![DaqList {
            prescaler: 1,
            id: 0,
            first_pid: 0,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["A".into()],
                    ecu_address: 0x1000,
                    size: 4,
                }],
            }],
            event: None,
            can_id: CanIdSource::Default,
        }];

        let with_ids = |master, slave| A2lProject {
            xcp_data: Some(XcpData {
                daq_info: DaqInfo::default(),
                event_info: EventInfo::default(),
                xcp_info: Some(XcpInfo {
                    can_id_master: master,
                    can_id_slave: slave,
                    ..Default::default()
                }),
                protocol_layer: None,
            }),
            ..Default::default()
        };

        let ok = with_ids(Some(0x455), Some(0x456));
        assert!(build_daq_init(Protocol::Xcp, &ok, &daq, 1000).is_some());
        assert!(build_daq_init(Protocol::Xcp, &with_ids(None, Some(0x456)), &daq, 1000).is_none());
        assert!(build_daq_init(Protocol::Xcp, &A2lProject::default(), &daq, 1000).is_none());

        assert!(build_daq_init(Protocol::Ccp, &ok, &daq, 1000).is_none());

        use crate::{CcpData, CcpInfo};
        let ccp = A2lProject {
            ccp_data: Some(CcpData {
                info: CcpInfo {
                    cro_id: 0x455,
                    dto_id: 0x456,
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let init = build_daq_init(Protocol::Ccp, &ccp, &daq, 1000)
            .expect("CCP init built from TP_BLOB ids");
        assert_eq!(init.master_id, 0x455);
        assert_eq!(init.slave_id, 0x456);
    }

    fn two_can_id_project() -> (A2lProject, Vec<DaqList>) {
        use crate::a2l::{CanIdSource, DaqInfo, EventInfo, ParsedSignal, XcpData, XcpInfo};
        use crate::daq::{Odt, OdtEntry};

        let project = A2lProject {
            signals: vec![
                ParsedSignal {
                    name: "A".into(),
                    size: 2,
                    ecu_address: 0x10,
                    ..Default::default()
                },
                ParsedSignal {
                    name: "B".into(),
                    size: 2,
                    ecu_address: 0x2000,
                    ..Default::default()
                },
            ],
            xcp_data: Some(XcpData {
                daq_info: DaqInfo::default(),
                event_info: EventInfo::default(),
                xcp_info: Some(XcpInfo {
                    can_id_slave: Some(0x456),
                    ..Default::default()
                }),
                protocol_layer: None,
            }),
            ..Default::default()
        };
        let daq_lists = vec![
            DaqList {
                prescaler: 1,
                id: 0,
                first_pid: 1,
                event: None,
                odts: vec![Odt {
                    entries: vec![OdtEntry {
                        names: vec!["A".into()],
                        ecu_address: 0x10,
                        size: 2,
                    }],
                }],
                can_id: CanIdSource::Default,
            },
            DaqList {
                prescaler: 1,
                id: 1,
                first_pid: 2,
                event: None,
                odts: vec![Odt {
                    entries: vec![OdtEntry {
                        names: vec!["B".into()],
                        ecu_address: 0x2000,
                        size: 2,
                    }],
                }],
                can_id: CanIdSource::Fixed { id: 0x789 },
            },
        ];
        (project, daq_lists)
    }

    #[test]
    fn inline_hooks_split_one_per_can_id_with_responses_on_the_default() {
        let (project, daq_lists) = two_can_id_project();
        let terms = parse_terms("A > 1 && B > 2").unwrap();
        let resolved = resolve_terms(&terms, true, &project, &daq_lists, 0x456)
            .expect("both signals decode inline");

        let hooks = resolve_signal_hooks(
            true,
            &project,
            &daq_lists,
            Protocol::Xcp,
            Some((&resolved, TriggerAction::StartLogger)),
        )
        .expect("both DAQ lists have a resolvable CAN id");

        assert_eq!(hooks.len(), 2, "one hook per distinct CAN id");

        let ids: Vec<u32> = hooks
            .iter()
            .map(|h| match h.message {
                MessageRef::Id { can_id, .. } => can_id,
                MessageRef::Dbc(_) => panic!("inline mode must hook raw CAN ids"),
            })
            .collect();
        assert_eq!(ids, [0x456, 0x789], "ascending, default id first");

        assert!(
            hooks[0].handles_responses,
            "the default id carries responses"
        );
        assert!(!hooks[1].handles_responses);

        fn idents(h: &SignalHook) -> Vec<&str> {
            h.trigger
                .as_ref()
                .map(|t| t.signals.iter().map(|s| s.ident.as_str()).collect())
                .unwrap_or_default()
        }
        assert_eq!(idents(&hooks[0]), ["A"], "0x456's hook only latches A");
        assert_eq!(idents(&hooks[1]), ["B"], "0x789's hook only latches B");

        for hook in &hooks {
            let comparisons = &hook.trigger.as_ref().unwrap().comparisons;
            assert_eq!(comparisons.len(), 2);
        }
    }

    #[test]
    fn dbc_mode_always_yields_exactly_one_hook() {
        let (project, daq_lists) = two_can_id_project();
        let terms = parse_terms("A > 1 && B > 2").unwrap();
        let resolved = resolve_terms(&terms, false, &project, &daq_lists, 0x456)
            .expect("DBC mode resolves both signals by name");

        let hooks = resolve_signal_hooks(
            false,
            &project,
            &daq_lists,
            Protocol::Xcp,
            Some((&resolved, TriggerAction::StartLogger)),
        )
        .expect("DBC mode never needs a CAN id");

        assert_eq!(hooks.len(), 1);
        assert!(matches!(hooks[0].message, MessageRef::Dbc(ref name) if name == "DAQ"));
        assert!(hooks[0].handles_responses);
        assert_eq!(hooks[0].trigger.as_ref().unwrap().signals.len(), 2);
    }
}
