//! Generating the Memorator `.t` script that drives DAQ logging.
//!
//! `emit` renders the script text, `trigger` resolves what the trigger reads,
//! `inline_signals` generates decoder functions for scripts that carry no DBC, and
//! `trigger_expr` parses a textual trigger expression into terms.
//!
//! This module holds the configuration those parts consume, plus the helpers that derive it from a parsed A2L.

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
    Accessor, CompareOp, Comparison, TriggerAction, TriggerCondition, TriggerSignal,
    dbc_trigger_signals, inline_trigger_signals,
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
    /// The CAN channel to configure at `on start`; CAN FD when it has a data bitrate.
    pub channel: CanChannel,
    /// How many ms of bus traffic to keep either side of the logger starting/stopping.
    pub pretrigger_ms: u32,
    /// What signals the script triggers on.
    pub signals: Vec<SignalHook>,
    /// Stop logging this many ms after the trigger fires. `None` leaves the logger running.
    pub timer_ms: Option<u32>,
    /// With `timer_ms`: stop DAQ after this many captures. `None` keeps capturing.
    pub max_captures: Option<u32>,
    /// Check once per this many ms that DAQ frames still arrive, and start the DAQ init over if none did.
    /// `None` disables the watchdog.
    pub daq_watchdog_ms: Option<u32>,
    /// Stop logging and DAQ for good when a frame with this CAN id arrives.
    pub stop_can_id: Option<u32>,
    /// The DAQ initialisation state machine to drive, when the script is to bring the ECU up itself.
    pub daq_init: Option<DaqInit>,
    pub traceability: Vec<String>,
    pub decoders: Vec<SignalDecoder>,
}

impl TScriptConfig {
    /// Whether the script talks CAN FD: the channel has a data-phase bitrate.
    pub(super) fn can_fd(&self) -> bool {
        self.channel.data_bitrate.is_some()
    }
}

/// The DAQ protocol initialisation the t-script drives before logging.
#[derive(Debug)]
pub struct DaqInit {
    pub protocol: Protocol,
    pub master_id: u32,
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
    // Responses arrive on the slave id, so the init needs both.
    let (master_id, _slave_id) = ids?;
    Some(DaqInit {
        protocol,
        master_id,
        timeout_ms: DEFAULT_INIT_TIMEOUT_MS,
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
    /// When it holds, the script stops logging and DAQ for good.
    pub stop: Option<TriggerCondition>,
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

/// Resolve the `on CanMessage` hooks the script needs, and split `trigger` and `stop` across them.
///
/// Returns an error naming the missing CAN id if `inline` is requested but the A2L declares none.
pub fn resolve_signal_hooks(
    inline: bool,
    project: &A2lProject,
    daq_lists: &[DaqList],
    protocol: Protocol,
    trigger: Option<(&ResolvedTrigger, TriggerAction)>,
    stop: Option<&ResolvedTrigger>,
) -> Result<Vec<SignalHook>, String> {
    if !inline {
        return Ok(dbc_signal_hooks(
            project, daq_lists, protocol, trigger, stop,
        ));
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
            let condition = |r: &ResolvedTrigger, action| TriggerCondition {
                signals: signals_for_can_id(&r.signals, &r.decoders, can_id),
                comparisons: r.comparisons.clone(),
                action,
            };
            SignalHook {
                message: MessageRef::Id {
                    can_id,
                    extended: can_id > 0x7FF,
                },
                trigger: trigger.map(|(r, action)| condition(r, action)),
                stop: stop.map(|r| condition(r, TriggerAction::StopLogger)),
                handles_responses: can_id == default_id,
            }
        })
        .collect())
}

/// The hooks of a script that reads signals through the DBC: one per DBC message plus a response-only hook on the default DAQ id when no list uses it.
pub fn dbc_signal_hooks(
    project: &A2lProject,
    daq_lists: &[DaqList],
    protocol: Protocol,
    trigger: Option<(&ResolvedTrigger, TriggerAction)>,
    stop: Option<&ResolvedTrigger>,
) -> Vec<SignalHook> {
    let condition = |r: &ResolvedTrigger, action, in_message: &dyn Fn(&TriggerSignal) -> bool| {
        TriggerCondition {
            signals: r
                .signals
                .iter()
                .filter(|s| in_message(s))
                .cloned()
                .collect(),
            comparisons: r.comparisons.clone(),
            action,
        }
    };
    // Without a DAQ CAN id there is no DBC either.
    let Some(default_id) = crate::layout::daq_can_id(project, protocol) else {
        return vec![SignalHook {
            message: MessageRef::Dbc(crate::dbc::DAQ_MESSAGE_NAME.to_string()),
            trigger: trigger.map(|(r, action)| condition(r, action, &|_| true)),
            stop: stop.map(|r| condition(r, TriggerAction::StopLogger, &|_| true)),
            handles_responses: true,
        }];
    };

    let groups = crate::layout::group_by_can_id(daq_lists, default_id);
    let mut hooks: Vec<SignalHook> = groups
        .iter()
        .map(|(can_id, lists)| {
            let placements = crate::layout::signal_placements(project, lists);
            let in_message = |s: &TriggerSignal| placements.iter().any(|p| p.name == s.source_name);
            SignalHook {
                message: MessageRef::Dbc(crate::dbc::message_name(*can_id, groups.len())),
                trigger: trigger.map(|(r, action)| condition(r, action, &in_message)),
                stop: stop.map(|r| condition(r, TriggerAction::StopLogger, &in_message)),
                handles_responses: *can_id == default_id,
            }
        })
        .collect();
    if !groups.iter().any(|(id, _)| *id == default_id) {
        hooks.push(SignalHook {
            message: MessageRef::Id {
                can_id: default_id,
                extended: default_id > 0x7FF,
            },
            trigger: None,
            stop: None,
            handles_responses: true,
        });
    }
    hooks
}

/// Why `can_id` can't be the stop frame: the script already receives DAQ data or sends commands on it.
pub fn stop_can_id_conflict(
    can_id: u32,
    project: &A2lProject,
    daq_lists: &[DaqList],
    protocol: Protocol,
) -> Option<String> {
    let daq_ids = crate::layout::daq_can_id(project, protocol)
        .map(|default| {
            let mut ids: Vec<u32> = crate::layout::group_by_can_id(daq_lists, default)
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            ids.push(default);
            ids
        })
        .unwrap_or_default();
    let command_id = match protocol {
        Protocol::Xcp => project
            .xcp_data
            .as_ref()
            .and_then(|x| x.xcp_info.as_ref())
            .and_then(|info| info.can_id_master),
        Protocol::Ccp => project.ccp_data.as_ref().map(|c| c.info.cro_id),
    };
    if daq_ids.contains(&can_id) {
        Some(format!("stop frame id 0x{can_id:X} carries DAQ data"))
    } else if command_id == Some(can_id) {
        Some(format!("stop frame id 0x{can_id:X} is the command id"))
    } else {
        None
    }
}

/// Every decoder in `a` and `b`, once each.
pub fn merge_decoders(mut a: Vec<SignalDecoder>, b: Vec<SignalDecoder>) -> Vec<SignalDecoder> {
    for d in b {
        if !a.iter().any(|existing| existing.ident == d.ident) {
            a.push(d);
        }
    }
    a
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
    use super::test_util::two_can_id_project;
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
        assert!(build_daq_init(Protocol::Xcp, &ok, &daq).is_some());
        assert!(build_daq_init(Protocol::Xcp, &with_ids(None, Some(0x456)), &daq).is_none());
        assert!(build_daq_init(Protocol::Xcp, &A2lProject::default(), &daq).is_none());

        assert!(build_daq_init(Protocol::Ccp, &ok, &daq).is_none());

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
        let init =
            build_daq_init(Protocol::Ccp, &ccp, &daq).expect("CCP init built from TP_BLOB ids");
        assert_eq!(init.master_id, 0x455);
    }

