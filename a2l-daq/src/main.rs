//! `a2l-daq-cli`: interactive A2L to DBC and t-script. Takes the A2L path as its only argument and asks for the rest.

use a2l_daq::{
    A2lProject, CanChannel, DaqList, MessageRef, Protocol, ResolvedTrigger, TScriptConfig,
    TriggerAction, build_daq_init, build_trigger, bus_defaults, dbc::sanitize_name,
    dbc_signal_hooks, event_channels, expand_signal_indices, generate_tscript, list_protocols,
    list_signal_names, parse_a2l_with_rejects, pipeline, resolve_signal_hooks,
};
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <path_to_a2l_file>", args[0]);
        std::process::exit(1);
    }

    let a2l_content = load_a2l_file(&args[1]).unwrap_or_else(|e| {
        eprintln!("Error: could not read '{}': {}", args[1], e);
        std::process::exit(1);
    });

    let all_names = list_signal_names(&a2l_content).unwrap_or_else(|e| {
        eprintln!("Error: failed to parse signal names: {}", e);
        std::process::exit(1);
    });
    if all_names.is_empty() {
        eprintln!("No MEASUREMENT signals found in the file.");
        std::process::exit(1);
    }

    println!("Available signals ({} total):", all_names.len());
    for (i, name) in all_names.iter().enumerate() {
        println!("  [{}] {}", i + 1, name);
    }

    let selected_signals = prompt_selection(&all_names);
    if selected_signals.is_empty() {
        println!("No signals selected, exiting.");
        return;
    }

    println!("\nSelected {} signal(s):", selected_signals.len());
    for name in &selected_signals {
        println!("  - {}", name);
    }

    let (mut project, rejected): (A2lProject, _) =
        parse_a2l_with_rejects(&a2l_content, &selected_signals).unwrap_or_else(|e| {
            eprintln!("Error: failed to parse A2L file: {}", e);
            std::process::exit(1);
        });

    let available_protocols = list_protocols(&a2l_content).unwrap_or_default();
    let protocol = if available_protocols.is_empty() {
        eprintln!("Error: no supported protocols found in the A2L file.");
        std::process::exit(1);
    } else if available_protocols.len() == 1 {
        available_protocols[0]
    } else {
        println!("\nAvailable protocols:");
        for (i, p) in available_protocols.iter().enumerate() {
            println!("  [{}] {}", i + 1, p.name());
        }
        let choice: usize = prompt_parse(
            &format!(
                "Select protocol [1-{}] (default: 1): ",
                available_protocols.len()
            ),
            1,
        );
        if choice == 0 || choice > available_protocols.len() {
            eprintln!("Invalid selection, exiting.");
            std::process::exit(1);
        }
        available_protocols[choice - 1]
    };

    println!("\nProtocol: {}", protocol.name());

    for unread in a2l_daq::unread_keywords(&a2l_content) {
        eprintln!("Warning: {unread}");
    }

    // Selected signals that did not survive parsing, or cannot be measured.
    for warning in a2l_daq::dropped_signals(&project, &selected_signals, &rejected) {
        eprintln!("Warning: {}", warning);
    }

    // Assign signals to event groups
    let assignments = assign_signals_to_events(&project, protocol);
    println!("\nEvent assignments:");
    for (event, signals) in &assignments {
        println!("  Event '{}': {} signal(s)", event, signals.len());
        for signal in signals {
            println!("    - {}", signal);
        }
    }

    // Source A2L file name, embedded as a traceability comment in the DBC/t-script.
    let source_name = std::path::Path::new(&args[1])
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&args[1])
        .to_string();

    // Core pipeline: parse output + assignments -> DAQ lists + DBC + warnings.
    // Stamps `project` with the assignments, so the t-script below builds from the same mapping.
    let core =
        pipeline::build(&mut project, &assignments, protocol, &source_name).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        });

    if !core.warnings.is_empty() {
        eprintln!("\n{} warning(s):", core.warnings.len());
        for w in &core.warnings {
            eprintln!("  - {}", w);
        }
        eprintln!();
    }

    // DBC, written next to the input.
    let a2l_stem = strip_a2l_extension(&args[1]);
    let dbc_path = format!("{}.dbc", a2l_stem);
    if let Err(e) = fs::write(&dbc_path, &core.dbc) {
        eprintln!("Error: could not write DBC file '{}': {}", dbc_path, e);
        std::process::exit(1);
    }
    println!("\nDBC file written to: {}", dbc_path);

    // Tscript gen
    if prompt_yes_no("\nConfigure t-script? [Y/n]: ", true) {
        let config = prompt_tscript_config(&core.daq_lists, &project, protocol, &source_name);
        let tscript = generate_tscript(&config);
        let t_path = format!("{}.t", a2l_stem_short(&a2l_stem));
        if let Err(e) = fs::write(&t_path, &tscript) {
            eprintln!("Error: could not write t-script '{}': {}", t_path, e);
            std::process::exit(1);
        }
        println!("t-script written to: {}", t_path);
        // A standalone script carries its own decoding, so scc needs no database.
        let reads_dbc = config.signals.is_empty()
            || config
                .signals
                .iter()
                .any(|h| matches!(h.message, MessageRef::Dbc(_)));
        let dbase = reads_dbc.then_some(dbc_path.as_str());
        compile_tscript(&t_path, dbase);
    }
}

