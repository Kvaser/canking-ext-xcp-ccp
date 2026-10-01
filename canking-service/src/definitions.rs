//! The frame and signal definitions the extension declares to CanKing.

use crate::bindings::canking::data_processor::types::{
    ByteOrder, ConversionMethod, DataLayout, DataType, EnumEntry, FrameDefinition,
    LinearConversion, LogSeverity, SignalDefinition, StateConversion,
};
use crate::decode::{ConversionMeta, Decoder, LayoutMeta, SignalMeta};
use crate::logging::log_msg;

pub(crate) const DAQ_FRAME_NAME: &str = "DAQ";
pub(crate) const PID_SIGNAL_NAME: &str = "PID";

/// Build the frame definitions CanKing uses to present the decoded DAQ signals
pub(crate) fn build_frame_definitions(
    decoder: &Decoder,
    metadata: &[SignalMeta],
    can_fd: bool,
    fd_brs: bool,
) -> Vec<FrameDefinition> {
    let can_ids = decoder.can_ids();
    can_ids
        .iter()
        .map(|&can_id| {
            let mut signals = vec![pid_signal_definition()];
            signals.extend(metadata.iter().filter(|m| m.can_id == can_id).map(|m| {
                SignalDefinition {
                    name: m.name.clone(),
                    comment: m.comment.clone(),
                    unit: m.unit.clone(),
                    min_value: m.min_value,
                    max_value: m.max_value,
                    layout: Some(data_layout(&m.layout)),
                    conversion: Some(conversion(&m.conversion)),
                    is_multiplexor: Some(false),
                    multiplexor: Some(PID_SIGNAL_NAME.to_string()),
                    multiplex_value: Some(m.pid as i32),
                }
            }));

            FrameDefinition {
                name: frame_name(can_id, can_ids.len()),
                comment: Some("Signals decoded from DAQ frames".to_string()),
                id: can_id,
                is_extended_id: Some(can_id > 0x7FF),
                is_can_fd: Some(can_fd),
                is_can_fd_with_brs: Some(fd_brs),
                data_length: Some(frame_length(decoder.frame_data_length(can_id), can_fd)),
                signals,
            }
        })
        .collect()
}

/// `len` rounded up to the next length a frame can carry: CAN FD only allows 0-8, 12, 16, 20, 24, 32, 48 or 64 bytes.
fn frame_length(len: u32, can_fd: bool) -> u32 {
    if !can_fd || len <= 8 {
        return len;
    }
    [12, 16, 20, 24, 32, 48, 64].into_iter().find(|&n| n >= len).unwrap_or(64)
}

/// The multiplexor signal every DAQ frame carries as its first byte.
fn pid_signal_definition() -> SignalDefinition {
    SignalDefinition {
        name: PID_SIGNAL_NAME.to_string(),
        comment: Some("DAQ ODT packet identifier".to_string()),
        unit: None,
        min_value: Some(0.0),
        max_value: Some(255.0),
        layout: Some(DataLayout {
            start_bit: 0,
            length: 8,
            byte_order: ByteOrder::LittleEndian,
            data_type: DataType::Unsigned,
        }),
        conversion: None,
        is_multiplexor: Some(true),
        multiplexor: None,
        multiplex_value: None,
    }
}

/// A frame's name: the plain `DAQ` name when it's the only one, otherwise disambiguated by CAN id.
fn frame_name(can_id: u32, total_frames: usize) -> String {
    if total_frames <= 1 {
        DAQ_FRAME_NAME.to_string()
    } else {
        format!("{DAQ_FRAME_NAME}_{can_id:X}")
    }
}

pub(crate) fn log_frame_definitions(defs: &[FrameDefinition]) {
    for f in defs {
        log_msg(format!("{f:?}"), LogSeverity::Debug);
    }
}

/// The wit `data-layout` for a decoded signal's bit layout.
fn data_layout(layout: &LayoutMeta) -> DataLayout {
    DataLayout {
        start_bit: layout.start_bit,
        length: layout.length,
        byte_order: if layout.big_endian {
            ByteOrder::BigEndian
        } else {
            ByteOrder::LittleEndian
        },
        data_type: if layout.is_float {
            if layout.length >= 64 {
                DataType::Double
            } else {
                DataType::Float
            }
        } else if layout.is_signed {
            DataType::Signed
        } else {
            DataType::Unsigned
        },
    }
}

