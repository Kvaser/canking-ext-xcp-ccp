//! Parsing `/begin GROUP ... /end GROUP` blocks
//!
//! Grabs the group's name/description, whether it's a `ROOT` group, its declared
//! `SUB_GROUP`s, and the measurements it directly references.

use super::GroupInfo;
use super::tokenizer::{blocks, extract_block};

/// Parses every `/begin GROUP ... /end GROUP` block in the file, each with the measurements it directly references.
pub(super) fn parse_groups(tokens: &[String]) -> Vec<(GroupInfo, Vec<String>)> {
    blocks(tokens, "GROUP")
        .filter_map(|b| b.split_first())
        .map(|(name, block)| parse_group_block(name, block))
        .collect()
}

/// Parses the content tokens of a GROUP block.
fn parse_group_block(name: &str, tokens: &[String]) -> (GroupInfo, Vec<String>) {
    let mut pos = 0;

    let description = if tokens.get(pos).map(|t| t.starts_with('"')).unwrap_or(false) {
        let s = tokens[pos].trim_matches('"').to_string();
        pos += 1;
        s
    } else {
        String::new()
    };

    let mut root = false;
    let mut sub_groups = Vec::new();
    let mut measurements = Vec::new();

    while pos < tokens.len() {
        match tokens[pos].as_str() {
            "ROOT" => {
                root = true;
                pos += 1;
            }
            "/begin" => {
                let keyword = tokens.get(pos + 1).cloned().unwrap_or_default();
                pos += 2;
                let block = extract_block(tokens, &mut pos);
                match keyword.as_str() {
                    "SUB_GROUP" => sub_groups.extend(block.iter().cloned()),
                    "REF_MEASUREMENT" => measurements.extend(block.iter().cloned()),
                    _ => {}
                }
            }
            _ => {
                pos += 1;
            }
        }
    }

    let group = GroupInfo {
        name: name.to_string(),
        description,
        root,
        sub_groups,
    };
    (group, measurements)
}
