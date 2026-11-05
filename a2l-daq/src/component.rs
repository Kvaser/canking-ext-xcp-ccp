//! WASI Preview 2 component bindings.
//!
//! Build: `cargo build --lib --target wasm32-wasip2 --no-default-features --features component`

wit_bindgen::generate!({
    world: "a2l-daq",
    path: "wit",
});

use exports::kvaser::a2l_daq::generator as bindings;

struct Component;

impl bindings::Guest for Component {
    fn inspect(a2l: String) -> Result<bindings::A2lSummary, String> {
        let summary = crate::inspect(&a2l).map_err(|e| e.to_string())?;
        Ok(bindings::A2lSummary {
            signals: summary.signals.into_iter().map(signal_to_wit).collect(),
            events: summary.events.into_iter().map(event_to_wit).collect(),
            protocols: summary.protocols.into_iter().map(protocol_to_wit).collect(),
            transports: summary
                .transports
                .into_iter()
                .map(transport_to_wit)
                .collect(),
            groups: summary.groups.into_iter().map(group_to_wit).collect(),
            unread: summary
                .unread
                .into_iter()
                .map(|u| bindings::UnreadKeyword {
                    keyword: u.keyword,
                    count: u.count,
                    effect: u.effect,
                })
                .collect(),
        })
    }

    // Build the core output (DAQ lists, DBC, warnings) from the A2L and assignments.
    fn build(
        a2l: String,
        source_name: String,
        selected: Vec<String>,
        assignments: Vec<bindings::EventAssignment>,
        protocol: bindings::Protocol,
    ) -> Result<bindings::CoreOutput, String> {
        let (mut project, rejected) =
            crate::parse_a2l_with_rejects(&a2l, &selected).map_err(|e| e.to_string())?;
        let assignments: Vec<(String, Vec<String>)> = assignments
            .into_iter()
            .map(|a| (a.event, a.signals))
            .collect();
        let mut core = crate::build(
            &mut project,
            &assignments,
            protocol_from_wit(protocol),
            &source_name,
        )
        .map_err(|e| e.to_string())?;
        core.warnings
            .extend(crate::dropped_signals(&project, &selected, &rejected));
        Ok(core_to_wit(core))
    }

    // Build the JSON config for use in (e.g) CanKing service extension
    fn build_config(
        a2l: String,
        source_name: String,
        selected: Vec<String>,
        assignments: Vec<bindings::EventAssignment>,
        start_daq: bool,
        daq_can_id_overrides: Vec<(u16, u32)>,
        daq_prescalers: Vec<(u16, u16)>,
        protocol: bindings::Protocol,
    ) -> Result<bindings::ConfigOutput, String> {
        let proto = protocol_from_wit(protocol);
        let (mut project, rejected) =
            crate::parse_a2l_with_rejects(&a2l, &selected).map_err(|e| e.to_string())?;
        let assignments: Vec<(String, Vec<String>)> = assignments
            .into_iter()
            .map(|a| (a.event, a.signals))
            .collect();

        let mut core = crate::build(&mut project, &assignments, proto, &source_name)
            .map_err(|e| e.to_string())?;
        core.warnings
            .extend(crate::dropped_signals(&project, &selected, &rejected));
        crate::apply_can_id_overrides(&mut core.daq_lists, &daq_can_id_overrides);
        crate::apply_prescalers(&mut core.daq_lists, &daq_prescalers);

        let config = crate::Config {
            project,
            daq_lists: core.daq_lists,
            start_daq,
            protocol: proto,
        };
        let config_json = serde_json::to_string(&config).map_err(|e| e.to_string())?;

        Ok(bindings::ConfigOutput {
            config_json,
            dbc: core.dbc,
            warnings: core.warnings.into_iter().map(diag_to_wit).collect(),
        })
    }

