//! The trigger a generated script arms: what it reads, and how it compares.
//!
//! DAQ data is multiplexed, so a signal's bytes are only meaningful in frames carrying its
//! ODT. The script therefore latches each read behind that ODT's PID gate and compares the
//! latched values rather than reading in place. This module resolves the reads against the
//! DAQ layout and renders the latch/condition fragments `super::emit` drops into the hook.

use super::inline_signals::{self, SignalDecoder, TriggerError};
use crate::a2l::A2lProject;
use crate::daq::DaqList;
use std::fmt::Write as FmtWrite;

/// Struct representing a trigger condition for starting or stopping the logger
///
/// Each signal the trigger reads is latched into a global when the ODT carrying it arrives, and the comparisons then run over the latched values.
/// This is to make multiplexed signals work correctly.
pub struct TriggerCondition {
    pub signals: Vec<TriggerSignal>,
    pub comparisons: Vec<Comparison>,
    pub action: TriggerAction,
}

/// One value the trigger reads, and where on the bus it arrives.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerSignal {
    pub ident: String,
    pub pid: u16,
    pub read: String,
    pub source_name: String,
}

/// One comparison against a latched value.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub ident: String,
    pub op: CompareOp,
    pub value: String,
}

/// A trigger comparison operator, restricted to what the t language's `if` expressions support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

/// Comparison operator util for tscript.
impl CompareOp {
    /// Every operator, ordered longest symbol first.
    ///
    /// Expression parsers split a term on the first symbol in this list that matches, so
    /// the two-character operators must come before the one-character ones
    pub const ALL: [Self; 6] = [Self::Ge, Self::Le, Self::Eq, Self::Ne, Self::Gt, Self::Lt];

    pub fn from_symbol(s: &str) -> Option<Self> {
        Some(match s {
            ">" => Self::Gt,
            ">=" => Self::Ge,
            "<" => Self::Lt,
            "<=" => Self::Le,
            "==" => Self::Eq,
            "!=" => Self::Ne,
            _ => return None,
        })
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Eq => "==",
            Self::Ne => "!=",
        }
    }
}

/// Which member of a DBC signal a trigger reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accessor {
    Phys,
    Raw,
}

impl Accessor {
    fn member(self) -> &'static str {
        match self {
            Self::Phys => "Phys",
            Self::Raw => "Raw",
        }
    }

    fn ident_suffix(self) -> &'static str {
        match self {
            Self::Phys => "",
            Self::Raw => "_raw",
        }
    }

    /// The latch identifier for reading `name` through this accessor.
    ///
    /// A [`Comparison`] must name the same identifier as the [`TriggerSignal`] it
    /// compares, and `.Phys` and `.Raw` reads of one signal latch separately.
    pub fn latch_ident(self, name: &str) -> String {
        format!("{}{}", crate::sanitize_name(name), self.ident_suffix())
    }
}

/// Resolve DBC-backed trigger reads against the DAQ layout.
pub fn dbc_trigger_signals(
    project: &A2lProject,
    daq_lists: &[DaqList],
    reads: &[(String, Accessor)],
) -> Result<Vec<TriggerSignal>, TriggerError> {
    let placements = crate::layout::signal_placements(project, daq_lists);
    let mut signals: Vec<TriggerSignal> = Vec::new();

    for (name, accessor) in reads {
        let ident = accessor.latch_ident(name);
        if signals.iter().any(|s| s.ident == ident) {
            continue;
        }
        let placement = crate::layout::find_placement(&placements, name, crate::sanitize_name)
            .ok_or_else(|| TriggerError::NotInDaqList { name: name.clone() })?;
        if placement.chunk_size.is_some() {
            return Err(TriggerError::SplitAcrossOdts {
                name: placement.name.clone(),
            });
        }
        signals.push(TriggerSignal {
            ident,
            pid: placement.pid,
            read: format!(
                "this.{}.{}",
                crate::sanitize_name(&placement.name),
                accessor.member()
            ),
            source_name: placement.name.clone(),
        });
    }
    Ok(signals)
}