fn prompt_selection(all_names: &[String]) -> Vec<String> {
    loop {
        let line = prompt_line(
            "\nEnter signal numbers or names (comma/space separated, or leave empty to select all): ",
        );

        if line.is_empty() {
            return all_names.to_vec();
        }

        let names: Vec<&str> = all_names.iter().map(String::as_str).collect();
        let (picked, invalid) = pick(&line, &names);
        let mut selected: Vec<String> = picked.into_iter().map(|i| all_names[i].clone()).collect();

        if !invalid.is_empty() {
            println!("Unknown entries (ignored): {}", invalid.join(", "));
        }

        selected.dedup();
        if !selected.is_empty() {
            return selected;
        }
        println!("No valid signals matched, please try again.");
    }
}

/// The indices into `names` of each comma/space separated 1-based number or name in `line`, and the parts that match nothing.
fn pick(line: &str, names: &[&str]) -> (Vec<usize>, Vec<String>) {
    let mut picked = Vec::new();
    let mut invalid = Vec::new();
    for part in line
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
    {
        let index = match part.parse::<usize>() {
            Ok(n) => n.checked_sub(1).filter(|&i| i < names.len()),
            Err(_) => names.iter().position(|n| *n == part),
        };
        match index {
            Some(i) => picked.push(i),
            None => invalid.push(part.to_string()),
        }
    }
    (picked, invalid)
}

fn assign_signals_to_events(
    project: &A2lProject,
    protocol: Protocol,
) -> Vec<(String, Vec<String>)> {
    let mut remaining_signals = project.signals.clone();

    // XCP EVENTs or CCP RASTERs, whichever the selected protocol samples on.
    let events = event_channels(project, protocol);
    if events.is_empty() {
        println!(
            "\nThe A2L declares no {} event channels; all signals go into one unassigned DAQ list.",
            protocol.name()
        );
        return Vec::new();
    }

    println!("\nAssign signals to events...");
    println!("Available events:");
    for (i, event) in events.iter().enumerate() {
        println!("  [{}] {}", i + 1, event.name);
    }

    let mut assignments = Vec::new();

    for event in events {
        println!("\nAssign signals to event '{}':", event.name);

        if remaining_signals.is_empty() {
            println!("No signals left to assign.");
            break;
        }

        println!("Available signals:");
        for (i, signal) in remaining_signals.iter().enumerate() {
            println!("  [{}] {}", i + 1, signal.name);
        }

        let line =
            prompt_line("Enter signal numbers or names (comma/space separated, empty to skip): ");

        if line.is_empty() {
            continue;
        }

        let names: Vec<&str> = remaining_signals.iter().map(|s| s.name.as_str()).collect();
        let (mut indices_to_remove, _) = pick(&line, &names);
        let assigned_signals: Vec<String> = indices_to_remove
            .iter()
            .map(|&i| remaining_signals[i].name.clone())
            .collect();

        // Remove assigned signals
        indices_to_remove.sort_unstable();
        indices_to_remove.dedup();
        for &idx in indices_to_remove.iter().rev() {
            remaining_signals.remove(idx);
        }

        if !assigned_signals.is_empty() {
            assignments.push((event.name.clone(), assigned_signals));
        }
    }

    assignments
}

