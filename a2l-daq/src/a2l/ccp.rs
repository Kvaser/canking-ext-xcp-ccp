//! Parsing the module-level `/begin IF_DATA ASAP1B_CCP ... /end IF_DATA` block: transport
//! parameters (`TP_BLOB`), event rasters, and DAQ list sources (`SOURCE`/`QP_BLOB`).

use super::tokenizer::{extract_block, parse_u16_token, parse_u32_token};
use super::{CcpData, CcpInfo, CcpSource, EventEntry};

/// Finds the module-level `IF_DATA ASAP1B_CCP` block and parses its contents.
///
/// Returns a `CcpData`, or `None` if the file declares no CCP transport.
pub(super) fn parse_if_data_ccp(tokens: &[String]) -> Option<CcpData> {
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "/begin"
            && tokens.get(i + 1).map(String::as_str) == Some("IF_DATA")
            && tokens.get(i + 2).map(String::as_str) == Some("ASAP1B_CCP")
        {
            i += 3; // consume /begin IF_DATA ASAP1B_CCP
            let block = extract_block(tokens, &mut i);
            if block_has_begin(block, "TP_BLOB") {
                return Some(parse_ccp_contents(block));
            }
        } else {
            i += 1;
        }
    }
    None
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

    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "/begin" {
            let keyword = tokens.get(i + 1).map(String::as_str).unwrap_or("");
            i += 2;
            let block = extract_block(tokens, &mut i);
            match keyword {
                "RASTER" => {
                    if let Some(r) = parse_ccp_raster(block) {
                        rasters.push(r);
                    }
                }
                "SOURCE" => {
                    if let Some(s) = parse_ccp_source(block) {
                        sources.push(s);
                    }
                }
                "TP_BLOB" => {
                    // Preserve a SEED_KEY parsed before TP_BLOB was reached; block order isn't guaranteed.
                    let seed_key_dll = info.seed_key_dll.take();
                    info = parse_tp_blob(block);
                    info.seed_key_dll = seed_key_dll;
                }
                "SEED_KEY" => info.seed_key_dll = parse_seed_key(block),
                _ => {} // EVENT_GROUP, CHECKSUM
            }
        } else {
            i += 1;
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

/// Parses a `/begin RASTER` block into an [`EventEntry`].
///
/// Return an `EventEntry` with the channel number, name, and optional time cycle/unit.
fn parse_ccp_raster(tokens: &[String]) -> Option<EventEntry> {
    let name = tokens
        .first()
        .filter(|t| t.starts_with('"'))
        .map(|t| t.trim_matches('"').to_string())?;
    let channel = parse_u32_token(tokens.get(2)?)?;
    let time_unit = tokens.get(3).and_then(|t| parse_u32_token(t)).unwrap_or(0);
    let time_cycle = tokens.get(4).and_then(|t| parse_u32_token(t)).unwrap_or(0);

    Some(EventEntry {
        channel,
        name,
        time_cycle,
        time_unit,
    })
}

/// Parses a `/begin SOURCE` block
///
/// Returns a `CcpSource` with the DAQ list number and tagged parameters.
fn parse_ccp_source(tokens: &[String]) -> Option<CcpSource> {
    let name = tokens
        .first()
        .filter(|t| t.starts_with('"'))
        .map(|t| t.trim_matches('"').to_string())?;

    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "/begin" {
            let keyword = tokens.get(i + 1).map(String::as_str).unwrap_or("");
            i += 2;
            let block = extract_block(tokens, &mut i);
            if keyword == "QP_BLOB" {
                return parse_qp_blob(name, block);
            }
        } else {
            i += 1;
        }
    }
    None
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
                source.can_id = tokens.get(i + 1).and_then(|t| parse_u32_token(t));
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
    let cro_id = tokens.get(2).and_then(|t| parse_u32_token(t)).unwrap_or(0);
    let dto_id = tokens.get(3).and_then(|t| parse_u32_token(t)).unwrap_or(0);
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
