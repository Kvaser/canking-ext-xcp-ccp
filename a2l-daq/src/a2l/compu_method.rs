//! Parsing `/begin COMPU_METHOD` blocks (and the `COMPU_VTAB`/`COMPU_VTAB_RANGE` they can reference) into [`CompuMethod`]s.

use super::formula::linear_from_formula_var;
use super::tokenizer::{blocks, parse_f64_token, value_after};
use super::{CompuMethod, VTabEntry};
use std::collections::HashMap;

/// Scans the token stream for all `/begin COMPU_METHOD` blocks
///
/// Returns a map of identifier -> `CompuMethod`, with any referenced `COMPU_VTAB`/`COMPU_VTAB_RANGE` resolved.
pub(super) fn parse_compu_methods(tokens: &[String]) -> HashMap<String, CompuMethod> {
    let vtabs = parse_compu_vtabs(tokens);
    blocks(tokens, "COMPU_METHOD")
        .filter_map(|b| b.split_first())
        .map(|(name, block)| {
            let (mut method, tab_ref) = parse_compu_method_block(block);
            if let Some((entries, default_text)) = tab_ref.and_then(|r| vtabs.get(&r)) {
                method.text_table = entries.clone();
                method.default_text = default_text.clone();
            }
            (name.clone(), method)
        })
        .collect()
}

/// Scans the token stream for all `/begin COMPU_VTAB` and `/begin COMPU_VTAB_RANGE` blocks
///
/// Returns a map of identifier -> (vector of `VTabEntry`, optional default text).
fn parse_compu_vtabs(tokens: &[String]) -> HashMap<String, (Vec<VTabEntry>, Option<String>)> {
    let plain = blocks(tokens, "COMPU_VTAB").map(|b| (b, false));
    let ranged = blocks(tokens, "COMPU_VTAB_RANGE").map(|b| (b, true));
    plain
        .chain(ranged)
        .filter_map(|(b, ranged)| {
            let (name, block) = b.split_first()?;
            Some((name.clone(), parse_vtab_block(block, ranged)))
        })
        .collect()
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

    // The keywords below are looked up in the whole block: the header tokens skipped above are quoted or `conv_type`.
    let mut factor = 1.0f64;
    let mut offset = 0.0f64;
    let mut tab_ref = None;
    let mut formula = None;
    let mut unsupported = None;

    match conv_type {
        "LINEAR" => {
            // physical = a * raw + b
            if let Some([a, b]) = coeffs(tokens, "COEFFS_LINEAR", [1.0, 0.0]) {
                (factor, offset) = (a, b);
            }
        }
        "RAT_FUNC" => {
            if let Some([a, b, c, d, e, f]) =
                coeffs(tokens, "COEFFS", [0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
            {
                if a == 0.0 && d == 0.0 && e == 0.0 && b != 0.0 {
                    // Linear case: raw = (b*phys + c) / f, inverted: physical = (f*raw - c) / b
                    (factor, offset) = (f / b, -c / b);
                } else {
                    unsupported = Some("non-linear RAT_FUNC".to_string());
                }
            }
        }
        "TAB_INTP" | "TAB_NOINTP" => {
            unsupported = Some(format!("{conv_type} numeric table"));
        }
        "TAB_VERB" => tab_ref = value_after(tokens, "COMPU_TAB_REF").cloned(),
        "FORM" => {
            formula = value_after(tokens, "FORMULA")
                .map(|t| t.trim_matches('"').trim().to_string())
                .filter(|t| !t.is_empty());
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

/// The `N` numbers after `keyword`, each falling back to its `defaults` entry
///
/// `None` if `keyword` is absent.
fn coeffs<const N: usize>(
    tokens: &[String],
    keyword: &str,
    defaults: [f64; N],
) -> Option<[f64; N]> {
    let at = tokens.iter().position(|t| t == keyword)?;
    Some(std::array::from_fn(|k| {
        tokens
            .get(at + 1 + k)
            .and_then(|t| t.parse().ok())
            .unwrap_or(defaults[k])
    }))
}
