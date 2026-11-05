//! The JSON configuration contract between the DAQ Configurator (GUI) and the CanKing `daq-decoder` data processor.
//!
//! [`Config`] contains the complete DAQ lists and signal/transport metadata needed to:
//!
//! (a) drive DAQ startup on the ECU and
//! (b) decode the raw DAQ frames back into signals at runtime.
//!
//! # JSON representation constraints
//!
//! 1. `compu_methods` is serialized as a sorted **array** of  entries (see `compu_methods_serde` in [`crate::types`])
//! 2. [`Config::sample`] must keep every optional sub-object (`can_fd`, `protocol_layer`, ...) populated so its schema is inferable.

use serde::{Deserialize, Serialize};

use crate::protocol::Protocol;
use crate::types::{
    A2lProject, ByteOrder, CanFdInfo, CanIdSource, CcpData, CcpInfo, CcpSource, CompuMethod,
    Conversion, DaqInfo, DaqList, DaqMode, EventEntry, EventInfo, Odt, OdtEntry, ParsedSignal,
    ProtocolLayerInfo, XcpData, XcpInfo,
};

/// The full extension configuration the data processor takes in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Signal definitions, Transport config and COMPU_METHOD scaling.
    pub project: A2lProject,

    /// The ODT-packed DAQ lists, already compiled by the configurator.
    pub daq_lists: Vec<DaqList>,

    /// Whether the extension should drive DAQ startup on the ECU.
    #[serde(default)]
    pub start_daq: bool,

    /// The measurement protocol selected in the configurator.
    #[serde(default)]
    pub protocol: Protocol,
}

impl Config {
    /// The default/example configuration, returned as the data processor's
    /// `default-config`. Every optional sub-object must stay populated.
    pub fn sample() -> Self {
        let mut compu_methods = std::collections::HashMap::new();
        compu_methods.insert(
            "Rpm_CM".to_string(),
            CompuMethod {
                factor: 0.5,
                offset: 0.0,
                unit: "rpm".to_string(),
                text_table: vec![],
                default_text: Some("".to_string()),
                formula: None,
                unsupported: None,
            },
        );

        let project = A2lProject {
            signals: vec![ParsedSignal {
                name: "Rpm".to_string(),
                long_identifier: "Engine speed".to_string(),
                conversion: Some(Conversion {
                    identifier: "Rpm_CM".to_string(),
                }),
                lower_limit: 0.0,
                upper_limit: 8000.0,
                ecu_address: 0x1000,
                size: 2,
                bit_mask: None,
                byte_order: Some(ByteOrder::MsbLast),
                is_signed: false,
                is_float: false,
                event: Some("Task10ms".to_string()),
                virtual_inputs: Vec::new(),
                virtual_conversion: None,
            }],
            xcp_data: Some(XcpData {
                daq_info: DaqInfo {
                    mode: DaqMode::Dynamic,
                    daq_lists: Vec::new(),
                    max_odt_entry_size: 7,
                    timestamp_size: 0,
                    timestamp_fixed: false,
                    min_daq: 0,
                    odt_entry_granularity: 1,
                },
                event_info: EventInfo {
                    events: vec![EventEntry {
                        channel: 1,
                        name: "Task10ms".to_string(),
                        time_cycle: 10,
                        time_unit: 6,
                    }],
                },
                xcp_info: Some(XcpInfo {
                    version: 0x0102,
                    can_id_broadcast: None,
                    can_id_master: Some(0x455),
                    can_id_slave: Some(0x456),
                    baudrate: Some(500_000),
                    can_fd: Some(CanFdInfo {
                        max_dlc: 64,
                        data_baudrate: Some(2_000_000),
                        max_dlc_required: false,
                    }),
                    max_dlc_required: false,
                }),
                protocol_layer: Some(ProtocolLayerInfo {
                    byte_order: Some(ByteOrder::MsbLast),
                    max_cto: 8,
                    max_dto: 8,
                }),
            }),
            compu_methods,
            default_byte_order: Some(ByteOrder::MsbLast),
            ccp_data: Some(CcpData {
                info: CcpInfo {
                    version: 0x0201,
                    cro_id: 0x455,
                    dto_id: 0x456,
                    station_address: 0x0100,
                    baudrate: Some(500_000),
                    ..Default::default()
                },
                rasters: vec![EventEntry {
                    channel: 1,
                    name: "Task10ms".to_string(),
                    ..Default::default()
                }],
                sources: vec![CcpSource {
                    name: "Task10ms".to_string(),
                    daq_list: 0,
                    rasters: vec![1],
                    first_pid: Some(0),
                    length: Some(1),
                    can_id: Some(0x456),
                }],
            }),
        };

        let daq_lists = vec![DaqList {
            id: 0,
            first_pid: 0,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["Rpm".to_string()],
                    ecu_address: 0x1000,
                    size: 2,
                }],
            }],
            event: Some("Task10ms".to_string()),
            can_id: CanIdSource::Default,
        }];

        Config {
            project,
            daq_lists,
            start_daq: false,
            protocol: Protocol::Xcp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_round_trips_through_json() {
        let cfg = Config::sample();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.daq_lists, cfg.daq_lists);
        assert_eq!(back.project.signals, cfg.project.signals);
        assert_eq!(back.protocol, cfg.protocol);
    }

    #[test]
    fn config_without_protocol_field_defaults_to_xcp() {
        let mut value: serde_json::Value = serde_json::to_value(Config::sample()).unwrap();
        value.as_object_mut().unwrap().remove("protocol");
        let back: Config = serde_json::from_value(value).unwrap();
        assert_eq!(back.protocol, Protocol::Xcp);
    }

    #[test]
    fn compu_methods_serialize_as_named_array() {
        let mut project = A2lProject::default();
        project.compu_methods.insert(
            "B_CM".into(),
            CompuMethod {
                factor: 2.0,
                offset: 1.0,
                unit: "V".into(),
                ..Default::default()
            },
        );
        project.compu_methods.insert(
            "A_CM".into(),
            CompuMethod {
                factor: 0.5,
                offset: 0.0,
                unit: "rpm".into(),
                ..Default::default()
            },
        );
        let config = Config {
            project,
            daq_lists: Vec::new(),
            start_daq: false,
            protocol: Protocol::Xcp,
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(
            r#""compu_methods":[{"name":"A_CM","factor":0.5,"offset":0.0,"unit":"rpm"},{"name":"B_CM","factor":2.0,"offset":1.0,"unit":"V"}]"#
        ));

        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.project.compu_methods, config.project.compu_methods);
    }
}
