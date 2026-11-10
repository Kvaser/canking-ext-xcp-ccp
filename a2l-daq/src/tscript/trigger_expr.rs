//! Parsing a textual trigger expression into the parts [`crate::tscript`] needs.
//!
//! The CLI takes a trigger as one line of text (`Rpm.Phys > 3000 && Temp.Phys < 90`).
//! This module turns that into [`TriggerTerm`]s and then resolves them against the DAQ layout

use super::inline_signals::decoders_for;
use super::inline_signals::{SignalDecoder, TriggerError};
use super::trigger::{
    Accessor, CompareOp, Comparison, TriggerSignal, dbc_trigger_signals, inline_trigger_signals,
};
use crate::a2l::A2lProject;
use crate::daq::DaqList;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// One comparison of a trigger expression: which signal, read how, against what.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerTerm {
    /// The signal name as written, before sanitisation.
    pub signal: String,
    pub accessor: Accessor,
    pub op: CompareOp,
    /// The right-hand side, already validated as a number and kept as written.
    pub value: String,
}

/// A trigger resolved against the DAQ layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTrigger {
    /// The values the script latches: one per distinct read.
    pub signals: Vec<TriggerSignal>,
    /// The comparisons over those latches, one per term, in the order written.
    pub comparisons: Vec<Comparison>,
    /// Decoder functions the script must define for itself. Empty in DBC mode.
    pub decoders: Vec<SignalDecoder>,
}

/// Why a trigger expression could not be turned into a script trigger.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerExprError {
    /// A `&&`-separated term was blank.
    EmptyTerm,
    /// A term contains none of the comparison operators.
    NoOperator { term: String },
    /// A term's right-hand side is not a number.
    NotANumber { term: String, value: String },
    /// A term's left-hand side is empty once the accessor is stripped.
    NoSignal { term: String },
    /// A `.Raw` read whose signal is scaled, in a script that decodes for itself.
    ScaledRawRead {
        name: String,
        factor: f64,
        offset: f64,
    },
    /// The reads could not be resolved against the DAQ layout.
    Unresolved(TriggerError),
}

impl Display for TriggerExprError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTerm => f.write_str("empty term"),
            Self::NoOperator { term } => write!(f, "no comparison operator in '{term}'"),
            Self::NotANumber { term, value } => {
                write!(f, "'{value}' in '{term}' is not a number")
            }
            Self::NoSignal { term } => write!(f, "no signal name in '{term}'"),
            Self::ScaledRawRead {
                name,
                factor,
                offset,
            } => write!(
                f,
                "'{name}.Raw' is scaled (phys = raw * {factor} + {offset}); a script-decoded \
                 trigger compares physical values only. Use '{name}.Phys', or DBC mode."
            ),
            Self::Unresolved(e) => Display::fmt(e, f),
        }
    }
}

impl Error for TriggerExprError {}

impl From<TriggerError> for TriggerExprError {
    fn from(e: TriggerError) -> Self {
        Self::Unresolved(e)
    }
}

/// Parse `SIGNAL.Phys <op> <number>` terms joined by `&&`.
///
/// A leading `this.` and the `.Phys`/`.Raw` suffix are both optional; a bare name reads
/// the physical value.
pub fn parse_terms(expr: &str) -> Result<Vec<TriggerTerm>, TriggerExprError> {
    let mut terms = Vec::new();
    for raw in expr.split("&&") {
        let term = raw.trim();
        if term.is_empty() {
            return Err(TriggerExprError::EmptyTerm);
        }

        // `CompareOp::ALL` is ordered longest-symbol-first.
        let (op, lhs, rhs) = CompareOp::ALL
            .iter()
            .find_map(|op| term.split_once(op.symbol()).map(|(l, r)| (*op, l, r)))
            .ok_or_else(|| TriggerExprError::NoOperator {
                term: term.to_string(),
            })?;

        let value = rhs.trim();
        if value.parse::<f64>().is_err() {
            return Err(TriggerExprError::NotANumber {
                term: term.to_string(),
                value: value.to_string(),
            });
        }

        let mut signal = lhs.trim().trim_start_matches("this.").trim();
        let mut accessor = Accessor::Phys;
        if let Some(base) = signal.strip_suffix(".Phys") {
            signal = base;
        } else if let Some(base) = signal.strip_suffix(".Raw") {
            signal = base;
            accessor = Accessor::Raw;
        }
        if signal.is_empty() {
            return Err(TriggerExprError::NoSignal {
                term: term.to_string(),
            });
        }

        terms.push(TriggerTerm {
            signal: signal.to_string(),
            accessor,
            op,
            value: value.to_string(),
        });
    }
    Ok(terms)
}

