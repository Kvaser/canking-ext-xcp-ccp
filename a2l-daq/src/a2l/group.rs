//! Parsing `/begin GROUP ... /end GROUP` blocks
//!
//! Grabs the group's name/description, whether it's a `ROOT` group, its declared
//! `SUB_GROUP`s, and the measurements it directly references.

use super::tokenizer::extract_block;

/// A MEASUREMENT GROUP as declared in the A2L.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ParsedGroup {
    pub name: String,
    pub description: String,
    pub root: bool,
    pub sub_groups: Vec<String>,
    pub measurements: Vec<String>,
}

/// Parses every `/begin GROUP ... /end GROUP` block in the file.
pub(super) fn parse_groups(tokens: &[String]) -> Vec<ParsedGroup> {
    let mut groups = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        if tokens[i] == "/begin" && tokens.get(i + 1).map(String::as_str) == Some("GROUP") {
            i += 2;
            let name = match tokens.get(i) {
                Some(t) => t.clone(),
                None => break,
            };
            i += 1;
            let block = extract_block(tokens, &mut i);
            groups.push(parse_group_block(&name, block));
        } else {
            i += 1;
        }
    }

    groups
}

/// Parses the content tokens of a GROUP block.
fn parse_group_block(name: &str, tokens: &[String]) -> ParsedGroup {
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

    ParsedGroup {
        name: name.to_string(),
        description,
        root,
        sub_groups,
        measurements,
    }
}

// -------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::tokenizer::tokenize_checked;
    use super::parse_groups;

    #[test]
    fn parses_root_and_sub_group_with_measurements() {
        let content = r#"
        /begin GROUP Engine "Engine related measurements"
          ROOT
          /begin SUB_GROUP Engine_Sensors /end SUB_GROUP
        /end GROUP
        /begin GROUP Engine_Sensors "Engine sensor measurements"
          /begin REF_MEASUREMENT Rpm Coolant_Temp /end REF_MEASUREMENT
        /end GROUP
        "#;
        let tokens = tokenize_checked(content).unwrap();
        let groups = parse_groups(&tokens);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Engine");
        assert_eq!(groups[0].description, "Engine related measurements");
        assert!(groups[0].root);
        assert_eq!(groups[0].sub_groups, ["Engine_Sensors"]);
        assert!(groups[0].measurements.is_empty());

        assert_eq!(groups[1].name, "Engine_Sensors");
        assert!(!groups[1].root);
        assert_eq!(groups[1].measurements, ["Rpm", "Coolant_Temp"]);
    }
}
