//! Core pipeline: parse output -> packed DAQ lists + DBC.
//!
//! [`build`] takes an already-parsed [`A2lProject`] plus the user's event
//! assignments and returns the packed DAQ lists, the DBC text, and a list of
//! non-fatal [`Diagnostic`]s. It performs no I/O.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Display, Formatter};

use crate::Protocol;
use crate::a2l::{A2lProject, CanIdSource, RejectedVirtual, RejectedVirtualReason};
use crate::daq::{DaqList, build_daq_list};
use crate::dbc::{DbcError, create_dbc_file, sanitize_name};

/// A non-fatal issue surfaced to the host so it can be shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// A signal was assigned to an event but no DAQ list slot remained for it, so it is absent from both the DBC and any downstream output.
    UnplacedSignal {
        name: String,
        event: Option<String>,
        size: u16,
        ecu_address: u64,
    },
    /// An event assignment referenced a signal name not present in the parsed project.
    UnknownSignal { name: String },
    /// A selected signal is absent from the parsed project: the name does not exist in the A2L, or its MEASUREMENT block could not be parsed
    DroppedSignal { name: String },
    /// A selected signal parsed fine but is a virtual measurement that cannot be measured,
    UnmeasurableSignal {
        name: String,
        reason: RejectedVirtualReason,
    },
    /// Two or more signals in the DAQ lists sanitise to one DBC identifier, so the generated artefacts cannot tell them apart.
    CollidingNames {
        sanitized: String,
        names: Vec<String>,
    },
    /// A placed signal's COMPU_METHOD could not be reduced to factor/offset, so every output carries its raw value.
    UnsupportedConversion {
        name: String,
        conversion: String,
        reason: String,
    },
}

impl Display for Diagnostic {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnplacedSignal {
                name,
                event,
                size,
                ecu_address,
            } => write!(
                f,
                "signal '{name}' could not be placed in any DAQ list \
                 (event: {event:?}, size: {size} bytes, address: 0x{ecu_address:X})"
            ),
            Self::UnknownSignal { name } => {
                write!(f, "event assignment references unknown signal '{name}'")
            }
            Self::DroppedSignal { name } => write!(
                f,
                "selected signal '{name}' is not in the parsed A2L \
                 (unknown name or unparsable MEASUREMENT block) and was dropped"
            ),
            Self::UnmeasurableSignal { name, reason } => {
                write!(f, "selected signal '{name}' cannot be measured: {reason}")
            }
            Self::CollidingNames { sanitized, names } => write!(
                f,
                "signals {} all become '{sanitized}' in the DBC and t-script, \
                 so they cannot be told apart downstream",
                names
                    .iter()
                    .map(|n| format!("'{n}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::UnsupportedConversion {
                name,
                conversion,
                reason,
            } => write!(
                f,
                "signal '{name}' uses conversion '{conversion}' ({reason}), which is not supported:                  its values are raw, not physical"
            ),
        }
    }
}

/// The core artefacts: the packed DAQ lists and the DBC text, plus any diagnostics collected along the way.
#[derive(Debug, Clone)]
pub struct CoreOutput {
    pub daq_lists: Vec<DaqList>,
    pub dbc: String,
    pub warnings: Vec<Diagnostic>,
}

/// Flag explicitly selected signal names that did not survive parsing.
pub fn dropped_signals(
    project: &A2lProject,
    selected: &[String],
    rejected: &[RejectedVirtual],
) -> Vec<Diagnostic> {
    let known: HashSet<&str> = project.signals.iter().map(|s| s.name.as_str()).collect();
    let unmeasurable: HashMap<&str, &RejectedVirtualReason> = rejected
        .iter()
        .map(|r| (r.signal.name.as_str(), &r.reason))
        .collect();

    selected
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !known.contains(s))
        .map(|name| match unmeasurable.get(name) {
            Some(reason) => Diagnostic::UnmeasurableSignal {
                name: name.to_string(),
                reason: (*reason).clone(),
            },
            None => Diagnostic::DroppedSignal {
                name: name.to_string(),
            },
        })
        .collect()
}

/// Stamp each signal with its assigned event (first match wins) and flag
/// assignments that name a signal the project doesn't know about.
///
/// `assignments` maps an event name to the signal names assigned to it; signals
/// not covered keep whatever event they were parsed with.
pub fn apply_assignments(
    project: &mut A2lProject,
    assignments: &[(String, Vec<String>)],
) -> Vec<Diagnostic> {
    let mut warnings = Vec::new();

    let known: HashSet<&str> = project.signals.iter().map(|s| s.name.as_str()).collect();
    for (_event, names) in assignments {
        for name in names {
            if !known.contains(name.as_str()) {
                warnings.push(Diagnostic::UnknownSignal { name: name.clone() });
            }
        }
    }

    for signal in &mut project.signals {
        for (event, assigned) in assignments {
            if assigned.iter().any(|n| n == &signal.name) {
                signal.event = Some(event.clone());
                break;
            }
        }
    }

    warnings
}

