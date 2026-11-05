//! Parsing `/begin COMPU_METHOD` blocks (and the `COMPU_VTAB`/`COMPU_VTAB_RANGE` they can reference) into [`CompuMethod`]s.

use super::formula::linear_from_formula_var;
use super::tokenizer::{extract_block, parse_f64_token};
use super::{CompuMethod, VTabEntry};
use std::collections::HashMap;

/// Scans the token stream for all `/begin COMPU_METHOD` blocks
///
/// Returns a map of identifier -> `CompuMethod`, with any referenced `COMPU_VTAB`/`COMPU_VTAB_RANGE` resolved.
pub(super) fn parse_compu_methods(tokens: &[String]) -> HashMap<String, CompuMethod> {
    let vtabs = parse_compu_vtabs(tokens);
    let mut map = HashMap::new();
    let mut i = 0;

    while i < tokens.len() {
        if tokens[i] == "/begin" && tokens.get(i + 1).map(String::as_str) == Some("COMPU_METHOD") {
            i += 2;
            let name = match tokens.get(i) {
                Some(t) => t.clone(),
                None => break,
            };
            i += 1;
            let block = extract_block(tokens, &mut i);
            let (mut method, tab_ref) = parse_compu_method_block(block);
            if let Some((entries, default_text)) = tab_ref.and_then(|r| vtabs.get(&r)) {
                method.text_table = entries.clone();
                method.default_text = default_text.clone();
            }
            map.insert(name, method);
        } else {
            i += 1;
        }
    }

    map
}

/// Scans the token stream for all `/begin COMPU_VTAB` and `/begin COMPU_VTAB_RANGE` blocks
///
/// Returns a map of identifier -> (vector of `VTabEntry`, optional default text).
fn parse_compu_vtabs(tokens: &[String]) -> HashMap<String, (Vec<VTabEntry>, Option<String>)> {
    let mut map = HashMap::new();
    let mut i = 0;

    while i < tokens.len() {
        if tokens[i] == "/begin" {
            let keyword = tokens.get(i + 1).map(String::as_str).unwrap_or("");
            if keyword == "COMPU_VTAB" || keyword == "COMPU_VTAB_RANGE" {
                let ranged = keyword == "COMPU_VTAB_RANGE";
                i += 2;
                let name = match tokens.get(i) {
                    Some(t) => t.clone(),
                    None => break,
                };
                i += 1;
                let block = extract_block(tokens, &mut i);
                map.insert(name, parse_vtab_block(block, ranged));
                continue;
            }
        }
        i += 1;
    }

    map
}

/// Parses the content tokens of a COMPU_VTAB or COMPU_VTAB_RANGE block.
///
/// Returns a vector of `VTabEntry`s with the lower/upper bounds and text, and an optional default text.
fn parse_vtab_block(tokens: &[String], ranged: bool) -> (Vec<VTabEntry>, Option<String>) {
    let mut pos = 0;

    if tokens.get(pos).map(|t| t.starts_with('"')).unwrap_or(false) {
        pos += 1;
    }
    if tokens.get(pos).map(String::as_str) == Some("TAB_VERB") {
        pos += 1;
    }
    if tokens.get(pos).and_then(|t| parse_f64_token(t)).is_some() {
        pos += 1;
    }

    let mut entries = Vec::new();
    let mut default_text = None;

    while pos < tokens.len() {
        if tokens[pos] == "DEFAULT_VALUE" {
            default_text = tokens.get(pos + 1).map(|t| t.trim_matches('"').to_string());
            pos += 2;
            continue;
        }

        let lower = match parse_f64_token(&tokens[pos]) {
            Some(v) => v,
            None => {
                pos += 1;
                continue;
            }
        };
        pos += 1;

        let upper = if ranged {
            match tokens.get(pos).and_then(|t| parse_f64_token(t)) {
                Some(v) => {
                    pos += 1;
                    v
                }
                None => break,
            }
        } else {
            lower
        };

        let text = match tokens.get(pos).filter(|t| t.starts_with('"')) {
            Some(t) => t.trim_matches('"').to_string(),
            None => break,
        };
        pos += 1;

        entries.push(VTabEntry { lower, upper, text });
    }

    (entries, default_text)
}

