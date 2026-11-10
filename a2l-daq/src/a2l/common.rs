//! Parsing the module-level `/begin MOD_COMMON` block: project-wide defaults that
//! apply regardless of which measurement protocol the file declares.

use super::ByteOrder;
use super::tokenizer::{blocks, value_after};

/// Extracts the default byte order from the first `/begin MOD_COMMON` block found.
pub(super) fn parse_mod_common(tokens: &[String]) -> Option<ByteOrder> {
    let block = blocks(tokens, "MOD_COMMON").next()?;
    parse_byte_order(value_after(block, "BYTE_ORDER")?)
}

/// A `BYTE_ORDER` value: `MSB_FIRST` or `MSB_LAST`.
pub(super) fn parse_byte_order(token: &str) -> Option<ByteOrder> {
    match token {
        "MSB_FIRST" => Some(ByteOrder::MsbFirst),
        "MSB_LAST" => Some(ByteOrder::MsbLast),
        _ => None,
    }
}