    #[test]
    fn dbc_hooks_use_the_dbc_message_names_and_split_the_signals() {
        let (project, mut daq_lists) = two_can_id_project();
        let terms = parse_terms("A > 1 && B > 2").unwrap();
        let resolved = resolve_terms(&terms, false, &project, &daq_lists, 0x456).unwrap();
        let trigger = Some((&resolved, TriggerAction::StartLogger));
        let latched = |h: &SignalHook| -> Vec<String> {
            let t = h
                .trigger
                .as_ref()
                .expect("every DAQ message hook evaluates the trigger");
            t.signals.iter().map(|s| s.ident.clone()).collect()
        };

        let hooks = dbc_signal_hooks(&project, &daq_lists, Protocol::Xcp, trigger, None);
        let dbc = crate::create_dbc_file(&project, &daq_lists, Protocol::Xcp, "t.a2l").unwrap();
        let names: Vec<&str> = hooks
            .iter()
            .map(|h| match &h.message {
                MessageRef::Dbc(name) => name.as_str(),
                MessageRef::Id { .. } => panic!("both ids carry a DAQ list"),
            })
            .collect();
        assert_eq!(names, ["DAQ_456", "DAQ_789"]);
        for name in &names {
            assert!(
                dbc.contains(&format!(" {name}: ")),
                "{name} not in the DBC:\n{dbc}"
            );
        }
        assert!(hooks[0].handles_responses && !hooks[1].handles_responses);
        assert_eq!(latched(&hooks[0]), ["A"]);
        assert_eq!(latched(&hooks[1]), ["B"]);

        // No list left on the default id: its responses still need a hook, and the DBC has no message for it.
        daq_lists[0].can_id = crate::CanIdSource::Fixed { id: 0x790 };
        let hooks = dbc_signal_hooks(&project, &daq_lists, Protocol::Xcp, trigger, None);
        assert_eq!(hooks.len(), 3);
        assert!(hooks[..2].iter().all(|h| !h.handles_responses));
        assert!(matches!(
            hooks[2],
            SignalHook {
                message: MessageRef::Id { can_id: 0x456, .. },
                handles_responses: true,
                trigger: None,
                ..
            }
        ));
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
            None,
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
    fn a_stop_frame_may_not_reuse_a_daq_or_command_id() {
        let project = crate::parse_a2l(
            r#"
            /begin MEASUREMENT Rpm "" UWORD NO_COMPU_METHOD 0 0 0 8000 ECU_ADDRESS 0x1000 /end MEASUREMENT
            /begin IF_DATA XCP
              /begin XCP_ON_CAN 0x0105 CAN_ID_MASTER 0x0455 CAN_ID_SLAVE 0x0456 BAUDRATE 500000 /end XCP_ON_CAN
              /begin DAQ DYNAMIC /begin DAQ_LIST 0x0000 MAX_ODT 0x05 MAX_ODT_ENTRIES 0x07 /end DAQ_LIST /end DAQ
            /end IF_DATA
            "#,
            &[],
        )
        .unwrap();
        let conflict = |id| stop_can_id_conflict(id, &project, &[], Protocol::Xcp);
        assert!(conflict(0x456).unwrap().contains("DAQ data"));
        assert!(conflict(0x455).unwrap().contains("command id"));
        assert_eq!(conflict(0x123), None);
    }
}