    // Build the t-script with `tscript.rs`
    fn build_tscript(
        a2l: String,
        source_name: String,
        selected: Vec<String>,
        assignments: Vec<bindings::EventAssignment>,
        options: bindings::TscriptOptions,
        daq_can_id_overrides: Vec<(u16, u32)>,
        daq_prescalers: Vec<(u16, u16)>,
        protocol: bindings::Protocol,
    ) -> Result<bindings::TscriptOutput, String> {
        let proto = protocol_from_wit(protocol);
        let (mut project, rejected) =
            crate::parse_a2l_with_rejects(&a2l, &selected).map_err(|e| e.to_string())?;
        let assignments: Vec<(String, Vec<String>)> = assignments
            .into_iter()
            .map(|a| (a.event, a.signals))
            .collect();
        let mut core = crate::build(&mut project, &assignments, proto, &source_name)
            .map_err(|e| e.to_string())?;
        core.warnings
            .extend(crate::dropped_signals(&project, &selected, &rejected));
        crate::apply_can_id_overrides(&mut core.daq_lists, &daq_can_id_overrides);
        crate::apply_prescalers(&mut core.daq_lists, &daq_prescalers);

        // Manual override if chosen, else the A2L transport block's defaults.
        let (is_fd, bitrate, data_bitrate) = match &options.bus {
            Some(bus) => {
                let is_fd = matches!(bus.mode, bindings::CanMode::CanFd);
                let data =
                    is_fd.then(|| bus.data_bitrate.unwrap_or(crate::DEFAULT_FD_DATA_BITRATE));
                (is_fd, bus.bitrate, data)
            }
            None => {
                let defaults = crate::bus_defaults(&project, proto);
                (
                    defaults.can_fd_data_bitrate.is_some(),
                    defaults.bitrate,
                    defaults.can_fd_data_bitrate,
                )
            }
        };

        let inline = matches!(options.signal_access, bindings::SignalAccess::Inline);

        // Only used inline; a project with no default CAN id still resolves fine in DBC mode.
        let default_can_id = if inline {
            crate::layout::daq_can_id(&project, proto)
        } else {
            None
        };

        let terms = trigger_terms(&options.conditions)?;
        let resolved_trigger: Option<(crate::ResolvedTrigger, crate::TriggerAction)> =
            if terms.is_empty() {
                None
            } else {
                let resolved = crate::resolve_terms(
                    &terms,
                    inline,
                    &project,
                    &core.daq_lists,
                    default_can_id.unwrap_or(0),
                )
                .map_err(|e| e.to_string())?;
                Some((resolved, crate::TriggerAction::StartLogger))
            };

        let combo = resolved_trigger.as_ref().map(|(r, a)| (r, *a));
        let signals = crate::resolve_signal_hooks(inline, &project, &core.daq_lists, proto, combo)
            .map_err(|e| format!("cannot decode signals in the script: {e}"))?;
        let decoders = resolved_trigger
            .map(|(r, _)| r.decoders)
            .unwrap_or_default();

        let config = crate::TScriptConfig {
            channels: vec![crate::CanChannel {
                index: options.channel_index,
                bitrate,
                data_bitrate,
            }],
            pretrigger_ms: options.pretrigger_ms,
            signals,
            timer_ms: options.timer_ms,
            max_captures: options.max_captures,
            daq_watchdog_ms: options.daq_watchdog_ms,
            daq_init: crate::build_daq_init(
                proto,
                &project,
                &core.daq_lists,
                crate::DEFAULT_INIT_TIMEOUT_MS,
            ),
            can_fd: is_fd,
            traceability: crate::traceability::traceability_lines(
                &source_name,
                &project,
                &core.daq_lists,
                proto,
            ),
            decoders,
        };
        Ok(bindings::TscriptOutput {
            tscript: crate::generate_tscript(&config),
            warnings: core.warnings.into_iter().map(diag_to_wit).collect(),
        })
    }
}

/// Validate the GUI's structured conditions.
fn trigger_terms(
    conditions: &[bindings::TriggerCondition],
) -> Result<Vec<crate::TriggerTerm>, String> {
    let mut terms = Vec::new();
    for c in conditions {
        let signal = c.signal.trim();
        if signal.is_empty() {
            continue;
        }
        let op_str = c.op.trim();
        let op = crate::CompareOp::from_symbol(op_str)
            .ok_or_else(|| format!("invalid trigger operator '{op_str}'"))?;
        let value = c.value.trim();
        if value.parse::<f64>().is_err() {
            return Err(format!(
                "invalid trigger value '{}' for signal '{}'",
                c.value, signal
            ));
        }
        terms.push(crate::TriggerTerm {
            signal: signal.to_string(),
            accessor: crate::Accessor::Phys,
            op,
            value: value.to_string(),
        });
    }
    Ok(terms)
}

export!(Component);

// ---- WIT conversions ----

fn core_to_wit(core: crate::CoreOutput) -> bindings::CoreOutput {
    bindings::CoreOutput {
        daq_lists: core.daq_lists.into_iter().map(daq_to_wit).collect(),
        dbc: core.dbc,
        warnings: core.warnings.into_iter().map(diag_to_wit).collect(),
    }
}

