//! Parsing the module-level `/begin IF_DATA XCP ... /end IF_DATA` block: DAQ list
//! capabilities, event channels, and the `XCP_ON_CAN` transport parameters.

use super::common::parse_byte_order;
use super::tokenizer::{
    blocks, child_blocks, extract_block, parse_can_id_token, parse_u16_token, parse_u32_token,
    value_after,
};
use super::{
    CanFdInfo, CanIdSource, DaqInfo, DaqListDef, DaqMode, EventEntry, EventInfo, Protocol,
    ProtocolLayerInfo, XcpData, XcpInfo,
};

/// Finds the module-level `/begin IF_DATA XCP /end IF_DATA` and returns the parsed `XcpData`
///
/// Returns `None` if the file has no XCP interface at all.
pub(super) fn parse_if_data_xcp(tokens: &[String]) -> Option<XcpData> {
    let xcp: Vec<&[String]> = blocks(tokens, "IF_DATA")
        .filter_map(|b| b.split_first())
        .filter(|(tag, _)| Protocol::from_if_data_tag(tag) == Some(Protocol::Xcp))
        .map(|(_, block)| block)
        .collect();
    let block = xcp
        .iter()
        .find(|b| describes_the_interface(b))
        .or(xcp.first())?;
    Some(parse_xcp_contents(block))
}

/// Whether an `IF_DATA XCP` block carries interface-level content rather than the per-measurement `DAQ_EVENT` reference.
fn describes_the_interface(block: &[String]) -> bool {
    const INTERFACE_BLOCKS: [&str; 3] = ["DAQ", "XCP_ON_CAN", "PROTOCOL_LAYER"];
    block
        .windows(2)
        .any(|w| w[0] == "/begin" && INTERFACE_BLOCKS.contains(&w[1].as_str()))
}

/// Parses a `IF_DATA XCP` block.
fn parse_xcp_contents(tokens: &[String]) -> XcpData {
    let mut daq_info = DaqInfo::default();
    let mut event_info = EventInfo::default();
    let mut xcp_info: Option<XcpInfo> = None;
    let mut protocol_layer: Option<ProtocolLayerInfo> = None;
    let mut daq_list_can_ids: Vec<(u16, CanIdSource)> = Vec::new();

    for (keyword, block) in child_blocks(tokens) {
        match keyword {
            "DAQ" => {
                let (info, events) = parse_daq_contents(block);
                daq_info = info;
                event_info.events = events;
            }
            "XCP_ON_CAN" => {
                let (info, overrides) = parse_xcp_on_can(block);
                xcp_info = info;
                daq_list_can_ids = overrides;
            }
            "PROTOCOL_LAYER" => {
                protocol_layer = Some(parse_protocol_layer(block));
            }
            _ => {} // SEGMENT, PAG, PGM, etc.
        }
    }

    for (list_id, can_id) in daq_list_can_ids {
        if let Some(def) = daq_info
            .daq_lists
            .iter_mut()
            .find(|d| d.identifier == list_id)
        {
            def.can_id = can_id;
        }
    }

    XcpData {
        daq_info,
        event_info,
        xcp_info,
        protocol_layer,
    }
}