fn prompt_tscript_config(
    daq_lists: &[DaqList],
    project: &A2lProject,
    protocol: Protocol,
    source_name: &str,
) -> TScriptConfig {
    let channel_index: u32 = prompt_parse("CAN channel index [0]: ", 0);

    // Bitrate and (if declared) CAN FD default from the A2L transport block.
    let defaults = bus_defaults(project, protocol);
    let bitrate: u32 = prompt_parse(
        &format!("CAN bitrate [{}]: ", defaults.bitrate),
        defaults.bitrate,
    );

    // CAN FD: only offered when the A2L declares a CAN_FD block.
    let data_bitrate: Option<u32> = defaults.can_fd_data_bitrate.map(|default_data| {
        prompt_parse(
            &format!("CAN FD detected. Data-phase bitrate [{}]: ", default_data),
            default_data,
        )
    });

    let pretrigger_ms: u32 = prompt_parse("Pretrigger (ms) [200]: ", 200);

    let timer_ms = if prompt_yes_no("Use timer-based stop? [Y/n]: ", true) {
        Some(prompt_parse("  Stop after (ms) [8000]: ", 8000))
    } else {
        None
    };
    // 0 is no limit / no watchdog
    let max_captures = timer_ms
        .map(|_| prompt_parse("  Captures before DAQ stops (0 = no limit) [1]: ", 1u32))
        .filter(|&n| n > 0);
    let daq_watchdog_ms = Some(prompt_parse(
        "Restart the DAQ init after this many ms without a DAQ frame (0 = never) [10000]: ",
        10_000u32,
    ))
    .filter(|&ms| ms > 0);

    // Collect all (sanitized_name, original_name, pid) for display.
    let mut signal_pids: Vec<(String, String, u16)> = Vec::new();
    for daq in daq_lists {
        for (odt_idx, odt) in daq.odts.iter().enumerate() {
            let pid = daq.first_pid + odt_idx as u16;
            for entry in &odt.entries {
                for name in &entry.names {
                    signal_pids.push((sanitize_name(name), name.clone(), pid));
                }
            }
        }
    }

    println!("\nAvailable signals (use these names or their index in trigger expressions):");
    for (idx, (san, orig, pid)) in signal_pids.iter().enumerate() {
        if san == orig {
            println!("  [{}] {}  (PID {})", idx + 1, san, pid);
        } else {
            println!("  [{}] {}  (original: {}, PID {})", idx + 1, san, orig, pid);
        }
    }

    let signal_names: Vec<&str> = signal_pids.iter().map(|(san, _, _)| san.as_str()).collect();

    // Standalone mode moves the signal decoding into the script, so the DBC is only needed for decoding logs elsewhere
    let standalone = prompt_yes_no(
        "\nDecode signals inside the t-script, so it compiles without the DBC? [y/N]: ",
        false,
    );

    // The raw expression plus the action, before deciding how to compile it.
    let asked = {
        let raw_expr = prompt_line(
            "\nTrigger expression (SIGNAL.Phys or N.Phys where N is the index, empty to skip): ",
        );
        if raw_expr.is_empty() {
            None
        } else {
            let expr = expand_signal_indices(&raw_expr, &signal_names);
            if expr != raw_expr {
                println!("Expanded expression: {}", expr);
            }
            let action = if prompt_line("Action [start/stop] (default: start): ")
                .eq_ignore_ascii_case("stop")
            {
                TriggerAction::StopLogger
            } else {
                TriggerAction::StartLogger
            };
            Some((expr, action))
        }
    };

    let stop_expr = prompt_line(
        "Stop expression: stops logging and DAQ for good when true (same syntax, empty to skip): ",
    );
    let stop_can_id = loop {
        let line = prompt_line(
            "Stop frame CAN id: stops for good when it arrives (0x... or decimal, empty for none): ",
        );
        if line.is_empty() {
            break None;
        }
        let parsed = match line.strip_prefix("0x").or_else(|| line.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => line.parse().ok(),
        };
        match parsed.filter(|&id| id <= 0x1FFF_FFFF) {
            Some(id) => match a2l_daq::stop_can_id_conflict(id, project, daq_lists, protocol) {
                Some(why) => eprintln!("{why}; pick another id."),
                None => break Some(id),
            },
            None => eprintln!("Not a CAN id: {line}"),
        }
    };

    let default_can_id = a2l_daq::layout::daq_can_id(project, protocol).unwrap_or(0);
    let stop_resolved: Option<ResolvedTrigger> = if stop_expr.is_empty() {
        None
    } else {
        let expr = expand_signal_indices(&stop_expr, &signal_names);
        build_trigger(&expr, standalone, project, daq_lists, default_can_id)
            .map_err(|e| eprintln!("\nStop condition dropped: {e}"))
            .ok()
    };

    let trigger_resolved: Option<(ResolvedTrigger, TriggerAction)> = match asked {
        None => None,
        Some((expr, action)) => {
            match build_trigger(&expr, standalone, project, daq_lists, default_can_id) {
                Ok(resolved) => Some((resolved, action)),
                Err(e) => {
                    eprintln!("\nTrigger dropped: {e}");
                    eprintln!(
                        "A trigger accepts `SIGNAL.Phys <op> <number>` terms joined by `&&`."
                    );
                    None
                }
            }
        }
    };

    // DAQ initialisation. Only offered when transport info is present.
    let has_transport = match protocol {
        Protocol::Xcp => project
            .xcp_data
            .as_ref()
            .and_then(|x| x.xcp_info.as_ref())
            .is_some(),
        Protocol::Ccp => project.ccp_data.is_some(),
    };
    let daq_init = if has_transport
        && prompt_yes_no(
            &format!(
                "\nGenerate {} DAQ initialisation commands? [Y/n]: ",
                protocol.name()
            ),
            true,
        ) {
        let init = build_daq_init(protocol, project, daq_lists);
        if init.is_none() {
            eprintln!(
                "Skipping {} init: A2L is missing the master/slave CAN identifiers.",
                protocol.name()
            );
        }
        init
    } else {
        None
    };

    let combo = trigger_resolved.as_ref().map(|(r, a)| (r, *a));
    let signals = resolve_signal_hooks(
        standalone,
        project,
        daq_lists,
        protocol,
        combo,
        stop_resolved.as_ref(),
    )
    .unwrap_or_else(|e| {
        eprintln!("Cannot build a standalone script: {e}. Falling back to DBC mode.");
        dbc_signal_hooks(project, daq_lists, protocol, combo, stop_resolved.as_ref())
    });
    let decoders = a2l_daq::merge_decoders(
        trigger_resolved
            .map(|(r, _)| r.decoders)
            .unwrap_or_default(),
        stop_resolved.map(|r| r.decoders).unwrap_or_default(),
    );

    TScriptConfig {
        max_captures,
        daq_watchdog_ms,
        stop_can_id,
        channel: CanChannel {
            index: channel_index,
            bitrate,
            data_bitrate,
        },
        pretrigger_ms,
        signals,
        timer_ms,
        daq_init,
        traceability: a2l_daq::traceability_lines(source_name, project, daq_lists, protocol),
        decoders,
    }
}