/// One [`TriggerSignal`] per decoder, for a standalone script. Decoders always
/// yield physical values.
pub fn inline_trigger_signals(decoders: &[SignalDecoder]) -> Vec<TriggerSignal> {
    decoders
        .iter()
        .map(|d| TriggerSignal {
            ident: d.ident.clone(),
            pid: d.pid,
            read: format!("sig_{}(this.data)", d.ident),
            source_name: d.source_name.clone(),
        })
        .collect()
}

/// The signals a trigger reads, plus the decoders a standalone script needs to read them.
/// Empty decoders mean the script reads through the DBC instead.
pub type TriggerParts = (Vec<TriggerSignal>, Vec<SignalDecoder>);

/// Resolve a trigger's reads against the DAQ layout. `default_can_id` is only used inline,
/// to resolve a list's CAN id when it declares no override of its own.
pub fn resolve_trigger(
    project: &A2lProject,
    daq_lists: &[DaqList],
    reads: &[(String, Accessor)],
    inline: bool,
    default_can_id: u32,
) -> Result<TriggerParts, TriggerError> {
    if !inline {
        return Ok((dbc_trigger_signals(project, daq_lists, reads)?, Vec::new()));
    }
    debug_assert!(
        reads.iter().all(|(_, a)| *a == Accessor::Phys),
        "inline decoders yield physical values; callers must normalise reads first"
    );
    let names: Vec<String> = reads.iter().map(|(name, _)| name.clone()).collect();
    let decoders = inline_signals::decoders_for(project, daq_lists, &names, default_can_id)?;
    let signals = inline_trigger_signals(&decoders);
    Ok((signals, decoders))
}

/// The latch and seen-flag globals, for the `variables` block.
///
/// Latches are `float` because the t language has no `double` and a physical value is
/// generally fractional. That costs precision above 2^24 on wide integers, so a `.Raw`
/// comparison against a 32-bit counter is approximate in its top bits. Physical values are
/// unaffected in practice, and the inline decoders already widen unsigned 32-bit fields
/// through a float for the same reason.
pub(super) fn latch_variables(signals: &[TriggerSignal]) -> String {
    let mut out = String::new();
    for s in signals {
        let _ = writeln!(
            out,
            "    float val_{} = 0;  /* last {} */",
            s.ident, s.source_name
        );
        let _ = writeln!(out, "    int got_{} = 0;", s.ident);
    }
    out
}

/// The PID-gated latch updates, one block per PID.
///
/// Emitted before any early return so the latched values keep tracking the bus even once logging has started.
pub(super) fn latch_block(signals: &[TriggerSignal], indent: &str) -> String {
    let mut out = String::new();
    let mut pids: Vec<u16> = signals.iter().map(|s| s.pid).collect();
    pids.sort_unstable();
    pids.dedup();

    for pid in pids {
        let _ = writeln!(out, "{indent}if (this.data[0] == 0x{pid:02X}) {{");
        for s in signals.iter().filter(|s| s.pid == pid) {
            let _ = writeln!(out, "{indent}    val_{} = {};", s.ident, s.read);
            let _ = writeln!(out, "{indent}    got_{} = 1;", s.ident);
        }
        let _ = writeln!(out, "{indent}}}");
    }
    out
}

pub(super) fn condition_expression(trigger: &TriggerCondition) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for ident in trigger.comparisons.iter().map(|c| c.ident.as_str()) {
        if !seen.contains(&ident) {
            seen.push(ident);
            parts.push(format!("got_{ident} == 1"));
        }
    }
    for c in &trigger.comparisons {
        parts.push(format!("val_{} {} {}", c.ident, c.op.symbol(), c.value));
    }
    parts.join(" && ")
}

#[derive(Clone, Copy)]
pub enum TriggerAction {
    StartLogger,
    StopLogger,
}

impl TriggerAction {
    pub(super) fn statements(&self) -> [&'static str; 2] {
        match self {
            TriggerAction::StartLogger => ["printf(\"Starting logger...\");", "loggerStart();"],
            TriggerAction::StopLogger => ["printf(\"Stopping logger...\");", "loggerStop();"],
        }
    }
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tscript::test_util::*;