/// The wit `conversion-method` for a decoded signal's physical value.
fn conversion(conversion: &ConversionMeta) -> ConversionMethod {
    match conversion {
        ConversionMeta::Linear { factor, offset } => ConversionMethod::Linear(LinearConversion {
            factor: *factor as f32,
            offset: *offset as f32,
        }),
        ConversionMeta::Enum(enums) => ConversionMethod::State(StateConversion {
            enum_definitions: enums
                .iter()
                .map(|(name, value)| EnumEntry {
                    name: name.clone(),
                    value: *value,
                })
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use protocol_core::protocol::Protocol;

    #[test]
    fn can_fd_frame_length_rounds_up_to_a_valid_dlc() {
        assert_eq!(frame_length(21, true), 24);
        assert_eq!(frame_length(20, true), 20);
        assert_eq!(frame_length(9, true), 12);
        assert_eq!(frame_length(7, true), 7);
        assert_eq!(frame_length(3, false), 3);
    }

    #[test]
    fn reports_layout_conversion_and_frame_data_length() {
        let cfg = Config::sample();
        let decoder = Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap();
        let metadata = decoder.signal_metadata(&cfg.project);

        let frames = build_frame_definitions(&decoder, &metadata, false, false);
        let frame = &frames[0];
        assert_eq!(frame.data_length, Some(3), "PID byte + 2-byte Rpm");

        let pid = frame
            .signals
            .iter()
            .find(|s| s.name == PID_SIGNAL_NAME)
            .expect("PID signal");
        let pid_layout = pid.layout.as_ref().expect("PID must have a layout");
        assert_eq!(pid_layout.start_bit, 0);
        assert_eq!(pid_layout.length, 8);
        assert_eq!(pid_layout.byte_order, ByteOrder::LittleEndian);
        assert_eq!(pid_layout.data_type, DataType::Unsigned);
        assert!(pid.conversion.is_none());

        let rpm = frame
            .signals
            .iter()
            .find(|s| s.name == "Rpm")
            .expect("Rpm signal");
        let rpm_layout = rpm.layout.as_ref().expect("Rpm must have a layout");
        assert_eq!(rpm_layout.start_bit, 8, "after the leading PID byte");
        assert_eq!(rpm_layout.length, 16);
        assert_eq!(rpm_layout.byte_order, ByteOrder::LittleEndian);
        assert_eq!(rpm_layout.data_type, DataType::Unsigned);
        match rpm.conversion.as_ref().expect("Rpm must have a conversion") {
            ConversionMethod::Linear(lin) => {
                assert_eq!(lin.factor, 0.5);
                assert_eq!(lin.offset, 0.0);
            }
            ConversionMethod::State(_) => panic!("Rpm has no enum representation"),
        }
    }

    #[test]
    fn builds_one_frame_per_can_id() {
        use protocol_core::types::{CanIdSource, DaqList, Odt, OdtEntry, ParsedSignal};

        let mut cfg = Config::sample();
        cfg.project.signals.push(ParsedSignal {
            name: "Load".to_string(),
            ecu_address: 0x2000,
            size: 1,
            event: Some("Task100ms".to_string()),
            ..Default::default()
        });
        cfg.daq_lists.push(DaqList {
            id: 1,
            first_pid: 0,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["Load".to_string()],
                    ecu_address: 0x2000,
                    size: 1,
                }],
            }],
            event: Some("Task100ms".to_string()),
            can_id: CanIdSource::Fixed { id: 0x789 },
        });

        let decoder = Decoder::build(&cfg.project, &cfg.daq_lists, Protocol::Xcp).unwrap();
        let metadata = decoder.signal_metadata(&cfg.project);
        let frames = build_frame_definitions(&decoder, &metadata, false, false);

        assert_eq!(frames.len(), 2, "one frame per CAN id");

        let default_frame = frames
            .iter()
            .find(|f| f.id == 0x456)
            .expect("default id frame");
        assert_eq!(default_frame.name, "DAQ_456", "disambiguated once >1 frame");
        assert!(default_frame.signals.iter().any(|s| s.name == "Rpm"));
        assert!(!default_frame.signals.iter().any(|s| s.name == "Load"));

        let other_frame = frames
            .iter()
            .find(|f| f.id == 0x789)
            .expect("override id frame");
        assert_eq!(other_frame.name, "DAQ_789");
        assert!(other_frame.signals.iter().any(|s| s.name == "Load"));
        assert!(!other_frame.signals.iter().any(|s| s.name == "Rpm"));
    }
}