fn compile_tscript(t_path: &str, dbc_path: Option<&str>) {
    let scc = r"C:\Program Files (x86)\Kvaser\Canlib\Bin\scc.exe";
    let dbase_arg = dbc_path.map(|p| format!("-dbase={p}"));
    if !std::path::Path::new(scc).exists() {
        println!("scc.exe not found - compile manually:");
        match &dbase_arg {
            Some(arg) => println!("  \"{}\" {} {}", scc, arg, t_path),
            None => println!("  \"{}\" {}", scc, t_path),
        }
        return;
    }
    println!("Compiling t-script...");
    match std::process::Command::new(scc)
        .args(dbase_arg.as_deref())
        .arg(t_path)
        .status()
    {
        Ok(s) if s.success() => println!("Compiled successfully."),
        Ok(s) => eprintln!("scc.exe exited with status: {}", s),
        Err(e) => eprintln!("Failed to run scc.exe: {}", e),
    }
}

/// Print `label`, flush stdout, then read and return one trimmed line from stdin.
/// Returns an empty string on EOF; exits on a read error.
fn prompt_line(label: &str) -> String {
    print!("{}", label);
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) => String::new(),
        Ok(_) => line.trim().to_string(),
        Err(e) => {
            eprintln!("Error: failed to read input: {}", e);
            std::process::exit(1);
        }
    }
}