/// Parses a `/begin DAQ` block, extracting global header parameters, DAQ_LIST, and EVENT sub-blocks.
fn parse_daq_contents(tokens: &[String]) -> (DaqInfo, Vec<EventEntry>) {
    let mut daq_lists = Vec::new();
    let mut events = Vec::new();
    let mut max_odt_entry_size: u16 = 0;
    let mut odt_entry_granularity: u16 = 0;
    let mut timestamp_size: u16 = 0;
    let mut timestamp_fixed = false;
    let mut prescaler_supported = false;

    let mode = match tokens.first().map(String::as_str) {
        Some("STATIC") => DaqMode::Static,
        Some("DYNAMIC") => DaqMode::Dynamic,
        _ => DaqMode::StaticAndDynamic,
    };

    // Header: mode MAX_DAQ MAX_EVENT_CHANNEL MIN_DAQ ...; read MIN_DAQ only when the counts before it are there.
    let header: Vec<Option<u16>> = (1..=3)
        .map(|i| tokens.get(i).and_then(|t| parse_u16_token(t)))
        .collect();
    let min_daq = match header[..] {
        [Some(_), Some(_), Some(min)] => min,
        _ => 0,
    };

    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "/begin" {
            let keyword = tokens.get(i + 1).map(String::as_str).unwrap_or("");
            i += 2;
            let block = extract_block(tokens, &mut i);

            match keyword {
                "DAQ_LIST" => {
                    if let Some(entry) = parse_daq_list_block(block) {
                        daq_lists.push(entry);
                    }
                }
                "EVENT" => {
                    if let Some(entry) = parse_xcp_event(block) {
                        events.push(entry);
                    }
                }
                "TIMESTAMP_SUPPORTED" => {
                    let (size, fixed) = parse_timestamp_supported(block);
                    timestamp_size = size;
                    timestamp_fixed = fixed;
                }
                _ => {}
            }
        } else {
            // Header-level `GRANULARITY_ODT_ENTRY_SIZE_DAQ_<unit> <MAX_ODT_ENTRY_SIZE_DAQ>`.
            let granularity = match tokens[i].strip_prefix("GRANULARITY_ODT_ENTRY_SIZE_DAQ_") {
                Some("BYTE") => Some(1),
                Some("WORD") => Some(2),
                Some("DWORD") => Some(4),
                Some("DLONG") => Some(8),
                _ => None,
            };
            if let Some(g) = granularity {
                odt_entry_granularity = g;
                if let Some(v) = tokens.get(i + 1).and_then(|t| parse_u16_token(t)) {
                    max_odt_entry_size = v;
                }
            }
            prescaler_supported |= tokens[i] == "PRESCALER_SUPPORTED";
            i += 1;
        }
    }

    let daq_info = DaqInfo {
        mode,
        daq_lists,
        max_odt_entry_size,
        timestamp_size,
        timestamp_fixed,
        min_daq,
        odt_entry_granularity,
        prescaler_supported,
    };
    (daq_info, events)
}

/// Parses a `/begin TIMESTAMP_SUPPORTED` block.
///
/// Returns the timestamp width in bytes and whether `TIMESTAMP_FIXED` is set.
fn parse_timestamp_supported(tokens: &[String]) -> (u16, bool) {
    let mut size = 0u16;
    let mut fixed = false;
    for t in tokens {
        match t.as_str() {
            "SIZE_BYTE" => size = 1,
            "SIZE_WORD" => size = 2,
            "SIZE_DWORD" => size = 4,
            "TIMESTAMP_FIXED" => fixed = true,
            _ => {}
        }
    }
    (size, fixed)
}

/// Parses a `/begin DAQ_LIST` block.
///
/// Returns a `DaqListDef` with the list identifier, number of ODTs, maximum ODT entries, and optional event channel and first PID.
fn parse_daq_list_block(tokens: &[String]) -> Option<DaqListDef> {
    let value = |keyword| value_after(tokens, keyword).and_then(|t| parse_u16_token(t));
    Some(DaqListDef {
        identifier: parse_u16_token(tokens.first()?)?,
        nr_odt: value("MAX_ODT").unwrap_or(0),
        max_odt_entries: value("MAX_ODT_ENTRIES").unwrap_or(0),
        event_fixed: value("EVENT_FIXED"),
        first_pid: value("FIRST_PID"),
        can_id: CanIdSource::Default, // filled in later from any DAQ_LIST_CAN_ID override
    })
}

/// Parses an XCP `/begin EVENT` block: `"name" "short name" channel DAQ max_daq_list time_cycle time_unit ...`.
pub(super) fn parse_xcp_event(tokens: &[String]) -> Option<EventEntry> {
    parse_event_entry(tokens, 5, 6)
}

/// Parses an event declaration `"name" "short name" channel ...` whose time cycle and unit are at `cycle_at`/`unit_at`.
///
/// Returns an `EventEntry` with the channel number, name, and optional time cycle/unit.
pub(super) fn parse_event_entry(
    tokens: &[String],
    cycle_at: usize,
    unit_at: usize,
) -> Option<EventEntry> {
    let name = tokens
        .first()
        .filter(|t| t.starts_with('"'))
        .map(|t| t.trim_matches('"').to_string())?;
    let channel = parse_u32_token(tokens.get(2)?)?;
    let time_cycle = tokens
        .get(cycle_at)
        .and_then(|t| parse_u32_token(t))
        .unwrap_or(0);
    let time_unit = tokens
        .get(unit_at)
        .and_then(|t| parse_u32_token(t))
        .unwrap_or(0);

    Some(EventEntry {
        channel,
        name,
        time_cycle,
        time_unit,
    })
}

