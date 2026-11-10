//! Parsing the module-level `/begin IF_DATA ASAP1B_CCP ... /end IF_DATA` block: transport
//! parameters (`TP_BLOB`), event rasters, and DAQ list sources (`SOURCE`/`QP_BLOB`).

use super::tokenizer::{
    blocks, child_blocks, extract_block, parse_can_id_token, parse_u16_token, parse_u32_token,
};
use super::xcp::parse_event_entry;
use super::{CcpData, CcpInfo, CcpSource};

/// Finds the module-level `IF_DATA ASAP1B_CCP` block and parses its contents.
///
/// Returns a `CcpData`, or `None` if the file declares no CCP transport.
pub(super) fn parse_if_data_ccp(tokens: &[String]) -> Option<CcpData> {
    blocks(tokens, "IF_DATA")
        .filter_map(|b| b.split_first())
        .find(|(tag, block)| *tag == "ASAP1B_CCP" && block_has_begin(block, "TP_BLOB"))
        .map(|(_, block)| parse_ccp_contents(block))
}

/// Whether `tokens` contains a `/begin KEYWORD` at its top scan (any depth).
fn block_has_begin(tokens: &[String], keyword: &str) -> bool {
    tokens
        .windows(2)
        .any(|w| w[0] == "/begin" && w[1] == keyword)
}

/// Parses the contents of the module-level `IF_DATA ASAP1B_CCP` block
///
/// Returns a `CcpData` with the transport parameters, event rasters, and DAQ list sources.
fn parse_ccp_contents(tokens: &[String]) -> CcpData {
    let mut rasters = Vec::new();
    let mut sources = Vec::new();
    let mut info = CcpInfo::default();

    for (keyword, block) in child_blocks(tokens) {
        match keyword {
            // `"name" "short name" channel time_unit time_cycle`
            "RASTER" => {
                if let Some(r) = parse_event_entry(block, 4, 3) {
                    rasters.push(r);
                }
            }
            "SOURCE" => {
                if let Some(s) = parse_ccp_source(block) {
                    sources.push(s);
                }
            }
            "TP_BLOB" => {
                // Preserve a SEED_KEY parsed before TP_BLOB was reached since block order isn't guaranteed.
                let seed_key_dll = info.seed_key_dll.take();
                info = parse_tp_blob(block);
                info.seed_key_dll = seed_key_dll;
            }
            "SEED_KEY" => info.seed_key_dll = parse_seed_key(block),
            _ => {} // EVENT_GROUP, CHECKSUM
        }
    }

    CcpData {
        info,
        rasters,
        sources,
    }
}

/// Parses `/begin SEED_KEY "<CAL dll>" "<DAQ dll>" "<PGM dll>" /end SEED_KEY`, keeping the DAQ privilege's DLL
fn parse_seed_key(tokens: &[String]) -> Option<String> {
    let names: Vec<&str> = tokens
        .iter()
        .filter(|t| t.starts_with('"'))
        .map(|t| t.trim_matches('"'))
        .collect();
    let daq = if names.len() > 1 {
        names[1]
    } else {
        *names.first()?
    };
    (!daq.is_empty()).then(|| daq.to_string())
}

/// Parses a `/begin SOURCE` block
///
/// Returns a `CcpSource` with the DAQ list number and tagged parameters.
fn parse_ccp_source(tokens: &[String]) -> Option<CcpSource> {
    let name = tokens
        .first()
        .filter(|t| t.starts_with('"'))
        .map(|t| t.trim_matches('"').to_string())?;
    let (_, qp_blob) = child_blocks(tokens).find(|(keyword, _)| *keyword == "QP_BLOB")?;
    parse_qp_blob(name, qp_blob)
}

/// Parses a `/begin QP_BLOB` block: the DAQ list number followed by tagged parameters
///
/// Returns a `CcpSource` with the DAQ list number and tagged parameters.
fn parse_qp_blob(name: String, tokens: &[String]) -> Option<CcpSource> {
    let daq_list = parse_u16_token(tokens.first()?)?;
    let mut source = CcpSource {
        name,
        daq_list,
        ..Default::default()
    };

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "LENGTH" => {
                source.length = tokens.get(i + 1).and_then(|t| parse_u16_token(t));
                i += 2;
            }
            "FIRST_PID" => {
                source.first_pid = tokens.get(i + 1).and_then(|t| parse_u16_token(t));
                i += 2;
            }
            "CAN_ID_FIXED" => {
                source.can_id = tokens.get(i + 1).and_then(|t| parse_can_id_token(t));
                i += 2;
            }
            "RASTER" => {
                i += 1;
                while let Some(channel) = tokens.get(i).and_then(|t| parse_u32_token(t)) {
                    source.rasters.push(channel);
                    i += 1;
                }
            }
            "/begin" => {
                i += 2;
                let _ = extract_block(tokens, &mut i);
            }
            _ => i += 1,
        }
    }

    Some(source)
}

/// Parses a `/begin TP_BLOB` block.
///
/// Returns a `CcpInfo` with the transport parameters, including version, CRO/DTO IDs, station address,
/// baudrate, and whether BYTES_ONLY is set.
fn parse_tp_blob(tokens: &[String]) -> CcpInfo {
    let version = tokens.first().and_then(|t| parse_u16_token(t)).unwrap_or(0);
    let cro_id = tokens
        .get(2)
        .and_then(|t| parse_can_id_token(t))
        .unwrap_or(0);
    let dto_id = tokens
        .get(3)
        .and_then(|t| parse_can_id_token(t))
        .unwrap_or(0);
    let station_address = tokens.get(4).and_then(|t| parse_u16_token(t)).unwrap_or(0);

    let mut baudrate = None;
    let mut bytes_only = false;

    let mut i = 6;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "BAUDRATE" => {
                baudrate = tokens.get(i + 1).and_then(|t| parse_u32_token(t));
                i += 2;
            }
            "DAQ_MODE" => {
                // Unread by any command builder; skip the value.
                i += 2;
            }
            "BYTES_ONLY" => {
                bytes_only = true;
                i += 1;
            }
            "/begin" => {
                i += 2;
                let _ = extract_block(tokens, &mut i);
            }
            _ => i += 1,
        }
    }

    CcpInfo {
        version,
        cro_id,
        dto_id,
        station_address,
        baudrate,
        bytes_only,
        seed_key_dll: None,
    }
}