    #[test]
    fn latch_block_gates_each_odt_separately() {
        let signals = vec![signal("A", 1, "this.A.Phys"), signal("B", 2, "this.B.Phys")];
        let block = latch_block(&signals, "    ");
        assert!(
            block.contains("    if (this.data[0] == 0x01) {\n        val_A = this.A.Phys;"),
            "got:\n{block}"
        );
        assert!(
            block.contains("    if (this.data[0] == 0x02) {\n        val_B = this.B.Phys;"),
            "got:\n{block}"
        );
        assert_eq!(block.matches("this.data[0] ==").count(), 2);
    }

    #[test]
    fn signals_in_one_odt_share_a_single_gate() {
        let signals = vec![signal("A", 3, "this.A.Phys"), signal("B", 3, "this.B.Phys")];
        let block = latch_block(&signals, "    ");
        assert_eq!(
            block.matches("this.data[0] ==").count(),
            1,
            "one ODT, one gate:\n{block}"
        );
        assert!(block.contains("val_A = this.A.Phys;"), "got:\n{block}");
        assert!(block.contains("val_B = this.B.Phys;"), "got:\n{block}");
    }

    #[test]
    fn condition_expression_gates_on_seen_then_compares() {
        let trigger = TriggerCondition {
            signals: vec![
                signal("Rpm", 1, "this.Rpm.Phys"),
                signal("Temp", 2, "this.Temp.Phys"),
            ],
            comparisons: vec![
                comparison("Rpm", ">", "3000"),
                comparison("Temp", "<", "90"),
            ],
            action: TriggerAction::StartLogger,
        };
        assert_eq!(
            condition_expression(&trigger),
            "got_Rpm == 1 && got_Temp == 1 && val_Rpm > 3000 && val_Temp < 90"
        );
    }

    #[test]
    fn repeated_signal_is_gated_once_but_compared_twice() {
        let trigger = TriggerCondition {
            signals: vec![signal("Rpm", 1, "this.Rpm.Phys")],
            comparisons: vec![
                comparison("Rpm", ">", "1000"),
                comparison("Rpm", "<", "5000"),
            ],
            action: TriggerAction::StartLogger,
        };
        assert_eq!(
            condition_expression(&trigger),
            "got_Rpm == 1 && val_Rpm > 1000 && val_Rpm < 5000"
        );
    }

    #[test]
    fn dbc_trigger_signals_take_the_pid_from_the_layout() {
        let (project, daq_lists) = two_odt_project();
        let reads = vec![
            ("Rpm".to_string(), Accessor::Phys),
            ("Temp".to_string(), Accessor::Phys),
        ];
        let signals = dbc_trigger_signals(&project, &daq_lists, &reads).unwrap();
        assert_eq!(signals.len(), 2);
        assert_eq!(
            (signals[0].pid, signals[0].read.as_str()),
            (1, "this.Rpm.Phys")
        );
        assert_eq!(
            (signals[1].pid, signals[1].read.as_str()),
            (2, "this.Temp.Phys")
        );
    }

    #[test]
    fn dbc_trigger_signals_keep_raw_and_phys_reads_apart() {
        let (project, daq_lists) = two_odt_project();
        let reads = vec![
            ("Rpm".to_string(), Accessor::Phys),
            ("Rpm".to_string(), Accessor::Raw),
            ("Rpm".to_string(), Accessor::Phys),
        ];
        let signals = dbc_trigger_signals(&project, &daq_lists, &reads).unwrap();
        assert_eq!(signals.len(), 2, "one latch per distinct read");
        assert_eq!(signals[0].ident, "Rpm");
        assert_eq!(signals[0].read, "this.Rpm.Phys");
        assert_eq!(signals[1].ident, "Rpm_raw");
        assert_eq!(signals[1].read, "this.Rpm.Raw");
    }

    #[test]
    fn dbc_trigger_signals_reject_what_no_frame_carries() {
        let (project, daq_lists) = two_odt_project();
        let err = dbc_trigger_signals(
            &project,
            &daq_lists,
            &[("Ghost".to_string(), Accessor::Phys)],
        )
        .unwrap_err();
        assert!(
            matches!(err, TriggerError::NotInDaqList { .. }),
            "got: {err:?}"
        );
    }
}