/// Resolve parsed terms into the pieces a [`crate::TriggerCondition`] is built from.
///
/// With `inline` decoding the script currently computes physical values itself.
/// `default_can_id` is only used inline, to resolve a list's CAN id when it declares no override of its own.
/// Pass anything when `inline` is `false`.
pub fn resolve_terms(
    terms: &[TriggerTerm],
    inline: bool,
    project: &A2lProject,
    daq_lists: &[DaqList],
    default_can_id: u32,
) -> Result<ResolvedTrigger, TriggerExprError> {
    let effective = |t: &TriggerTerm| if inline { Accessor::Phys } else { t.accessor };

    let reads: Vec<(String, Accessor)> = terms
        .iter()
        .map(|t| (t.signal.clone(), effective(t)))
        .collect();
    let comparisons: Vec<Comparison> = terms
        .iter()
        .map(|t| Comparison {
            ident: effective(t).latch_ident(&t.signal),
            op: t.op,
            value: t.value.clone(),
        })
        .collect();

    let (signals, decoders) = if inline {
        let names: Vec<String> = reads.iter().map(|(name, _)| name.clone()).collect();
        let decoders = decoders_for(project, daq_lists, &names, default_can_id)?;
        reject_scaled_raw_reads(terms, &decoders)?;
        (inline_trigger_signals(&decoders), decoders)
    } else {
        (dbc_trigger_signals(project, daq_lists, &reads)?, Vec::new())
    };
    Ok(ResolvedTrigger {
        signals,
        comparisons,
        decoders,
    })
}

/// Parse and resolve in one step.
pub fn build_trigger(
    expr: &str,
    inline: bool,
    project: &A2lProject,
    daq_lists: &[DaqList],
    default_can_id: u32,
) -> Result<ResolvedTrigger, TriggerExprError> {
    resolve_terms(
        &parse_terms(expr)?,
        inline,
        project,
        daq_lists,
        default_can_id,
    )
}

/// Error on any term that asked for `.Raw` but whose decoder applies a scaling, since the
/// physical value it was coerced to is a different number than the user wrote.
fn reject_scaled_raw_reads(
    terms: &[TriggerTerm],
    decoders: &[SignalDecoder],
) -> Result<(), TriggerExprError> {
    for term in terms.iter().filter(|t| t.accessor == Accessor::Raw) {
        let ident = Accessor::Phys.latch_ident(&term.signal);
        let Some(d) = decoders.iter().find(|d| d.ident == ident) else {
            continue;
        };
        if d.factor != 1.0 || d.offset != 0.0 {
            return Err(TriggerExprError::ScaledRawRead {
                name: term.signal.clone(),
                factor: d.factor,
                offset: d.offset,
            });
        }
    }
    Ok(())
}