/// Apply event assignments, pack DAQ lists, and generate the DBC for `protocol`.
pub fn build(
    project: &mut A2lProject,
    assignments: &[(String, Vec<String>)],
    protocol: Protocol,
    source_name: &str,
) -> Result<CoreOutput, DbcError> {
    let mut warnings = apply_assignments(project, assignments);

    let (daq_lists, unplaced) = build_daq_list(project, protocol);
    for s in unplaced {
        warnings.push(Diagnostic::UnplacedSignal {
            name: s.name,
            event: s.event,
            size: s.size,
            ecu_address: s.ecu_address,
        });
    }

    warnings.extend(colliding_names(&daq_lists));
    warnings.extend(unsupported_conversions(project, &daq_lists));

    let dbc = create_dbc_file(project, &daq_lists, protocol, source_name)?;

    Ok(CoreOutput {
        daq_lists,
        dbc,
        warnings,
    })
}

/// Apply user-chosen CAN ids onto `Dynamic` DAQ lists, by list identifier.
///
/// Only a list whose `CanIdSource` is `Dynamic` is affected
pub fn apply_can_id_overrides(daq_lists: &mut [DaqList], overrides: &[(u16, u32)]) {
    for &(list_id, can_id) in overrides {
        let Some(list) = daq_lists.iter_mut().find(|d| d.id == list_id) else {
            continue;
        };
        if let CanIdSource::Dynamic { assigned } = &mut list.can_id {
            *assigned = Some(can_id);
        }
    }
}

/// Flag placed signals whose conversion could not be applied, once per signal.
fn unsupported_conversions(project: &A2lProject, daq_lists: &[DaqList]) -> Vec<Diagnostic> {
    let signals = crate::layout::signal_lookup(project);
    let mut seen = HashSet::new();
    daq_lists
        .iter()
        .flat_map(|d| &d.odts)
        .flat_map(|o| &o.entries)
        .flat_map(|e| &e.names)
        .filter_map(|n| crate::layout::resolve_signal(&signals, n))
        .filter(|s| seen.insert(s.name.as_str()))
        .filter_map(|s| {
            let conversion = s.conversion.as_ref()?;
            let reason = project
                .compu_methods
                .get(&conversion.identifier)?
                .unsupported
                .clone()?;
            Some(Diagnostic::UnsupportedConversion {
                name: s.name.clone(),
                conversion: conversion.identifier.clone(),
                reason,
            })
        })
        .collect()
}