/// Prompt for a value parsed from the input, falling back to `default` on empty input.
fn prompt_parse<T: std::str::FromStr + std::fmt::Display>(label: &str, default: T) -> T {
    let line = prompt_line(label);
    if line.is_empty() {
        return default;
    }
    match line.parse() {
        Ok(value) => value,
        Err(_) => {
            eprintln!("  '{line}' is not a valid value, using {default}.");
            default
        }
    }
}

/// Prompt a yes/no question. Only an explicit "n"/"no" is treated as no;
/// empty or unrecognised input returns `default_yes`.
fn prompt_yes_no(label: &str, default_yes: bool) -> bool {
    match prompt_line(label).to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

fn load_a2l_file(path: &str) -> Result<String, io::Error> {
    let bytes = fs::read(path)?;
    Ok(a2l_daq::decode_a2l_bytes(&bytes))
}

/// Drop a trailing `.a2l`/`.A2L` from `path`, leaving the rest untouched.
fn strip_a2l_extension(path: &str) -> String {
    match path.len().checked_sub(4) {
        Some(cut) if path[cut..].eq_ignore_ascii_case(".a2l") => path[..cut].to_string(),
        _ => path.to_string(),
    }
}

/// How many characters of the script's base name the logger keeps.
const MAX_SCRIPT_NAME_CHARS: usize = 8;

/// The `.t` file's base name: the input's, reduced to what the logger will accept.
fn a2l_stem_short(a2l_stem: &str) -> String {
    let base = std::path::Path::new(a2l_stem)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(a2l_stem);
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem: String = clean.chars().take(MAX_SCRIPT_NAME_CHARS).collect();
    let dir = std::path::Path::new(a2l_stem)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("");
    if dir.is_empty() {
        stem
    } else {
        format!("{}/{}", dir, stem)
    }
}

#[cfg(test)]
mod tests {
    use super::pick;

    #[test]
    fn pick_takes_numbers_and_names_and_reports_the_rest() {
        let names = ["Rpm", "Speed", "Temp"];
        let (picked, invalid) = pick("3, Speed  9 0 Gear", &names);
        assert_eq!(picked, [2, 1]);
        assert_eq!(invalid, ["9", "0", "Gear"]);
    }
}
