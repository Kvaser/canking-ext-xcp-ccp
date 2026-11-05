//! Parsing the module-level `/begin MOD_COMMON` block: project-wide defaults that
//! apply regardless of which measurement protocol the file declares.

use super::ByteOrder;
use super::tokenizer::extract_block;

/// Extracts the default byte order from the first `/begin MOD_COMMON` block found.
pub(super) fn parse_mod_common(tokens: &[String]) -> Option<ByteOrder> {
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "/begin" && tokens.get(i + 1).map(String::as_str) == Some("MOD_COMMON") {
            i += 2;
            let block = extract_block(tokens, &mut i);
            let mut j = 0;
            while j < block.len() {
                if block[j] == "BYTE_ORDER" {
                    return block.get(j + 1).and_then(|t| match t.as_str() {
                        "MSB_FIRST" => Some(ByteOrder::MsbFirst),
                        "MSB_LAST" => Some(ByteOrder::MsbLast),
                        _ => None,
                    });
                }
                j += 1;
            }
            return None;
        }
        i += 1;
    }
    None
}