fn transport_to_wit(t: crate::TransportInfo) -> bindings::TransportInfo {
    bindings::TransportInfo {
        protocol: protocol_to_wit(t.protocol),
        can_fd: t.can_fd,
        bitrate: t.bitrate,
        data_bitrate: t.data_bitrate,
        prescaler: t.prescaler,
    }
}

fn protocol_to_wit(p: crate::Protocol) -> bindings::Protocol {
    match p {
        crate::Protocol::Xcp => bindings::Protocol::Xcp,
        crate::Protocol::Ccp => bindings::Protocol::Ccp,
    }
}

fn protocol_from_wit(p: bindings::Protocol) -> crate::Protocol {
    match p {
        bindings::Protocol::Xcp => crate::Protocol::Xcp,
        bindings::Protocol::Ccp => crate::Protocol::Ccp,
    }
}

fn signal_to_wit(s: crate::SignalSummary) -> bindings::SignalSummary {
    bindings::SignalSummary {
        name: s.name,
        description: s.description,
        unit: s.unit,
        ecu_address: s.ecu_address,
        size: s.size,
        virtual_input: s.virtual_input,
        virtual_issue: s.virtual_reject.as_ref().map(issue_to_wit),
        groups: s.groups,
    }
}

fn group_to_wit(g: crate::GroupInfo) -> bindings::GroupInfo {
    bindings::GroupInfo {
        name: g.name,
        description: g.description,
        root: g.root,
        sub_groups: g.sub_groups,
    }
}

fn issue_to_wit(reason: &crate::RejectedVirtualReason) -> bindings::VirtualIssue {
    bindings::VirtualIssue {
        label: reason.label().to_string(),
        detail: reason.to_string(),
    }
}

fn event_to_wit(e: crate::EventEntry) -> bindings::EventInfo {
    bindings::EventInfo {
        name: e.name,
        channel: e.channel,
        time_cycle: e.time_cycle,
        time_unit: e.time_unit,
    }
}

fn daq_to_wit(d: crate::DaqList) -> bindings::DaqList {
    bindings::DaqList {
        id: d.id,
        first_pid: d.first_pid,
        odts: d.odts.into_iter().map(odt_to_wit).collect(),
        event: d.event,
        can_id: can_id_to_wit(d.can_id),
    }
}

fn can_id_to_wit(c: crate::CanIdSource) -> bindings::CanIdSource {
    match c {
        crate::CanIdSource::Default => bindings::CanIdSource::Shared,
        crate::CanIdSource::Fixed { id } => bindings::CanIdSource::Fixed(id),
        crate::CanIdSource::Dynamic { assigned } => bindings::CanIdSource::Dynamic(assigned),
    }
}

fn odt_to_wit(o: crate::Odt) -> bindings::Odt {
    bindings::Odt {
        entries: o.entries.into_iter().map(entry_to_wit).collect(),
    }
}

fn entry_to_wit(e: crate::OdtEntry) -> bindings::OdtEntry {
    bindings::OdtEntry {
        names: e.names,
        ecu_address: e.ecu_address,
        size: e.size,
    }
}

fn diag_to_wit(d: crate::Diagnostic) -> bindings::Diagnostic {
    match d {
        crate::Diagnostic::UnplacedSignal {
            name,
            event,
            size,
            ecu_address,
        } => bindings::Diagnostic::UnplacedSignal(bindings::UnplacedSignal {
            name,
            event,
            size,
            ecu_address,
        }),
        crate::Diagnostic::UnknownSignal { name } => bindings::Diagnostic::UnknownSignal(name),
        crate::Diagnostic::DroppedSignal { name } => bindings::Diagnostic::DroppedSignal(name),
        crate::Diagnostic::UnmeasurableSignal { name, reason } => {
            bindings::Diagnostic::UnmeasurableSignal(bindings::UnmeasurableSignal {
                name,
                issue: issue_to_wit(&reason),
            })
        }
        crate::Diagnostic::CollidingNames { sanitized, names } => {
            bindings::Diagnostic::CollidingNames(bindings::CollidingNames { sanitized, names })
        }
        crate::Diagnostic::UnsupportedConversion {
            name,
            conversion,
            reason,
        } => bindings::Diagnostic::UnsupportedConversion(bindings::UnsupportedConversion {
            name,
            conversion,
            reason,
        }),
    }
}
