//! Util shared by the `tscript` submodules' tests.

use super::*;
use crate::a2l::{A2lProject, CanIdSource};
use crate::daq::DaqList;
use protocol_core::protocol::{Command, ExpectedResponse, Phase, Protocol};

pub(super) fn cmd(bytes: Vec<u8>, label: &str) -> Command {
    Command {
        phase: Phase::Connect,
        bytes,
        label: label.into(),
        response: ExpectedResponse::Ack,
        optional: false,
    }
}

pub(super) fn optional_cmd(bytes: Vec<u8>, label: &str) -> Command {
    Command {
        optional: true,
        ..cmd(bytes, label)
    }
}

pub(super) fn stop_cmd(bytes: Vec<u8>, label: &str) -> Command {
    Command {
        phase: Phase::Stop,
        bytes,
        label: label.into(),
        response: ExpectedResponse::Ack,
        optional: false,
    }
}

pub(super) fn signal(ident: &str, pid: u16, read: &str) -> TriggerSignal {
    TriggerSignal {
        ident: ident.into(),
        pid,
        read: read.into(),
        source_name: ident.into(),
    }
}

pub(super) fn comparison(ident: &str, op: &str, value: &str) -> Comparison {
    Comparison {
        ident: ident.into(),
        op: CompareOp::from_symbol(op)
            .unwrap_or_else(|| panic!("'{op}' is not a comparison operator")),
        value: value.into(),
    }
}

/// One classic 500 kbit/s channel and one DBC `DAQ` hook without trigger, timer or DAQ init.
pub(super) fn dbc_config() -> TScriptConfig {
    TScriptConfig {
        stop_can_id: None,
        max_captures: None,
        daq_watchdog_ms: None,
        channel: CanChannel {
            index: 0,
            bitrate: 500_000,
            data_bitrate: None,
        },
        pretrigger_ms: 0,
        signals: vec![SignalHook {
            stop: None,
            message: MessageRef::Dbc("DAQ".into()),
            trigger: None,
            handles_responses: true,
        }],
        timer_ms: None,
        daq_init: None,
        traceability: vec![],
        decoders: vec![],
    }
}

pub(super) fn init_config(
    protocol: Protocol,
    commands: Vec<Command>,
    stop: Vec<Command>,
) -> TScriptConfig {
    TScriptConfig {
        timer_ms: Some(8000),
        daq_init: Some(DaqInit {
            protocol,
            master_id: 0x455,
            timeout_ms: 1000,
            commands,
            stop_commands: stop,
        }),
        ..dbc_config()
    }
}

pub(super) fn two_odt_project() -> (A2lProject, Vec<DaqList>) {
    use crate::a2l::{CompuMethod, Conversion, ParsedSignal};
    use crate::daq::{Odt, OdtEntry};

    let mut temp = ParsedSignal {
        name: "Temp".into(),
        size: 2,
        ecu_address: 0x20,
        is_signed: true,
        ..Default::default()
    };
    temp.conversion = Some(Conversion {
        identifier: "CM".into(),
    });
    let mut project = A2lProject {
        signals: vec![
            ParsedSignal {
                name: "Rpm".into(),
                size: 2,
                ecu_address: 0x10,
                ..Default::default()
            },
            temp,
        ],
        ..Default::default()
    };
    project.compu_methods.insert(
        "CM".into(),
        CompuMethod {
            factor: 0.1,
            offset: -40.0,
            unit: "degC".into(),
            ..Default::default()
        },
    );
    let daq_lists = vec![DaqList {
        prescaler: 1,
        id: 0,
        first_pid: 1,
        event: None,
        odts: vec![
            Odt {
                entries: vec![OdtEntry {
                    names: vec!["Rpm".into()],
                    ecu_address: 0x10,
                    size: 2,
                }],
            },
            Odt {
                entries: vec![OdtEntry {
                    names: vec!["Temp".into()],
                    ecu_address: 0x20,
                    size: 2,
                }],
            },
        ],
        can_id: CanIdSource::Default,
    }];
    (project, daq_lists)
}

pub(super) fn two_can_id_project() -> (A2lProject, Vec<DaqList>) {
    use crate::a2l::{DaqInfo, EventInfo, ParsedSignal, XcpData, XcpInfo};
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
                can_id_master: Some(0x455),
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

pub(super) fn standalone_config(can_id: u32, extended: bool) -> TScriptConfig {
    let (project, daq_lists) = two_odt_project();
    let decoders = decoders_for(
        &project,
        &daq_lists,
        &["Rpm".to_string(), "Temp".to_string()],
        can_id,
    )
    .expect("decoders build");
    TScriptConfig {
        pretrigger_ms: 200,
        signals: vec![SignalHook {
            stop: None,
            message: MessageRef::Id { can_id, extended },
            trigger: Some(TriggerCondition {
                signals: crate::inline_trigger_signals(&decoders),
                comparisons: vec![
                    comparison("Rpm", ">", "3000"),
                    comparison("Temp", "<", "90"),
                ],
                action: TriggerAction::StartLogger,
            }),
            handles_responses: true,
        }],
        timer_ms: Some(8000),
        decoders,
        ..dbc_config()
    }
}