/// Replace `N.Phys` / `N.Raw` index references with the Nth name in `names`.
///
/// The CLI lists the available signals numbered, so a trigger can name a signal by its
/// index instead of retyping it. An index with no matching name is left as written.
pub fn expand_signal_indices(expr: &str, names: &[&str]) -> String {
    let mut out = String::with_capacity(expr.len());
    let mut rest = expr;

    while !rest.is_empty() {
        let starts_token = out
            .chars()
            .last()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let digits_len = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());

        if starts_token && digits_len > 0 {
            let (digits, after) = rest.split_at(digits_len);
            match accessor_suffix(after).zip(index_into(names, digits)) {
                Some((suffix, name)) => {
                    out.push_str(name);
                    out.push_str(suffix);
                    rest = &after[suffix.len()..];
                }
                None => {
                    out.push_str(digits);
                    rest = after;
                }
            }
            continue;
        }

        // Advance one whole character
        let c = rest.chars().next().expect("rest is non-empty");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// The accessor suffix `rest` opens with, when it forms a whole token there.
fn accessor_suffix(rest: &str) -> Option<&'static str> {
    [".Phys", ".Raw"].into_iter().find(|suffix| {
        rest.strip_prefix(suffix)
            .is_some_and(|tail| !tail.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

/// The 1-based `digits`th entry of `names`.
fn index_into<'a>(names: &[&'a str], digits: &str) -> Option<&'a str> {
    let index = digits.parse::<usize>().ok()?;
    names.get(index.checked_sub(1)?).copied()
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tscript::test_util::two_odt_project;

    const TEST_CAN_ID: u32 = 0x456;

    fn term(signal: &str, accessor: Accessor, op: &str, value: &str) -> TriggerTerm {
        TriggerTerm {
            signal: signal.into(),
            accessor,
            op: CompareOp::from_symbol(op).expect("operator"),
            value: value.into(),
        }
    }

    #[test]
    fn parses_a_single_term() {
        assert_eq!(
            parse_terms("Rpm.Phys > 3000").unwrap(),
            [term("Rpm", Accessor::Phys, ">", "3000")]
        );
    }

    #[test]
    fn parses_conjunctions_and_both_accessors() {
        assert_eq!(
            parse_terms("Rpm.Phys >= 3000 && Temp.Raw != -5").unwrap(),
            [
                term("Rpm", Accessor::Phys, ">=", "3000"),
                term("Temp", Accessor::Raw, "!=", "-5"),
            ]
        );
    }

    #[test]
    fn a_bare_name_reads_the_physical_value() {
        assert_eq!(
            parse_terms("Rpm < 10").unwrap(),
            [term("Rpm", Accessor::Phys, "<", "10")]
        );
    }

    #[test]
    fn a_leading_this_is_optional() {
        assert_eq!(
            parse_terms("this.Rpm.Phys == 1").unwrap(),
            [term("Rpm", Accessor::Phys, "==", "1")]
        );
    }

    #[test]
    fn two_char_operators_are_not_split_as_one_char_ones() {
        for symbol in [">=", "<=", "==", "!="] {
            let parsed = parse_terms(&format!("Rpm.Phys {symbol} 5")).unwrap();
            assert_eq!(parsed[0].op.symbol(), symbol, "for '{symbol}'");
            assert_eq!(parsed[0].value, "5", "for '{symbol}'");
        }
    }

    #[test]
    fn rejects_malformed_terms() {
        assert_eq!(
            parse_terms("Rpm.Phys 3000"),
            Err(TriggerExprError::NoOperator {
                term: "Rpm.Phys 3000".into()
            })
        );
        assert_eq!(
            parse_terms("Rpm.Phys > fast"),
            Err(TriggerExprError::NotANumber {
                term: "Rpm.Phys > fast".into(),
                value: "fast".into()
            })
        );
        assert_eq!(
            parse_terms(".Phys > 1"),
            Err(TriggerExprError::NoSignal {
                term: ".Phys > 1".into()
            })
        );
        assert_eq!(parse_terms("Rpm > 1 && "), Err(TriggerExprError::EmptyTerm));
    }

    #[test]
    fn error_messages_name_the_offending_term() {
        let e = parse_terms("Rpm.Phys > fast").unwrap_err();
        assert!(e.to_string().contains("'fast'"), "got: {e}");
    }

    #[test]
    fn expands_indices_to_names() {
        let names = ["Rpm", "Temp"];
        assert_eq!(
            expand_signal_indices("1.Phys > 3000 && 2.Raw < 90", &names),
            "Rpm.Phys > 3000 && Temp.Raw < 90"
        );
    }

    #[test]
    fn leaves_plain_numbers_alone() {
        let names = ["Rpm"];
        assert_eq!(
            expand_signal_indices("Rpm.Phys > 3000", &names),
            "Rpm.Phys > 3000"
        );
        assert_eq!(expand_signal_indices("1 > 2", &names), "1 > 2");
    }

    #[test]
    fn leaves_out_of_range_indices_as_written() {
        assert_eq!(expand_signal_indices("9.Phys > 1", &["Rpm"]), "9.Phys > 1");
        assert_eq!(expand_signal_indices("0.Phys > 1", &["Rpm"]), "0.Phys > 1");
    }

    #[test]
    fn a_digit_inside_a_name_is_not_an_index() {
        let names = ["Rpm", "Temp"];
        assert_eq!(
            expand_signal_indices("Sensor1.Phys > 5", &names),
            "Sensor1.Phys > 5"
        );
    }

    #[test]
    fn non_ascii_names_survive_expansion() {
        let names = ["Kühlmittel", "Drehzahl"];
        assert_eq!(
            expand_signal_indices("1.Phys > 90 && Öl.Raw < 2", &names),
            "Kühlmittel.Phys > 90 && Öl.Raw < 2"
        );
    }

    #[test]
    fn dbc_mode_keeps_raw_and_phys_comparisons_on_separate_latches() {
        let (project, lists) = two_odt_project();
        let terms = parse_terms("Temp.Phys > 10 && Temp.Raw < 500").unwrap();
        let ResolvedTrigger {
            signals,
            comparisons,
            decoders,
        } = resolve_terms(&terms, false, &project, &lists, TEST_CAN_ID).unwrap();

        assert!(decoders.is_empty(), "DBC mode generates no decoders");
        let idents: Vec<&str> = comparisons.iter().map(|c| c.ident.as_str()).collect();
        assert_eq!(idents, ["Temp", "Temp_raw"]);
        for c in &comparisons {
            assert!(
                signals.iter().any(|s| s.ident == c.ident),
                "no signal fills latch '{}'",
                c.ident
            );
        }
    }

    #[test]
    fn comparison_idents_stay_aligned_when_reads_collapse() {
        let (project, lists) = two_odt_project();
        let terms = parse_terms("Rpm.Phys > 1000 && Rpm.Phys < 5000").unwrap();
        let r = resolve_terms(&terms, false, &project, &lists, TEST_CAN_ID).unwrap();
        assert_eq!(r.signals.len(), 1);
        assert_eq!(r.comparisons.len(), 2);
        assert!(r.comparisons.iter().all(|c| c.ident == "Rpm"));
    }

    #[test]
    fn inline_mode_rejects_a_scaled_raw_read() {
        let (project, lists) = two_odt_project();
        let terms = parse_terms("Temp.Raw > 500").unwrap();
        let err = resolve_terms(&terms, true, &project, &lists, TEST_CAN_ID).unwrap_err();
        assert_eq!(
            err,
            TriggerExprError::ScaledRawRead {
                name: "Temp".into(),
                factor: 0.1,
                offset: -40.0
            }
        );
        assert!(err.to_string().contains("Use 'Temp.Phys'"), "got: {err}");
    }

    #[test]
    fn inline_mode_accepts_an_unscaled_raw_read() {
        let (project, lists) = two_odt_project();
        let terms = parse_terms("Rpm.Raw > 3000").unwrap();
        let r = resolve_terms(&terms, true, &project, &lists, TEST_CAN_ID).unwrap();
        assert_eq!(r.decoders.len(), 1);
        assert_eq!(r.comparisons[0].ident, "Rpm");
        assert_eq!(r.signals[0].ident, "Rpm");
        assert_eq!(r.signals[0].read, "sig_Rpm(this.data)");
    }

    #[test]
    fn a_signal_no_frame_carries_is_reported() {
        let (project, lists) = two_odt_project();
        let terms = parse_terms("Ghost.Phys > 1").unwrap();
        let err = resolve_terms(&terms, false, &project, &lists, TEST_CAN_ID).unwrap_err();
        assert!(
            matches!(
                err,
                TriggerExprError::Unresolved(TriggerError::NotInDaqList { .. })
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn build_trigger_parses_and_resolves_in_one_step() {
        let (project, lists) = two_odt_project();
        let r = build_trigger(
            "Rpm.Phys > 3000 && Temp.Phys < 90",
            false,
            &project,
            &lists,
            TEST_CAN_ID,
        )
        .unwrap();
        assert_eq!(r.signals.len(), 2);
        assert_eq!(r.comparisons.len(), 2);
        assert_eq!((r.signals[0].pid, r.signals[1].pid), (1, 2));
    }
}