/// Flag signal names that collide once sanitised into DBC identifiers.
fn colliding_names(daq_lists: &[DaqList]) -> Vec<Diagnostic> {
    let mut by_ident: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in daq_lists
        .iter()
        .flat_map(|d| &d.odts)
        .flat_map(|o| &o.entries)
        .flat_map(|e| &e.names)
    {
        let group = by_ident.entry(sanitize_name(name)).or_default();
        if !group.contains(name) {
            group.push(name.clone());
        }
    }

    by_ident
        .into_iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(sanitized, names)| Diagnostic::CollidingNames { sanitized, names })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Protocol, parse_a2l, parse_a2l_with_rejects};

    const A2L: &str = r#"
    /begin MEASUREMENT Rpm "Engine speed" UWORD Rpm_CM 0 0 0 8000 ECU_ADDRESS 0x1000 /end MEASUREMENT
    /begin IF_DATA XCP
    /begin XCP_ON_CAN
        0x0105 CAN_ID_MASTER 0x0455 CAN_ID_SLAVE 0x0456 BAUDRATE 500000
    /end XCP_ON_CAN
    /begin DAQ
        DYNAMIC
        /begin DAQ_LIST 0x0000 MAX_ODT 0x05 MAX_ODT_ENTRIES 0x07 /end DAQ_LIST
        /begin EVENT "Task10ms" "10ms" 0x0000 DAQ 0x01 0x0A 0x06 0xFF /end EVENT
    /end DAQ
    /end IF_DATA
    "#;

    #[test]
    fn build_produces_daq_lists_and_dbc() {
        let mut project = parse_a2l(A2L, &[]).unwrap();
        let assignments = vec![("Task10ms".into(), vec!["Rpm".into()])];

        let out = build(&mut project, &assignments, Protocol::Xcp, "test.a2l").unwrap();
        assert!(out.warnings.is_empty(), "warnings: {:?}", out.warnings);
        assert_eq!(out.daq_lists.len(), 1);
        assert!(out.dbc.contains("BO_ 1110 DAQ:"));
    }

    #[test]
    fn placed_signal_with_unsupported_conversion_is_flagged_once() {
        let content = r#"
        /begin MEASUREMENT Power "" UWORD Square 0 0 0 100 ECU_ADDRESS 0x1000 /end MEASUREMENT
        /begin MEASUREMENT Temp "" UWORD Lin 0 0 0 100 ECU_ADDRESS 0x2000 /end MEASUREMENT
        /begin COMPU_METHOD Square "" FORM "%5.1" "W" /begin FORMULA "X1*X1" /end FORMULA /end COMPU_METHOD
        /begin COMPU_METHOD Lin "" FORM "%5.1" "degC" /begin FORMULA "X1-40" /end FORMULA /end COMPU_METHOD
        "#;
        let a2l = format!("{content}{}", &A2L[A2L.find("/begin IF_DATA").unwrap()..]);
        let mut project = parse_a2l(&a2l, &[]).unwrap();
        let assignments = vec![("Task10ms".into(), vec!["Power".into(), "Temp".into()])];
        let out = build(&mut project, &assignments, Protocol::Xcp, "test.a2l").unwrap();
        let flagged: Vec<&str> = out
            .warnings
            .iter()
            .filter_map(|w| match w {
                Diagnostic::UnsupportedConversion { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(flagged, vec!["Power"]);
    }

    #[test]
    fn unknown_assigned_signal_becomes_warning() {
        let mut project = parse_a2l(A2L, &[]).unwrap();
        let assignments = vec![("Task10ms".into(), vec!["Ghost".into()])];
        let out = build(&mut project, &assignments, Protocol::Xcp, "test.a2l").unwrap();
        assert!(
            out.warnings
                .iter()
                .any(|w| matches!(w, Diagnostic::UnknownSignal { name } if name == "Ghost"))
        );
    }

    #[test]
    fn selected_but_unparsable_signal_is_flagged_as_dropped() {
        let content = r#"
        /begin MEASUREMENT Rpm "Engine speed" UWORD Rpm_CM 0 0 0 8000 ECU_ADDRESS 0x1000 /end MEASUREMENT
        /begin MEASUREMENT Broken "no address" UWORD B_CM 0 0 0 100 /end MEASUREMENT
        "#;
        let selected = vec!["Rpm".to_string(), "Broken".to_string(), "Ghost".to_string()];
        let (project, rejected) = parse_a2l_with_rejects(content, &selected).unwrap();
        assert!(rejected.is_empty(), "no virtuals in this A2L");

        let dropped = dropped_signals(&project, &selected, &rejected);
        let names: Vec<&str> = dropped
            .iter()
            .map(|d| match d {
                Diagnostic::DroppedSignal { name } => name.as_str(),
                other => panic!("unexpected diagnostic {other:?}"),
            })
            .collect();
        assert_eq!(names, ["Broken", "Ghost"]);
        assert!(dropped_signals(&project, &[], &rejected).is_empty());
    }

    #[test]
    fn selected_but_unmeasurable_virtual_reports_its_reason() {
        let content = r#"
        /begin MEASUREMENT Raw "raw input" UWORD Raw_CM 0 0 0 65535 ECU_ADDRESS 0x1000 /end MEASUREMENT
        /begin MEASUREMENT Sum "two inputs" FLOAT32_IEEE Sum_CM 0 0 0 100
          /begin VIRTUAL Raw Other /end VIRTUAL
        /end MEASUREMENT
        "#;
        let selected = vec!["Sum".to_string(), "Ghost".to_string()];
        let (project, rejected) = parse_a2l_with_rejects(content, &selected).unwrap();

        let warnings = dropped_signals(&project, &selected, &rejected);
        assert_eq!(
            warnings,
            vec![
                Diagnostic::UnmeasurableSignal {
                    name: "Sum".to_string(),
                    reason: RejectedVirtualReason::MultipleInputs {
                        inputs: vec!["Raw".to_string(), "Other".to_string()],
                    },
                },
                Diagnostic::DroppedSignal {
                    name: "Ghost".to_string()
                },
            ]
        );
        assert!(warnings[0].to_string().contains("cannot be measured"));
    }

    #[test]
    fn names_that_sanitise_alike_are_flagged() {
        let content = r#"
        /begin MEASUREMENT Motor.Rpm "dotted" UWORD CM 0 0 0 8000 ECU_ADDRESS 0x1000 /end MEASUREMENT
        /begin MEASUREMENT Motor_Rpm "underscored" UWORD CM 0 0 0 8000 ECU_ADDRESS 0x2000 /end MEASUREMENT
        /begin IF_DATA XCP
        /begin XCP_ON_CAN
            0x0105 CAN_ID_MASTER 0x0455 CAN_ID_SLAVE 0x0456 BAUDRATE 500000
        /end XCP_ON_CAN
        /begin DAQ
            DYNAMIC
            /begin DAQ_LIST 0x0000 MAX_ODT 0x05 MAX_ODT_ENTRIES 0x07 /end DAQ_LIST
            /begin EVENT "Task10ms" "10ms" 0x0000 DAQ 0x01 0x0A 0x06 0xFF /end EVENT
        /end DAQ
        /end IF_DATA
        "#;
        let mut project = parse_a2l(content, &[]).unwrap();
        let assignments = vec![(
            "Task10ms".to_string(),
            vec!["Motor.Rpm".to_string(), "Motor_Rpm".to_string()],
        )];
        let out = build(&mut project, &assignments, Protocol::Xcp, "test.a2l").unwrap();

        let collision = out
            .warnings
            .iter()
            .find_map(|w| match w {
                Diagnostic::CollidingNames { sanitized, names } => Some((sanitized, names)),
                _ => None,
            })
            .expect("collision reported");
        assert_eq!(collision.0, "Motor_Rpm");
        assert_eq!(collision.1.len(), 2);
        assert!(collision.1.contains(&"Motor.Rpm".to_string()));
        assert!(collision.1.contains(&"Motor_Rpm".to_string()));

        assert_eq!(out.dbc.matches(" SG_ Motor_Rpm ").count(), 2, "{}", out.dbc);
    }

    #[test]
    fn distinct_names_produce_no_collision_warning() {
        let mut project = parse_a2l(A2L, &[]).unwrap();
        let assignments = vec![("Task10ms".into(), vec!["Rpm".into()])];
        let out = build(&mut project, &assignments, Protocol::Xcp, "test.a2l").unwrap();
        assert!(
            !out.warnings
                .iter()
                .any(|w| matches!(w, Diagnostic::CollidingNames { .. }))
        );
    }

    #[test]
    fn build_stamps_the_assignments_onto_the_project() {
        let mut project = parse_a2l(A2L, &[]).unwrap();
        assert_eq!(project.signals[0].event, None);
        let assignments = vec![("Task10ms".into(), vec!["Rpm".into()])];
        build(&mut project, &assignments, Protocol::Xcp, "test.a2l").unwrap();
        assert_eq!(project.signals[0].event.as_deref(), Some("Task10ms"));
    }

    #[test]
    fn can_id_override_only_lands_on_a_dynamic_list() {
        use crate::daq::{Odt, OdtEntry};

        let entry = |name: &str| Odt {
            entries: vec![OdtEntry {
                names: vec![name.to_string()],
                ecu_address: 0x1000,
                size: 2,
            }],
        };
        let mut lists = vec![
            DaqList {
                id: 0,
                first_pid: 0,
                odts: vec![entry("A")],
                event: None,
                can_id: CanIdSource::Default,
            },
            DaqList {
                id: 1,
                first_pid: 1,
                odts: vec![entry("B")],
                event: None,
                can_id: CanIdSource::Fixed { id: 0x789 },
            },
            DaqList {
                id: 2,
                first_pid: 2,
                odts: vec![entry("C")],
                event: None,
                can_id: CanIdSource::Dynamic { assigned: None },
            },
        ];

        apply_can_id_overrides(
            &mut lists,
            &[(0, 0x111), (1, 0x222), (2, 0x333), (9, 0x444)],
        );

        assert_eq!(
            lists[0].can_id,
            CanIdSource::Default,
            "override on a Default list is ignored"
        );
        assert_eq!(
            lists[1].can_id,
            CanIdSource::Fixed { id: 0x789 },
            "override on a Fixed list is ignored"
        );
        assert_eq!(
            lists[2].can_id,
            CanIdSource::Dynamic {
                assigned: Some(0x333)
            },
            "override lands on the matching Dynamic list"
        );
    }

    #[test]
    fn missing_can_id_slave_is_fatal() {
        let mut project = parse_a2l(A2L, &[]).unwrap();
        if let Some(x) = project.xcp_data.as_mut() {
            if let Some(c) = x.xcp_info.as_mut() {
                c.can_id_slave = None;
            }
        }
        let err = build(&mut project, &[], Protocol::Xcp, "test.a2l").unwrap_err();
        assert_eq!(err, DbcError::MissingCanIdSlave);
    }
}