/// Parses a `/begin XCP_ON_CAN` block.
///
/// Returns an `XcpInfo` with the version, CAN IDs, baudrate, and optional CAN FD info,
/// plus any `DAQ_LIST_CAN_ID` overrides found for the caller to apply onto the DAQ lists declared elsewhere in the module.
fn parse_xcp_on_can(tokens: &[String]) -> (Option<XcpInfo>, Vec<(u16, CanIdSource)>) {
    let Some(version) = tokens.first().and_then(|t| parse_u32_token(t)) else {
        return (None, Vec::new());
    };
    let mut can_id_broadcast = None;
    let mut can_id_master = None;
    let mut can_id_slave = None;
    let mut baudrate = None;
    let mut can_fd = None;
    let mut max_dlc_required = false;
    let mut daq_list_can_ids: Vec<(u16, CanIdSource)> = Vec::new();

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "CAN_ID_BROADCAST" => {
                can_id_broadcast = tokens.get(i + 1).and_then(|t| parse_can_id_token(t));
                i += 2;
            }
            "CAN_ID_MASTER" => {
                can_id_master = tokens.get(i + 1).and_then(|t| parse_can_id_token(t));
                i += 2;
            }
            "CAN_ID_SLAVE" => {
                can_id_slave = tokens.get(i + 1).and_then(|t| parse_can_id_token(t));
                i += 2;
            }
            "BAUDRATE" => {
                baudrate = tokens.get(i + 1).and_then(|t| parse_u32_token(t));
                i += 2;
            }
            "MAX_DLC_REQUIRED" => {
                max_dlc_required = true;
                i += 1;
            }
            "/begin" => {
                let keyword = tokens.get(i + 1).map(String::as_str).unwrap_or("");
                i += 2;
                let block = extract_block(tokens, &mut i);
                match keyword {
                    "CAN_FD" => can_fd = Some(parse_can_fd(block)),
                    "DAQ_LIST_CAN_ID" => {
                        if let Some(pair) = parse_daq_list_can_id(block) {
                            daq_list_can_ids.push(pair);
                        }
                    }
                    _ => {}
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    (
        Some(XcpInfo {
            version,
            can_id_broadcast,
            can_id_master,
            can_id_slave,
            baudrate,
            can_fd,
            max_dlc_required,
        }),
        daq_list_can_ids,
    )
}

/// Parses a `/begin DAQ_LIST_CAN_ID` block: the DAQ list identifier, followed by either `FIXED <id>` or `VARIABLE` if it is mutable or not
///
/// Returns `(daq list identifier, CAN id source)`.
fn parse_daq_list_can_id(tokens: &[String]) -> Option<(u16, CanIdSource)> {
    let daq_list = parse_u16_token(tokens.first()?)?;
    let source = match tokens.get(1).map(String::as_str) {
        Some("VARIABLE") => CanIdSource::Dynamic { assigned: None },
        Some("FIXED") => CanIdSource::Fixed {
            id: parse_can_id_token(tokens.get(2)?)?,
        },
        _ => return None,
    };
    Some((daq_list, source))
}

/// Parses a `/begin CAN_FD` block, extracting the frame size and data baud rate.
///
/// Returns a `CanFdInfo` with the maximum DLC and optional data transfer baud rate.
fn parse_can_fd(tokens: &[String]) -> CanFdInfo {
    CanFdInfo {
        max_dlc: value_after(tokens, "MAX_DLC")
            .and_then(|t| parse_u16_token(t))
            .unwrap_or(0),
        data_baudrate: value_after(tokens, "CAN_FD_DATA_TRANSFER_BAUDRATE")
            .and_then(|t| parse_u32_token(t)),
        max_dlc_required: tokens.iter().any(|t| t == "MAX_DLC_REQUIRED"),
    }
}

/// Parses the content tokens of a `PROTOCOL_LAYER` block.
///
/// Returns a `ProtocolLayerInfo` with the maximum CTO/DTO sizes and optional byte order.
fn parse_protocol_layer(tokens: &[String]) -> ProtocolLayerInfo {
    let max_cto = tokens.get(8).and_then(|t| parse_u16_token(t)).unwrap_or(0);
    let max_dto = tokens.get(9).and_then(|t| parse_u16_token(t)).unwrap_or(0);
    // The last `BYTE_ORDER_MSB_FIRST`/`BYTE_ORDER_MSB_LAST` after the fixed parameters wins.
    let byte_order = tokens
        .iter()
        .skip(10)
        .rev()
        .find_map(|t| t.strip_prefix("BYTE_ORDER_").and_then(parse_byte_order));
    ProtocolLayerInfo {
        byte_order,
        max_cto,
        max_dto,
    }
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2l::tokenizer::tokenize_checked;

    const MEASUREMENT_FIRST: &str = r#"
    /begin MEASUREMENT Rpm "Engine speed" UWORD Rpm_CM 0 0 0 8000
      ECU_ADDRESS 0x1000
      /begin IF_DATA XCP
        /begin DAQ_EVENT FIXED_EVENT_LIST EVENT 0x0001 /end DAQ_EVENT
      /end IF_DATA
    /end MEASUREMENT
    /begin IF_DATA XCP
      /begin XCP_ON_CAN
        0x0105 CAN_ID_MASTER 0x0455 CAN_ID_SLAVE 0x0456 BAUDRATE 500000
      /end XCP_ON_CAN
      /begin DAQ
        DYNAMIC
        /begin EVENT "Task10ms" "10ms" 0x0000 DAQ 0x01 0x0A 0x06 0xFF /end EVENT
      /end DAQ
    /end IF_DATA
    "#;

    #[test]
    fn the_module_interface_wins_over_a_measurements_daq_event() {
        let tokens = tokenize_checked(MEASUREMENT_FIRST).unwrap();
        let xcp = parse_if_data_xcp(&tokens).expect("XCP interface found");
        let info = xcp
            .xcp_info
            .expect("transport comes from the module-level block, not the DAQ_EVENT one");
        assert_eq!(info.can_id_master, Some(0x455));
        assert_eq!(info.can_id_slave, Some(0x456));
        assert_eq!(xcp.event_info.events.len(), 1);
        assert_eq!(xcp.event_info.events[0].name, "Task10ms");
    }

    #[test]
    fn a_daq_event_only_file_still_yields_something() {
        let tokens = tokenize_checked(
            r#"
            /begin IF_DATA XCP
              /begin DAQ_EVENT FIXED_EVENT_LIST EVENT 0x0001 /end DAQ_EVENT
            /end IF_DATA
            "#,
        )
        .unwrap();
        let xcp = parse_if_data_xcp(&tokens).expect("the block is still an XCP interface");
        assert!(xcp.xcp_info.is_none());
    }

    #[test]
    fn daq_list_can_id_overrides_the_owning_list_only() {
        let tokens = tokenize_checked(
            r#"
            /begin IF_DATA XCP
              /begin XCP_ON_CAN
                0x0105 CAN_ID_MASTER 0x0455 CAN_ID_SLAVE 0x0456 BAUDRATE 500000
                /begin DAQ_LIST_CAN_ID
                  0x0001 FIXED 0x0789
                /end DAQ_LIST_CAN_ID
              /end XCP_ON_CAN
              /begin DAQ
                STATIC
                /begin DAQ_LIST 0x0000 MAX_ODT 0x01 MAX_ODT_ENTRIES 0x01 /end DAQ_LIST
                /begin DAQ_LIST 0x0001 MAX_ODT 0x01 MAX_ODT_ENTRIES 0x01 /end DAQ_LIST
              /end DAQ
            /end IF_DATA
            "#,
        )
        .unwrap();
        let xcp = parse_if_data_xcp(&tokens).expect("XCP interface found");
        let lists = xcp.daq_info.daq_lists;
        assert_eq!(lists.len(), 2);
        assert_eq!(
            lists[0].can_id,
            CanIdSource::Default,
            "list 0 keeps transmitting on CAN_ID_SLAVE"
        );
        assert_eq!(
            lists[1].can_id,
            CanIdSource::Fixed { id: 0x789 },
            "list 1 has its own fixed CAN id"
        );
    }

    #[test]
    fn daq_list_can_id_variable_marks_the_list_as_dynamic() {
        let tokens = tokenize_checked(
            r#"
            /begin IF_DATA XCP
              /begin XCP_ON_CAN
                0x0105 CAN_ID_MASTER 0x0455 CAN_ID_SLAVE 0x0456 BAUDRATE 500000
                /begin DAQ_LIST_CAN_ID
                  0x0000 VARIABLE
                /end DAQ_LIST_CAN_ID
              /end XCP_ON_CAN
              /begin DAQ
                STATIC
                /begin DAQ_LIST 0x0000 MAX_ODT 0x01 MAX_ODT_ENTRIES 0x01 /end DAQ_LIST
              /end DAQ
            /end IF_DATA
            "#,
        )
        .unwrap();
        let xcp = parse_if_data_xcp(&tokens).expect("XCP interface found");
        assert_eq!(
            xcp.daq_info.daq_lists[0].can_id,
            CanIdSource::Dynamic { assigned: None },
            "VARIABLE gives no id, only the capability to assign one via SET_DAQ_ID"
        );
    }

    #[test]
    fn no_xcp_interface_is_none() {
        let tokens = tokenize_checked(
            r#"
            /begin IF_DATA ASAP1B_CCP
              /begin TP_BLOB 0x0201 /end TP_BLOB
            /end IF_DATA
            "#,
        )
        .unwrap();
        assert!(parse_if_data_xcp(&tokens).is_none());
    }

    #[test]
    fn extended_id_flag_is_stripped_and_max_dlc_required_is_read() {
        let tokens = tokenize_checked(
            r#"
            /begin IF_DATA XCP
              /begin XCP_ON_CAN
                0x0104 CAN_ID_MASTER 0x98FF0001 CAN_ID_SLAVE 0x98FF0002 MAX_DLC_REQUIRED
                /begin DAQ_LIST_CAN_ID 0x0000 FIXED 0x98FF0003 /end DAQ_LIST_CAN_ID
                /begin CAN_FD MAX_DLC 64 MAX_DLC_REQUIRED /end CAN_FD
              /end XCP_ON_CAN
              /begin DAQ DYNAMIC /begin DAQ_LIST 0x0000 /end DAQ_LIST /end DAQ
            /end IF_DATA
            "#,
        )
        .unwrap();
        let xcp = parse_if_data_xcp(&tokens).unwrap();
        let info = xcp.xcp_info.unwrap();
        assert_eq!(info.can_id_master, Some(0x18FF0001));
        assert_eq!(info.can_id_slave, Some(0x18FF0002));
        assert_eq!(
            xcp.daq_info.daq_lists[0].can_id,
            CanIdSource::Fixed { id: 0x18FF0003 }
        );
        assert!(info.max_dlc_required);
        assert!(info.can_fd.unwrap().max_dlc_required);
    }

    #[test]
    fn daq_header_gives_min_daq_granularity_and_max_entry_size() {
        let parse = |header: &str| {
            let a2l = format!(
                "/begin IF_DATA XCP /begin DAQ {header} \
                 /begin DAQ_LIST 0x0003 /end DAQ_LIST /end DAQ /end IF_DATA"
            );
            parse_if_data_xcp(&tokenize_checked(&a2l).unwrap())
                .unwrap()
                .daq_info
        };

        let d = parse(
            "DYNAMIC 0x10 0x04 0x02 OPTIMISATION_TYPE_DEFAULT ADDRESS_EXTENSION_FREE \
             IDENTIFICATION_FIELD_TYPE_ABSOLUTE GRANULARITY_ODT_ENTRY_SIZE_DAQ_WORD 0x06 NO_OVERLOAD_INDICATION",
        );
        assert_eq!(d.min_daq, 2);
        assert_eq!(d.odt_entry_granularity, 2);
        assert_eq!(
            d.max_odt_entry_size, 6,
            "read for every granularity, not just BYTE"
        );

        let d = parse("DYNAMIC");
        assert_eq!(
            d.min_daq, 0,
            "a DAQ_LIST id must not be mistaken for MIN_DAQ"
        );
        assert_eq!(d.odt_entry_granularity, 0);
        assert!(!d.prescaler_supported);

        let d = parse("DYNAMIC 0x10 0x04 0x00 OVERLOAD_INDICATION_EVENT PRESCALER_SUPPORTED");
        assert!(d.prescaler_supported);
    }
}