/// Parses the content tokens of a COMPU_METHOD block.
///
/// Returns a `CompuMethod` with the factor/offset/unit, and an optional reference to a `COMPU_VTAB`/`COMPU_VTAB_RANGE`.
fn parse_compu_method_block(tokens: &[String]) -> (CompuMethod, Option<String>) {
    let mut pos = 0;

    if tokens.get(pos).map(|t| t.starts_with('"')).unwrap_or(false) {
        pos += 1;
    }

    let conv_type = tokens.get(pos).map(String::as_str).unwrap_or("");
    pos += 1;

    if tokens.get(pos).map(|t| t.starts_with('"')).unwrap_or(false) {
        pos += 1;
    }

    let unit = tokens
        .get(pos)
        .filter(|t| t.starts_with('"'))
        .map(|t| t.trim_matches('"').to_string())
        .unwrap_or_default();
    if tokens.get(pos).map(|t| t.starts_with('"')).unwrap_or(false) {
        pos += 1;
    }

    let mut factor = 1.0f64;
    let mut offset = 0.0f64;
    let mut tab_ref = None;
    let mut formula = None;
    let mut unsupported = None;

    match conv_type {
        "LINEAR" => {
            while pos < tokens.len() {
                if tokens[pos] == "COEFFS_LINEAR" {
                    // physical = a * raw + b
                    factor = tokens
                        .get(pos + 1)
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(1.0);
                    offset = tokens
                        .get(pos + 2)
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(0.0);
                    break;
                }
                pos += 1;
            }
        }
        "RAT_FUNC" => {
            while pos < tokens.len() {
                if tokens[pos] == "COEFFS" {
                    let a: f64 = tokens
                        .get(pos + 1)
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(0.0);
                    let b: f64 = tokens
                        .get(pos + 2)
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(1.0);
                    let c: f64 = tokens
                        .get(pos + 3)
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(0.0);
                    let d: f64 = tokens
                        .get(pos + 4)
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(0.0);
                    let e: f64 = tokens
                        .get(pos + 5)
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(0.0);
                    let f: f64 = tokens
                        .get(pos + 6)
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(1.0);
                    if a == 0.0 && d == 0.0 && e == 0.0 && b != 0.0 {
                        // Linear case: raw = (b*phys + c) / f, inverted: physical = (f*raw - c) / b
                        factor = f / b;
                        offset = -c / b;
                    } else {
                        unsupported = Some("non-linear RAT_FUNC".to_string());
                    }
                    break;
                }
                pos += 1;
            }
        }
        "TAB_INTP" | "TAB_NOINTP" => {
            unsupported = Some(format!("{conv_type} numeric table"));
        }
        "TAB_VERB" => {
            while pos < tokens.len() {
                if tokens[pos] == "COMPU_TAB_REF" {
                    tab_ref = tokens.get(pos + 1).cloned();
                    break;
                }
                pos += 1;
            }
        }
        "FORM" => {
            while pos < tokens.len() {
                if tokens[pos] == "FORMULA" {
                    formula = tokens
                        .get(pos + 1)
                        .map(|t| t.trim_matches('"').trim().to_string())
                        .filter(|t| !t.is_empty());
                    break;
                }
                pos += 1;
            }
            // X1 is the raw value for an ordinary measurement but virtual ones have their FORM separately.
            match formula.as_deref().and_then(linear_from_formula_var) {
                Some((None | Some(1), f, o)) => (factor, offset) = (f, o),
                _ => {
                    unsupported = Some(format!(
                        "FORM '{}' is not linear in X1",
                        formula.as_deref().unwrap_or("")
                    ))
                }
            }
        }
        _ => {} // IDENTICAL
    }

    (
        CompuMethod {
            factor,
            offset,
            unit,
            text_table: Vec::new(),
            default_text: None,
            formula,
            unsupported,
        },
        tab_ref,
    )
}
