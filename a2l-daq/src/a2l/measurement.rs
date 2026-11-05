//! Parsing `/begin MEASUREMENT ... /end MEASUREMENT` blocks into [`ParsedSignal`]s.

use super::datatype::A2lDataType;
use super::tokenizer::{extract_block, parse_u16_token, parse_u64_token};
use super::{ByteOrder, Conversion, ParsedSignal};

/// Parses the tokens of an A2L file and extracts all MEASUREMENT blocks, returning a vector of [`ParsedSignal`]s.
pub(super) fn parse_measurements(tokens: &[String], selected: &[&str]) -> Vec<ParsedSignal> {
    let mut signals = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        if tokens[i] == "/begin" && tokens.get(i + 1).map(String::as_str) == Some("MEASUREMENT") {
            i += 2;
            let name = match tokens.get(i) {
                Some(t) => t.clone(),
                None => break,
            };
            i += 1;
            let block = extract_block(tokens, &mut i);
            let include = selected.is_empty() || selected.iter().any(|&s| s == name);
            if include && let Some(signal) = parse_measurement_block(&name, block) {
                signals.push(signal);
            }
        } else {
            i += 1;
        }
    }

    signals
}

/// The token following `keyword`, unquoted, when the block names it and the value is non-empty.
fn keyword_value(tokens: &[String], keyword: &str) -> Option<String> {
    let at = tokens.iter().position(|t| t == keyword)?;
    tokens
        .get(at + 1)
        .map(|t| t.trim_matches('"').to_string())
        .filter(|t| !t.is_empty())
}

/// Parses the content tokens of a MEASUREMENT block.
///
/// Returns a `ParsedSignal` with the name, long identifier, conversion, limits, ECU address, size, byte order, bit mask, and virtual inputs/conversion.
fn parse_measurement_block(name: &str, tokens: &[String]) -> Option<ParsedSignal> {
    let mut pos = 0;

    // 1. Long Identifier
    let long_identifier = if tokens.get(pos).map(|t| t.starts_with('"')).unwrap_or(false) {
        let s = tokens[pos].trim_matches('"').to_string();
        pos += 1;
        Some(s)
    } else {
        None
    };

    // 2. Datatype
    let data_type = A2lDataType::from_token(tokens.get(pos)?)?;
    let base_size = data_type.size_bytes();
    let is_signed = data_type.is_signed();
    let is_float = data_type.is_float();
    pos += 1;

    // 3. Conversion identifier
    let conversion_identifier = tokens
        .get(pos)
        .filter(|t| A2lDataType::from_token(t).is_none())?
        .clone();
    pos += 1;

    // 4. Resolution, 5. Accuracy
    pos += 2;

    // 6. LowerLimit, 7. UpperLimit
    let lower_limit = tokens.get(pos).and_then(|t| t.parse::<f64>().ok())?;
    pos += 1;
    let upper_limit = tokens.get(pos).and_then(|t| t.parse::<f64>().ok())?;
    pos += 1;

    let mut ecu_address: Option<u64> = None;
    let mut address_extension: u8 = 0;
    let mut bit_mask: Option<u64> = None;
    let mut byte_order: Option<ByteOrder> = None;
    let mut array_size: Option<u16> = None;
    let mut virtual_inputs: Vec<String> = Vec::new();
    let mut virtual_conversion: Option<String> = None;

    while pos < tokens.len() {
        match tokens[pos].as_str() {
            "ECU_ADDRESS" => {
                ecu_address = tokens.get(pos + 1).and_then(|t| parse_u64_token(t));
                pos += 2;
            }
            "ECU_ADDRESS_EXTENSION" => {
                address_extension = tokens
                    .get(pos + 1)
                    .and_then(|t| parse_u64_token(t))
                    .and_then(|v| u8::try_from(v).ok())
                    .unwrap_or(0);
                pos += 2;
            }
            "BIT_MASK" => {
                bit_mask = tokens.get(pos + 1).and_then(|t| parse_u64_token(t));
                pos += 2;
            }
            "BYTE_ORDER" => {
                byte_order = tokens.get(pos + 1).and_then(|t| match t.as_str() {
                    "MSB_FIRST" => Some(ByteOrder::MsbFirst),
                    "MSB_LAST" => Some(ByteOrder::MsbLast),
                    _ => None,
                });
                pos += 2;
            }
            "ARRAY_SIZE" => {
                array_size = tokens.get(pos + 1).and_then(|t| parse_u16_token(t));
                pos += 2;
            }
            "MATRIX_DIM" => {
                pos += 1;
                let mut total: u16 = 1;
                let mut seen = false;
                while pos < tokens.len() {
                    if let Some(v) = parse_u16_token(&tokens[pos]) {
                        total = total.saturating_mul(v.max(1));
                        seen = true;
                        pos += 1;
                    } else {
                        break;
                    }
                }
                if seen {
                    array_size = Some(total);
                }
            }
            "FORMAT" => {
                // Display format is not used downstream; skip its value.
                pos += 2;
            }
            "/begin" => {
                let keyword = tokens.get(pos + 1).cloned().unwrap_or_default();
                pos += 2; // skip past `/begin KEYWORD`
                let block = extract_block(tokens, &mut pos);

                // A VIRTUAL block is nothing but the list of measurements the value is computed from.
                if keyword == "VIRTUAL" {
                    virtual_inputs.extend(block.iter().cloned());
                }
                if let Some(id) = keyword_value(block, "VIRTUAL_CONVERSION") {
                    virtual_conversion = Some(id);
                }
            }
            _ => {
                pos += 1;
            }
        }
    }

    let conversion = Some(Conversion {
        identifier: conversion_identifier,
    });

    let size = array_size
        .map(|n| base_size.saturating_mul(n))
        .unwrap_or(base_size);

    // A virtual measurement has no ECU_ADDRESS of its own
    let ecu_address = match ecu_address {
        Some(addr) => addr,
        None if !virtual_inputs.is_empty() => 0,
        None => return None,
    };

    Some(ParsedSignal {
        name: name.to_string(),
        long_identifier: long_identifier.unwrap_or_else(|| name.to_string()),
        conversion,
        lower_limit,
        upper_limit,
        ecu_address,
        address_extension,
        size,
        byte_order,
        bit_mask,
        is_signed,
        is_float,
        event: None,
        virtual_inputs,
        virtual_conversion,
    })
}
