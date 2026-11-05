//! Rewriting single-input virtual measurements into ordinary measurable signals.

use super::formula::linear_from_formula_var;
use super::{A2lProject, CompuMethod, Conversion, ParsedSignal};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display, Formatter};

/// Outcome of trying to turn a virtual measurement into a measurable DAQ signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectedVirtualReason {
    MultipleInputs { inputs: Vec<String> },
    UnknownInput { input: String },
    InputOutOfRange { index: usize, available: usize },
    NonLinearFormula { formula: String },
    MissingFormula,
}

impl Display for RejectedVirtualReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultipleInputs { inputs } => write!(
                f,
                "computed from {} inputs ({}); only single-input virtuals can be measured directly - select those inputs instead",
                inputs.len(),
                inputs.join(", ")
            ),
            Self::UnknownInput { input } => {
                write!(f, "input '{input}' has no addressable MEASUREMENT")
            }
            Self::InputOutOfRange { index, available } => write!(
                f,
                "formula uses X{index} but the VIRTUAL block lists only {available} input(s)"
            ),
            Self::NonLinearFormula { formula } => {
                write!(f, "formula '{formula}' is not linear in a single input")
            }
            Self::MissingFormula => f.write_str("no FORMULA to convert its input with"),
        }
    }
}

impl RejectedVirtualReason {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::MultipleInputs { .. } => "multiple inputs",
            Self::UnknownInput { .. } => "unknown input",
            Self::InputOutOfRange { .. } => "formula input out of range",
            Self::NonLinearFormula { .. } => "non-linear formula",
            Self::MissingFormula => "no formula",
        }
    }
}

/// A virtual measurement that could not be turned into a measurable signal.
#[derive(Debug, Clone, PartialEq)]
pub struct RejectedVirtual {
    pub signal: ParsedSignal,
    pub reason: RejectedVirtualReason,
}

/// Rewrite single-input virtual measurements into ordinary measurable signals.
///
/// The ones that cannot be rewritten are removed from `project` and returned.
pub(super) fn resolve_virtual_signals(project: &mut A2lProject) -> Vec<RejectedVirtual> {
    let real: HashMap<String, ParsedSignal> = project
        .signals
        .iter()
        .filter(|s| s.virtual_inputs.is_empty())
        .map(|s| (s.name.clone(), s.clone()))
        .collect();

    let mut rejected: Vec<RejectedVirtual> = Vec::new();
    let mut synthetic: Vec<(String, CompuMethod)> = Vec::new();

    for sig in &mut project.signals {
        if sig.virtual_inputs.is_empty() {
            continue;
        }
        let own = sig
            .conversion
            .as_ref()
            .and_then(|c| project.compu_methods.get(&c.identifier));

        let reduced = own
            .and_then(|cm| cm.formula.as_deref())
            .and_then(linear_from_formula_var);

        let input_name = match reduced {
            Some((Some(index), ..)) => match sig.virtual_inputs.get(index - 1) {
                Some(name) => name.clone(),
                None => {
                    // Reject virtual signals that reference an input index that doesn't exist.
                    rejected.push(RejectedVirtual {
                        reason: RejectedVirtualReason::InputOutOfRange {
                            index,
                            available: sig.virtual_inputs.len(),
                        },
                        signal: sig.clone(),
                    });
                    continue;
                }
            },
            // Check that the virtual signal has exactly one input, and use that as the input name.
            // Reject virtual signals with multiple inputs, since we (currently) cannot resolve them into a single measurable signal.
            _ if sig.virtual_inputs.len() == 1 => sig.virtual_inputs[0].clone(),
            _ => {
                rejected.push(RejectedVirtual {
                    reason: RejectedVirtualReason::MultipleInputs {
                        inputs: sig.virtual_inputs.clone(),
                    },
                    signal: sig.clone(),
                });
                continue;
            }
        };

        // Look up the input signal by name, and reject if it doesn't exist.
        let Some(input) = real.get(&input_name) else {
            rejected.push(RejectedVirtual {
                reason: RejectedVirtualReason::UnknownInput { input: input_name },
                signal: sig.clone(),
            });
            continue;
        };

        // Look up the input's conversion method, and reject if it doesn't exist or has no formula.
        let Some(formula) = own.and_then(|cm| cm.formula.clone()) else {
            rejected.push(RejectedVirtual {
                reason: RejectedVirtualReason::MissingFormula,
                signal: sig.clone(),
            });
            continue;
        };

        // Reduce the virtual's formula to a linear function of the input's conversion, and reject if it can't be done.
        let Some((_, f_form, o_form)) = reduced else {
            rejected.push(RejectedVirtual {
                reason: RejectedVirtualReason::NonLinearFormula { formula },
                signal: sig.clone(),
            });
            continue;
        };

        let input_cm = input
            .conversion
            .as_ref()
            .and_then(|c| project.compu_methods.get(&c.identifier));
        let (f_in, o_in) = input_cm
            .map(|cm| (cm.factor, cm.offset))
            .unwrap_or((1.0, 0.0));

        let unit = own
            .map(|cm| cm.unit.clone())
            .filter(|u| !u.is_empty())
            .or_else(|| {
                sig.virtual_conversion
                    .as_ref()
                    .and_then(|id| project.compu_methods.get(id))
                    .map(|cm| cm.unit.clone())
            })
            .unwrap_or_default();

        let identifier = format!("{}.VIRTUAL_RESOLVED", sig.name);
        synthetic.push((
            identifier.clone(),
            CompuMethod {
                factor: f_form * f_in,
                offset: f_form * o_in + o_form,
                unit,
                text_table: Vec::new(),
                default_text: None,
                formula: Some(formula),
                unsupported: input_cm.and_then(|cm| cm.unsupported.clone()),
            },
        ));

        sig.virtual_inputs = vec![input_name];
        sig.ecu_address = input.ecu_address;
        sig.size = input.size;
        sig.is_signed = input.is_signed;
        sig.is_float = input.is_float;
        sig.byte_order = input.byte_order.clone();
        sig.bit_mask = input.bit_mask;
        if sig.event.is_none() {
            sig.event = input.event.clone();
        }
        sig.conversion = Some(Conversion { identifier });
    }

    project.compu_methods.extend(synthetic);
    let dropped: HashSet<&str> = rejected.iter().map(|r| r.signal.name.as_str()).collect();
    project
        .signals
        .retain(|s| !dropped.contains(s.name.as_str()));
    rejected
}
