//! Rendering a [`TScriptConfig`] into `.t` source.

use super::inline_signals;
use super::trigger::{TriggerSignal, condition_expression, latch_block, latch_variables};
use super::{DaqInit, MessageRef, SignalHook, TScriptConfig};
use protocol_core::protocol::BytePattern;
use std::collections::HashSet;
use std::fmt::Write as FmtWrite;

const DAQ_INIT_MAX_RETRIES: u32 = 10;
/// Pause before the DAQ init starts over from its first command.
const DAQ_INIT_RESTART_MS: u32 = 5000;

/// Generate the `.t` script content from a config.
pub fn generate_tscript(config: &TScriptConfig) -> String {
    let standalone = config
        .signals
        .first()
        .is_some_and(|h| matches!(h.message, MessageRef::Id { .. }));

    let mut out = String::new();
    out.push_str(&header_comment(config, standalone));
    out.push_str(&variables_block(config));
    out.push_str(&inline_signals::decoder_functions(&config.decoders));
    if let Some(init) = &config.daq_init {
        out.push_str(&daq_functions_block(config, init));
    }
    out.push_str(&end_daq_function(config));
    out.push_str(&on_start_block(config));
    out.push_str("/*\n    Signal Hook\n*/\n\n");
    for hook in &config.signals {
        out.push_str(&signal_hook_block(config, hook));
    }
    out.push_str(&stop_frame_blocks(config));
    if config.timer_ms.is_some() {
        out.push_str(&log_timer_block(config));
    }
    if config.daq_init.is_some() {
        out.push_str(&daq_timer_blocks(config));
    }
    out.push_str(&stop_and_exception_block());
    out
}

/// The header comment: script description, traceability lines, and the compile instructions
fn header_comment(config: &TScriptConfig, standalone: bool) -> String {
    let mut out = String::new();
    out.push_str("/*\n\nMemorator Logger\n\nCan only be used in program slot 0\n");
    if !config.traceability.is_empty() {
        out.push_str("\n--- Generated from ---\n");
        for line in &config.traceability {
            let _ = writeln!(out, "{line}");
        }
    }
    out.push_str(&behaviour_lines(config));
    out.push_str("\n--- Compiling ---\n");
    if standalone {
        out.push_str(
            "This script decodes the DAQ frames itself and needs no DBC file:\n\
             \x20 scc.exe <this file>\n",
        );
    } else {
        out.push_str(
            "Signal values come from the generated DBC file, which must be compiled in alongside this script:\n\
             \x20 scc.exe -dbase=<the generated .dbc> <this file>\n",
        );
    }
    out.push_str("\n*/\n\n");
    out
}

/// The header's description of when the script logs and how it recovers.
fn behaviour_lines(config: &TScriptConfig) -> String {
    let mut out = String::from("\n--- Behaviour ---\n");
    if let Some(ms) = config.timer_ms {
        let _ = writeln!(
            out,
            "Starts the logger when the trigger condition is true and stops it {ms} ms later."
        );
        out.push_str(
            "The next capture starts once the condition has been false and turns true again.\n",
        );
        match config.max_captures {
            Some(max) => {
                let _ = writeln!(
                    out,
                    "DAQ keeps running between captures; after capture {max} the script stops DAQ on the ECU."
                );
            }
            None => {
                out.push_str("DAQ keeps running between captures, with no limit on their number.\n")
            }
        }
    } else {
        out.push_str(
            "Starts the logger when the trigger condition is true and keeps it running.\n",
        );
    }
    if config.signals.iter().any(|h| h.stop.is_some()) {
        out.push_str("Stops logging and DAQ for good when the stop condition is true.\n");
    }
    if let Some(id) = config.stop_can_id {
        let _ = writeln!(
            out,
            "Stops logging and DAQ for good when a frame with CAN id 0x{id:X} arrives."
        );
    }
    out.push_str("Stopping the script stops DAQ too. If the logger loses power or is unplugged while DAQ runs,\n");
    out.push_str(
        "the ECU keeps sending DAQ frames until it is stopped by other means or restarts.\n",
    );
    if config.daq_init.is_some() {
        let _ = writeln!(
            out,
            "If the ECU stops answering the DAQ init, the init starts over after {DAQ_INIT_RESTART_MS} ms."
        );
        if let Some(ms) = config.daq_watchdog_ms {
            // The watchdog checks once per period, so it fires one to two periods after the last frame.
            let (ms64, pause) = (u64::from(ms), u64::from(DAQ_INIT_RESTART_MS));
            let (lo, hi) = (ms64 + pause, 2 * ms64 + pause);
            let _ = writeln!(
                out,
                "Every {ms} ms the script checks that DAQ frames still arrive. If none did, e.g. after the ECU restarts,\n\
                 the DAQ init starts over {DAQ_INIT_RESTART_MS} ms later: {lo}-{hi} ms after the last DAQ frame."
            );
        }
    }
    out
}

/// The `variables{}` block: variables used in the t-script
fn variables_block(config: &TScriptConfig) -> String {
    let mut out = String::new();
    out.push_str("variables{\n");
    let ch = &config.channel;
    let _ = writeln!(out, "    const int can_channel = {};", ch.index);
    match ch.data_bitrate {
        Some(data_bitrate) => {
            let bitrate = fd_bitrate_token(ch.bitrate);
            let data_bitrate = fd_bitrate_token(data_bitrate);
            let _ = writeln!(out, "    const int bitrate = {bitrate};");
            let _ = writeln!(out, "    const int data_bitrate = {data_bitrate};");
        }
        None => {
            let _ = writeln!(out, "    const int bitrate = {};", ch.bitrate);
        }
    }
    if let Some(init) = &config.daq_init {
        let _ = writeln!(out, "    const int master_id = 0x{:X};", init.master_id);
        let _ = writeln!(out, "    const int INIT_STATES = {};", init.commands.len());
    }
    let _ = writeln!(out, "    int logging = 0;");
    let _ = writeln!(out, "    int daq_ended = 0;");
    if config.daq_init.is_some() {
        let _ = writeln!(out, "    int count = 0;");
        // -1 while a restart is pending: responses then belong to no command
        let _ = writeln!(out, "    int init_state = 0;");
        let _ = writeln!(out, "    Timer daqTimeout;");
        let _ = writeln!(out, "    Timer initRestart;");
        if config.daq_watchdog_ms.is_some() {
            let _ = writeln!(out, "    int daq_seen = 0;");
            let _ = writeln!(out, "    Timer daqWatchdog;");
        }
    }
    if config.timer_ms.is_some() {
        let _ = writeln!(out, "    Timer logTimer;");
        // A capture starts only while armed: at first, then once the condition has been false again
        let _ = writeln!(out, "    int armed = 1;");
        let _ = writeln!(out, "    int captures = 0;");
        if let Some(max) = config.max_captures {
            let _ = writeln!(out, "    const int MAX_CAPTURES = {max};");
        }
    }
    out.push_str(&inline_signals::helper_typedef(&config.decoders));
    out.push_str(&latch_variables(&trigger_signals(&config.signals)));
    out.push_str("}\n\n");
    out
}

/// Every distinct signal the triggers and stop conditions of `hooks` read, first occurrence first.
fn trigger_signals<'a>(hooks: impl IntoIterator<Item = &'a SignalHook>) -> Vec<TriggerSignal> {
    let mut seen = HashSet::new();
    hooks
        .into_iter()
        .flat_map(|hook| hook.trigger.iter().chain(&hook.stop))
        .flat_map(|condition| &condition.signals)
        .filter(|s| seen.insert(s.ident.as_str()))
        .cloned()
        .collect()
}

/// Stop the logger if it is running.
const STOP_LOGGER: &str = concat!(
    "    if (loggerStatus() == LOGGER_STATE_LOGGING) {\n",
    "        printf(\"Stopping logger...\\n\");\n",
    "        loggerStop();\n",
    "    }\n",
);

/// (Re)send the pending init command and wait for its response.
fn send_pending_command(indent: &str) -> String {
    format!("{indent}sendDaqCommand(init_state, master_id);\n{indent}timerStart(daqTimeout);\n")
}

/// Stop DAQ and logging status. Called after the last capture, by the stop condition or stop frame, and when the script stops.
fn end_daq_function(config: &TScriptConfig) -> String {
    let mut out = String::from("void endDaq() {\n");
    out.push_str("    if (daq_ended == 1) { return; }\n");
    out.push_str("    daq_ended = 1;\n");
    out.push_str(STOP_LOGGER);
    if config.timer_ms.is_some() {
        out.push_str("    timerCancel(logTimer);\n");
    }
    if let Some(init) = &config.daq_init {
        out.push_str("    timerCancel(daqTimeout);\n");
        out.push_str("    timerCancel(initRestart);\n");
        if config.daq_watchdog_ms.is_some() {
            out.push_str("    timerCancel(daqWatchdog);\n");
        }
        out.push_str("    init_state = -1;\n");
        if !init.stop_commands.is_empty() {
            out.push_str("    printf(\"Stopping DAQ\\n\");\n");
            out.push_str("    sendDaqStop(master_id);\n");
        }
    }
    out.push_str("}\n\n");
    out
}

/// The hooks for the stop frame
fn stop_frame_blocks(config: &TScriptConfig) -> String {
    let Some(can_id) = config.stop_can_id else {
        return String::new();
    };
    let selector = MessageRef::Id {
        can_id,
        extended: can_id > 0x7FF,
    }
    .hook_selector();
    let kinds: &[&str] = if config.can_fd() {
        &["CanMessage", "CanMessageFd"]
    } else {
        &["CanMessage"]
    };
    let mut out = String::new();
    for kind in kinds {
        let _ = writeln!(out, "on {kind}<*> {selector} {{");
        let _ = writeln!(out, "    printf(\"Stop frame 0x{can_id:X} received\\n\");");
        out.push_str("    endDaq();\n");
        out.push_str("}\n\n");
    }
    out
}

/// The `send`/`sendDaqCommand`/`sendDaqStop` functions that drive the DAQ-init state machine.
fn daq_functions_block(config: &TScriptConfig, init: &DaqInit) -> String {
    let mut out = String::new();
    out.push_str("void send(int id, const byte data[]) {\n");

    if config.can_fd() {
        out.push_str("    CanMessageFd msg;\n");
        out.push_str("    msg.flags = canFDMSG_FDF | canFDMSG_BRS;\n");
    } else {
        out.push_str("    CanMessage msg;\n");
        out.push_str("    msg.flags = 0;\n");
    }
    out.push_str("    msg.id = id;\n");
    out.push_str("    msg.dlc = data.count;\n");
    out.push_str("    msg.data = data;\n");
    out.push_str("    canWrite(msg);\n");
    out.push_str("}\n\n");

    // Init daq sequence switch case
    out.push_str("void sendDaqCommand(int state, int id) {\n");
    out.push_str("    switch (state) {\n");
    for (i, cmd) in init.commands.iter().enumerate() {
        let _ = writeln!(
            out,
            "    case {i}:  /* [{}] {} */",
            cmd.phase.tag(),
            cmd.label
        );
        out.push_str("        {\n");
        let _ = writeln!(
            out,
            "            byte data[{}] = {{{}}};",
            cmd.bytes.len(),
            byte_literals(&cmd.bytes)
        );
        out.push_str("            send(id, data);\n");
        out.push_str("        }\n");
        out.push_str("        break;\n");
    }
    out.push_str("    }\n");
    out.push_str("}\n\n");

    // Which init states may get an unsupported/negative response without aborting the sequence.
    if init.commands.iter().any(|c| c.optional) {
        out.push_str("int isOptionalState(int state) {\n");
        for (i, cmd) in init.commands.iter().enumerate().filter(|(_, c)| c.optional) {
            let _ = writeln!(
                out,
                "    if (state == {i}) {{ return 1; }}  /* {} */",
                cmd.label
            );
        }
        out.push_str("    return 0;\n");
        out.push_str("}\n\n");
    }

    // Single definition of the teardown sequence. The bytes are the protocol's own DISCONNECT/stop commands
    if !init.stop_commands.is_empty() {
        out.push_str("void sendDaqStop(int id) {\n");
        for (i, cmd) in init.stop_commands.iter().enumerate() {
            let _ = writeln!(
                out,
                "    byte stopData{i}[{}] = {{{}}};  /* [{}] {} */",
                cmd.bytes.len(),
                byte_literals(&cmd.bytes),
                cmd.phase.tag(),
                cmd.label
            );
            let _ = writeln!(out, "    send(id, stopData{i});");
        }
        out.push_str("}\n\n");
    }

    // Every failure ends here: the ECU may be off, restarting, or in a state the init can't continue from.
    out.push_str("void restartDaqInit() {\n");
    out.push_str("    timerCancel(daqTimeout);\n");
    if config.daq_watchdog_ms.is_some() {
        out.push_str("    timerCancel(daqWatchdog);\n");
    }
    if !init.stop_commands.is_empty() {
        out.push_str("    sendDaqStop(master_id);\n");
    }
    out.push_str("    init_state = -1;\n");
    out.push_str("    count = 0;\n");
    out.push_str("    timerStart(initRestart);\n");
    out.push_str("}\n\n");
    out
}

/// The `on start` hook. Initialize the CAN bus, Timers
fn on_start_block(config: &TScriptConfig) -> String {
    let mut out = String::new();
    out.push_str("on start {\n");
    out.push_str("    printf(\"Starting script...\\n\");\n\n");
    out.push_str("    canBusOff(can_channel);\n");
    if config.can_fd() {
        out.push_str("    canSetBusParams(can_channel, bitrate, 0, 0, 0, 0);\n");
        out.push_str("    canSetBusParamsFd(can_channel, data_bitrate, 0, 0, 0);\n");
        out.push_str("    canSetCommunicationMode(can_channel, canMODE_CAN_FD);\n");
    } else {
        out.push_str("    canSetBitrate(can_channel, bitrate);\n");
    }
    out.push_str("    canSetBusOutputControl(can_channel, canDRIVER_NORMAL);\n");
    out.push_str("    canBusOn(can_channel);\n");
    out.push('\n');
    let _ = writeln!(out, "    loggerSetPretrigger({});", config.pretrigger_ms);
    if let Some(ms) = config.timer_ms {
        let _ = writeln!(out, "    logTimer.timeout = {ms};");
    }

    if let Some(init) = &config.daq_init {
        let _ = writeln!(out, "\n    daqTimeout.timeout = {};", init.timeout_ms);
        let _ = writeln!(out, "    initRestart.timeout = {DAQ_INIT_RESTART_MS};");
        if let Some(ms) = config.daq_watchdog_ms {
            let _ = writeln!(out, "    daqWatchdog.timeout = {ms};");
        }
        out.push_str(&send_pending_command("    "));
    }
    out.push_str("}\n\n");
    out
}

fn advance_state_lines(indent: &str, protocol_name: &str, watchdog: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{indent}timerCancel(daqTimeout);");
    let _ = writeln!(out, "{indent}init_state++;");
    let _ = writeln!(out, "{indent}count = 0;");
    let _ = writeln!(out, "{indent}if (init_state < INIT_STATES) {{");
    out.push_str(&send_pending_command(&format!("{indent}    ")));
    let _ = writeln!(out, "{indent}}} else {{");
    let _ = writeln!(
        out,
        "{indent}    printf(\"{protocol_name} init complete\\n\");"
    );
    if watchdog {
        let _ = writeln!(out, "{indent}    daq_seen = 0;");
        let _ = writeln!(out, "{indent}    timerStart(daqWatchdog);");
    }
    let _ = writeln!(out, "{indent}}}");
    out
}

fn fatal_error_lines(indent: &str, protocol_name: &str, error_code_byte: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{indent}printf(\"{protocol_name} error at state %d: 0x%02x\\n\", init_state, this.data[{error_code_byte}]);"
    );
    let _ = writeln!(out, "{indent}restartDaqInit();");
    out
}

/// Ignore a response that belongs to no command: one to the stop sequence, or one after the init is done.
const NO_PENDING_COMMAND: &str = "if (init_state < 0 || init_state >= INIT_STATES) { return; }";

/// One `on CanMessage(Fd)<*>` block
fn signal_hook_block(config: &TScriptConfig, hook: &SignalHook) -> String {
    let mut out = String::new();
    let has_init = config.daq_init.is_some() && hook.handles_responses;
    let msg_kind = if config.can_fd() {
        "CanMessageFd"
    } else {
        "CanMessage"
    };
    let _ = writeln!(out, "on {msg_kind}<*> {} {{", hook.message.hook_selector());
    // Every DAQ message has a hook, so any DAQ frame feeds the watchdog.
    let watchdog = config.daq_init.is_some() && config.daq_watchdog_ms.is_some();
    if watchdog {
        out.push_str("    daq_seen = 1;\n");
    }

    // Slave response handler.
    if has_init {
        let init = config.daq_init.as_ref().expect("has_init implies daq_init");
        let format = init.protocol.response_format();
        let name = init.protocol.name();
        let has_optional = init.commands.iter().any(|c| c.optional);

        let _ = writeln!(out, "    if ({}) {{", frame_condition(format.positive));
        let _ = writeln!(out, "        {NO_PENDING_COMMAND}");
        out.push_str(&advance_state_lines("        ", name, watchdog));
        let _ = writeln!(out, "    }} else if ({}) {{", frame_condition(format.error));
        let _ = writeln!(out, "        {NO_PENDING_COMMAND}");
        if has_optional {
            out.push_str("        if (isOptionalState(init_state)) {\n");
            let _ = writeln!(
                out,
                "            printf(\"{name} optional command not supported at state %d (0x%02x), continuing\\n\", init_state, this.data[{}]);",
                format.error_code_byte
            );
            out.push_str(&advance_state_lines("            ", name, watchdog));
            out.push_str("        } else {\n");
            out.push_str(&fatal_error_lines(
                "            ",
                name,
                format.error_code_byte,
            ));
            out.push_str("        }\n");
        } else {
            out.push_str(&fatal_error_lines("        ", name, format.error_code_byte));
        }
        out.push_str("    }\n");
    }

    // DAQ message condition
    if hook.trigger.is_some() || hook.stop.is_some() {
        let indent = if has_init { "        " } else { "    " };
        if has_init {
            out.push_str("    else {\n");
        }

        out.push_str(&latch_block(&trigger_signals([hook]), indent));
        let _ = writeln!(out, "{indent}if (daq_ended == 1) {{ return; }}");
        if let Some(stop) = &hook.stop {
            let _ = writeln!(out, "{indent}if ({}) {{", condition_expression(stop));
            let _ = writeln!(out, "{indent}    printf(\"Stop condition met\\n\");");
            let _ = writeln!(out, "{indent}    endDaq();");
            let _ = writeln!(out, "{indent}    return;");
            let _ = writeln!(out, "{indent}}}");
        }
    }
    if let Some(trigger) = &hook.trigger {
        let indent = if has_init { "        " } else { "    " };
        let _ = writeln!(out, "{indent}if (logging == 1) {{ return; }}");
        let _ = writeln!(out, "{indent}if ({}) {{", condition_expression(trigger));
        if config.timer_ms.is_some() {
            // Edge-triggered: one capture per false -> true transition of the condition
            let limit = if config.max_captures.is_some() {
                " && captures < MAX_CAPTURES"
            } else {
                ""
            };
            let _ = writeln!(out, "{indent}    if (armed == 1{limit}) {{");
            let _ = writeln!(out, "{indent}        armed = 0;");
            let _ = writeln!(out, "{indent}        captures++;");
            let _ = writeln!(out, "{indent}        printf(\"Capture %d\\n\", captures);");
            for statement in trigger.action.statements() {
                let _ = writeln!(out, "{indent}        {statement}");
            }
            let _ = writeln!(out, "{indent}        logging = 1;");
            let _ = writeln!(out, "{indent}        timerStart(logTimer);");
            let _ = writeln!(out, "{indent}    }}");
            let _ = writeln!(out, "{indent}}} else {{");
            let _ = writeln!(out, "{indent}    armed = 1;");
        } else {
            for statement in trigger.action.statements() {
                let _ = writeln!(out, "{indent}    {statement}");
            }
            let _ = writeln!(out, "{indent}    logging = 1;");
        }
        let _ = writeln!(out, "{indent}}}");
    }
    if has_init && (hook.trigger.is_some() || hook.stop.is_some()) {
        out.push_str("    }\n");
    }
    out.push_str("}\n\n");
    out
}

/// The `on Timer logTimer` hook: stop the logger once the post-trigger timer expires, and DAQ after the last capture.
///
/// Only emitted when `config.timer_ms` is `Some`.
fn log_timer_block(config: &TScriptConfig) -> String {
    let mut out = String::new();
    out.push_str("on Timer logTimer {\n");
    out.push_str("    printf(\"Log timer expired\\n\");\n");
    out.push_str(STOP_LOGGER);
    out.push_str("    logging = 0;\n");
    if config.max_captures.is_some() {
        out.push_str("    if (captures >= MAX_CAPTURES) {\n");
        out.push_str("        printf(\"Last capture done\\n\");\n");
        out.push_str("        endDaq();\n");
        out.push_str("    }\n");
    }
    out.push_str("}\n\n");
    out
}

/// `daqTimeout` resends the current command up to [`DAQ_INIT_MAX_RETRIES`] times
/// before the init starts over, `initRestart` starts it over, and `daqWatchdog` restarts it when DAQ frames stop arriving.
fn daq_timer_blocks(config: &TScriptConfig) -> String {
    let mut out = String::new();
    out.push_str("on Timer daqTimeout {\n");
    let _ = writeln!(out, "    if (count < {DAQ_INIT_MAX_RETRIES}) {{");
    out.push_str("        count++;\n");
    out.push_str("    } else {\n");
    let _ = writeln!(
        out,
        "        printf(\"DAQ timeout at state %d after {DAQ_INIT_MAX_RETRIES} attempts, restarting the init\\n\", init_state);"
    );
    out.push_str("        restartDaqInit();\n");
    out.push_str("        return;\n");
    out.push_str("    }\n");
    out.push_str("    printf(\"DAQ timeout at state %d, retrying...\\n\", init_state);\n");
    out.push_str(&send_pending_command("    "));
    out.push_str("}\n\n");

    out.push_str("on Timer initRestart {\n");
    out.push_str("    printf(\"Restarting the DAQ init\\n\");\n");
    out.push_str("    init_state = 0;\n");
    out.push_str(&send_pending_command("    "));
    out.push_str("}\n\n");

    if let Some(ms) = config.daq_watchdog_ms {
        out.push_str("on Timer daqWatchdog {\n");
        out.push_str("    if (daq_seen == 0) {\n");
        let _ = writeln!(
            out,
            "        printf(\"No DAQ frame for {ms} ms, restarting the DAQ init\\n\");"
        );
        out.push_str("        restartDaqInit();\n");
        out.push_str("        return;\n");
        out.push_str("    }\n");
        out.push_str("    daq_seen = 0;\n");
        out.push_str("    timerStart(daqWatchdog);\n");
        out.push_str("}\n\n");
    }
    out
}

/// The `on stop`/`on exception` hooks. Stopping the script stops DAQ too.
fn stop_and_exception_block() -> String {
    let mut out = String::new();
    out.push_str("on stop {\n");
    out.push_str("    printf(\"Stopping script...\\n\");\n");
    out.push_str("    endDaq();\n");
    out.push_str("}\n\n");
    out.push_str("on exception {\n");
    out.push_str(
        "    printf(\"An exception occurred! error=%d line=%d\\n\", this.error, this.line);\n",
    );
    out.push_str("    scriptStop(0);\n");
    out.push_str("}\n");
    out
}

/// Render command bytes as a t-script initialiser list
fn byte_literals(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("0x{b:02X}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a [`BytePattern`] as a t-script condition over the received frame.
fn frame_condition(pattern: BytePattern) -> String {
    pattern
        .iter()
        .map(|(index, value)| format!("this.data[{index}] == 0x{value:02X}"))
        .collect::<Vec<_>>()
        .join(" && ")
}

fn fd_bitrate_token(bps: u32) -> String {
    match bps {
        500_000 => "canFD_BITRATE_500K_80P".to_string(),
        1_000_000 => "canFD_BITRATE_1M_80P".to_string(),
        2_000_000 => "canFD_BITRATE_2M_80P".to_string(),
        4_000_000 => "canFD_BITRATE_4M_80P".to_string(),
        8_000_000 => "canFD_BITRATE_8M_60P".to_string(),
        other => other.to_string(),
    }
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tscript::test_util::*;
    use crate::tscript::*;

    fn function_body<'a>(t: &'a str, head: &str) -> &'a str {
        t.split(head)
            .nth(1)
            .and_then(|rest| rest.split("\n}").next())
            .unwrap_or_else(|| panic!("no `{head}` in:\n{t}"))
    }

    #[test]
    fn generates_basic_tscript() {
        let mut config = dbc_config();
        config.channel = CanChannel {
            index: 0,
            bitrate: 1_000_000,
            data_bitrate: None,
        };
        config.pretrigger_ms = 200;
        config.signals[0].message = MessageRef::Dbc("PowertrainStatus".into());
        config.signals[0].trigger = Some(TriggerCondition {
            signals: vec![signal("EngineSpeed", 0, "this.EngineSpeed.Phys")],
            comparisons: vec![comparison("EngineSpeed", ">", "4000")],
            action: TriggerAction::StartLogger,
        });
        config.timer_ms = Some(8000);
        let tscript = generate_tscript(&config);
        assert!(tscript.contains("const int can_channel = 0;"));
        assert!(tscript.contains("const int bitrate = 1000000;"));
        assert!(tscript.contains("loggerSetPretrigger(200);"));
        assert!(tscript.contains("on CanMessage<*> PowertrainStatus {"));
        assert!(tscript.contains("val_EngineSpeed = this.EngineSpeed.Phys;"));
        assert!(tscript.contains("if (got_EngineSpeed == 1 && val_EngineSpeed > 4000) {"));
        assert!(tscript.contains("printf(\"Starting logger...\");"));
        assert!(tscript.contains("loggerStart();"));
        assert!(tscript.contains("Timer logTimer;"));
        assert!(tscript.contains("logTimer.timeout = 8000;"));
    }

    #[test]
    fn traceability_lines_appear_in_header() {
        let mut config = dbc_config();
        config.traceability = vec!["Source A2L: MyEcu.a2l".into(), "Protocol: XCP".into()];
        let t = generate_tscript(&config);
        assert!(t.contains("--- Generated from ---"), "got:\n{t}");
        assert!(t.contains("Source A2L: MyEcu.a2l"), "got:\n{t}");
        let header_end = t.find("*/").expect("header comment close");
        assert!(t.find("Source A2L").unwrap() < header_end, "got:\n{t}");
    }

    #[test]
    fn generates_can_fd_bus_config_and_frame_flags() {
        let mut config = init_config(
            Protocol::Xcp,
            vec![cmd(vec![0xFF, 0x00], "CONNECT")],
            vec![stop_cmd(vec![0xFE], "DISCONNECT")],
        );
        config.channel = CanChannel {
            index: 0,
            bitrate: 1_000_000,
            data_bitrate: Some(2_000_000),
        };
        config.pretrigger_ms = 200;
        config.signals[0].message = MessageRef::Dbc("Test".into());
        config.timer_ms = None;
        let t = generate_tscript(&config);
        assert!(t.contains("const int bitrate = canFD_BITRATE_1M_80P;"));
        assert!(t.contains("const int data_bitrate = canFD_BITRATE_2M_80P;"));
        assert!(t.contains("canSetBusParams(can_channel, bitrate, 0, 0, 0, 0);"));
        assert!(t.contains("canSetBusParamsFd(can_channel, data_bitrate, 0, 0, 0);"));
        assert!(t.contains("canSetCommunicationMode(can_channel, canMODE_CAN_FD);"));
        assert!(t.contains("CanMessageFd msg;"));
        assert!(t.contains("msg.flags = canFDMSG_FDF | canFDMSG_BRS;"));
    }

    #[test]
    fn unknown_fd_bitrate_falls_back_to_raw_frequency() {
        assert_eq!(fd_bitrate_token(2_000_000), "canFD_BITRATE_2M_80P");
        assert_eq!(fd_bitrate_token(1_500_000), "1500000");
    }

    #[test]
    fn no_xcp_init_omits_response_handler() {
        let mut config = dbc_config();
        config.pretrigger_ms = 200;
        config.signals[0].trigger = Some(TriggerCondition {
            signals: vec![signal("Speed", 0, "this.Speed.Phys")],
            comparisons: vec![comparison("Speed", ">", "10")],
            action: TriggerAction::StartLogger,
        });
        let t = generate_tscript(&config);
        assert!(!t.contains("init_state"), "got:\n{t}");
        assert!(!t.contains("INIT_STATES"));
        assert!(
            t.contains("if (got_Speed == 1 && val_Speed > 10) {"),
            "got:\n{t}"
        );
    }

    #[test]
    fn classic_can_omits_fd_calls() {
        let mut config = dbc_config();
        config.pretrigger_ms = 200;
        config.signals[0].message = MessageRef::Dbc("Test".into());
        let t = generate_tscript(&config);
        assert!(!t.contains("canMODE_CAN_FD"));
        assert!(!t.contains("data_bitrate"));
    }

    #[test]
    fn generates_xcp_init_state_machine() {
        let mut config = dbc_config();
        config.pretrigger_ms = 200;
        config.signals[0].message = MessageRef::Dbc("Test".into());
        config.daq_init = Some(DaqInit {
            protocol: Protocol::Xcp,
            master_id: 0x51,
            timeout_ms: 200,
            commands: vec![
                cmd(vec![0xFF, 0x00], "CONNECT"),
                cmd(vec![0xFD], "GET_STATUS"),
                cmd(vec![0xDD, 0x01], "START_STOP_SYNCH"),
            ],
            stop_commands: vec![
                stop_cmd(vec![0xDD, 0x00], "START_STOP_SYNCH  stop all"),
                stop_cmd(vec![0xFE], "DISCONNECT"),
            ],
        });
        let t = generate_tscript(&config);
        assert!(t.contains("const int master_id = 0x51;"));
        assert!(!t.contains("slave_id"), "the script never uses it");
        assert!(t.contains("const int INIT_STATES = 3;"));
        assert!(t.contains("Timer daqTimeout;"));
        assert!(t.contains("void send(int id, const byte data[]) {"));
        assert!(t.contains("msg.flags = 0;"));
        assert!(t.contains("void sendDaqCommand(int state, int id) {"));
        assert!(t.contains("    switch (state) {"));
        assert!(t.contains("    case 0:"));
        assert!(t.contains("byte data[2] = {0xFF, 0x00};"));
        assert!(t.contains("        break;"));
        assert!(t.contains("byte data[1] = {0xFD};"));
        assert!(t.contains("daqTimeout.timeout = 200;"));
        assert!(t.contains("sendDaqCommand(init_state, master_id);"));
        assert!(t.contains("on CanMessage<*> Test {"));
        assert!(t.contains("if (this.data[0] == 0xFF) {"));
        assert!(t.contains("} else if (this.data[0] == 0xFE) {"));
        assert!(
            t.contains("printf(\"XCP error at state %d: 0x%02x\\n\", init_state, this.data[1]);"),
            "got:\n{t}"
        );
        assert!(t.contains("on Timer daqTimeout"));
    }

    #[test]
    fn response_handler_follows_the_protocol_response_format() {
        let ccp = generate_tscript(&init_config(
            Protocol::Ccp,
            vec![cmd(vec![0x01, 0x00, 0x00, 0x01], "CONNECT")],
            vec![stop_cmd(
                vec![0x07, 0x01, 0x01, 0x00, 0x00, 0x01],
                "DISCONNECT",
            )],
        ));
        assert!(
            ccp.contains("if (this.data[0] == 0xFF && this.data[1] == 0x00) {"),
            "CCP acknowledges with PID 0xFF and error code 0x00, got:\n{ccp}"
        );
        assert!(
            ccp.contains("} else if (this.data[0] == 0xFF) {"),
            "got:\n{ccp}"
        );
        assert!(
            ccp.contains("printf(\"CCP init complete\\n\");"),
            "got:\n{ccp}"
        );
        assert!(
            ccp.contains("printf(\"CCP error at state %d: 0x%02x\\n\", init_state, this.data[1]);"),
            "got:\n{ccp}"
        );
        assert!(
            !ccp.contains("0xFE"),
            "no XCP frame bytes in a CCP script:\n{ccp}"
        );
    }

    #[test]
    fn every_failure_restarts_the_init_after_the_stop_commands() {
        for protocol in Protocol::ALL {
            let t = generate_tscript(&init_config(
                protocol,
                vec![cmd(vec![0x01, 0x00], "CONNECT")],
                vec![
                    stop_cmd(vec![0x08, 0x00, 0x00], "START_STOP_ALL  stop all"),
                    stop_cmd(vec![0x07, 0x01], "DISCONNECT"),
                ],
            ));
            assert!(
                t.contains("void sendDaqStop(int id) {"),
                "{protocol:?}:\n{t}"
            );
            assert!(
                t.contains("    byte stopData0[3] = {0x08, 0x00, 0x00};  /* [stop] START_STOP_ALL  stop all */"),
                "{protocol:?}:\n{t}"
            );
            assert!(
                t.contains("    byte stopData1[2] = {0x07, 0x01};"),
                "{protocol:?}:\n{t}"
            );
            let restart = function_body(&t, "void restartDaqInit() {");
            assert!(
                restart.contains("sendDaqStop(master_id);"),
                "{protocol:?}:\n{t}"
            );
            assert!(restart.contains("init_state = -1;"), "{protocol:?}:\n{t}");
            assert_eq!(
                t.matches("restartDaqInit();").count(),
                2,
                "an error response and exhausted retries both restart, {protocol:?}:\n{t}"
            );
            assert!(
                t.contains("on Timer initRestart {\n    printf(\"Restarting the DAQ init\\n\");\n    init_state = 0;"),
                "{protocol:?}:\n{t}"
            );
        }
    }

    #[test]
    fn responses_outside_a_pending_command_are_ignored() {
        let t = generate_tscript(&init_config(
            Protocol::Xcp,
            vec![cmd(vec![0xFF, 0x00], "CONNECT")],
            vec![stop_cmd(vec![0xFE], "DISCONNECT")],
        ));
        let guard = "if (init_state < 0 || init_state >= INIT_STATES) { return; }";
        assert!(
            t.contains(&format!("if (this.data[0] == 0xFF) {{\n        {guard}\n")),
            "positive responses, got:\n{t}"
        );
        assert!(
            t.contains(&format!(
                "}} else if (this.data[0] == 0xFE) {{\n        {guard}\n"
            )),
            "error responses, got:\n{t}"
        );
    }

    #[test]
    fn abort_calls_omitted_when_there_are_no_stop_commands() {
        let t = generate_tscript(&init_config(
            Protocol::Xcp,
            vec![cmd(vec![0xFF, 0x00], "CONNECT")],
            vec![],
        ));
        assert!(!t.contains("sendDaqStop"), "got:\n{t}");
    }

    #[test]
    fn optional_command_error_advances_instead_of_aborting() {
        let t = generate_tscript(&init_config(
            Protocol::Xcp,
            vec![
                cmd(vec![0xFF, 0x00], "CONNECT"),
                optional_cmd(vec![0xF8, 0x00, 0x04], "GET_SEED"),
            ],
            vec![stop_cmd(vec![0xFE], "DISCONNECT")],
        ));

        assert!(t.contains("int isOptionalState(int state) {"), "got:\n{t}");
        assert!(
            t.contains("if (state == 1) { return 1; }  /* GET_SEED */"),
            "got:\n{t}"
        );
        assert!(
            t.contains("if (isOptionalState(init_state)) {"),
            "got:\n{t}"
        );
        assert!(
            t.contains("optional command not supported at state %d"),
            "got:\n{t}"
        );
        let optional_branch = t
            .split("if (isOptionalState(init_state)) {")
            .nth(1)
            .unwrap()
            .split("} else {")
            .next()
            .unwrap();
        assert!(optional_branch.contains("init_state++;"), "got:\n{t}");
        assert!(!optional_branch.contains("restartDaqInit"), "got:\n{t}");
    }

    #[test]
    fn no_optional_state_helper_when_nothing_is_optional() {
        let t = generate_tscript(&init_config(
            Protocol::Xcp,
            vec![cmd(vec![0xFF, 0x00], "CONNECT")],
            vec![stop_cmd(vec![0xFE], "DISCONNECT")],
        ));
        assert!(!t.contains("isOptionalState"), "got:\n{t}");
    }

    #[test]
    fn trigger_block_is_indented_inside_the_response_handler_else() {
        let mut config = init_config(
            Protocol::Xcp,
            vec![cmd(vec![0xFF, 0x00], "CONNECT")],
            vec![stop_cmd(vec![0xFE], "DISCONNECT")],
        );
        config.signals[0].trigger = Some(TriggerCondition {
            signals: vec![signal("Speed", 0, "this.Speed.Phys")],
            comparisons: vec![comparison("Speed", ">", "10")],
            action: TriggerAction::StartLogger,
        });
        let t = generate_tscript(&config);
        assert!(
            t.contains("    else {\n        if (this.data[0] == 0x00) {"),
            "got:\n{t}"
        );
        assert!(
            t.contains("        if (logging == 1) { return; }"),
            "got:\n{t}"
        );
        assert!(
            t.contains("        if (got_Speed == 1 && val_Speed > 10) {\n"),
            "got:\n{t}"
        );
        assert!(
            t.contains("            printf(\"Starting logger...\");\n"),
            "got:\n{t}"
        );
        assert!(t.contains("            loggerStart();\n"), "got:\n{t}");
        assert!(
            t.contains("            timerStart(logTimer);\n"),
            "got:\n{t}"
        );
    }

    #[test]
    fn dbc_mode_gates_the_trigger_on_the_pid() {
        let (project, daq_lists) = two_odt_project();
        let reads = vec![
            ("Rpm".to_string(), Accessor::Phys),
            ("Temp".to_string(), Accessor::Phys),
        ];
        let mut config = dbc_config();
        config.signals[0].trigger = Some(TriggerCondition {
            signals: dbc_trigger_signals(&project, &daq_lists, &reads).unwrap(),
            comparisons: vec![
                comparison("Rpm", ">", "3000"),
                comparison("Temp", "<", "90"),
            ],
            action: TriggerAction::StartLogger,
        });
        let t = generate_tscript(&config);

        assert!(t.contains("on CanMessage<*> DAQ {"), "got:\n{t}");
        assert!(t.contains("scc.exe -dbase="), "got:\n{t}");
        assert!(!t.contains("float sig_"), "no inline decoders:\n{t}");

        assert!(
            t.contains("    if (this.data[0] == 0x01) {\n        val_Rpm = this.Rpm.Phys;"),
            "got:\n{t}"
        );
        assert!(
            t.contains("    if (this.data[0] == 0x02) {\n        val_Temp = this.Temp.Phys;"),
            "got:\n{t}"
        );
        assert!(
            t.contains(
                "    if (got_Rpm == 1 && got_Temp == 1 && val_Rpm > 3000 && val_Temp < 90) {"
            ),
            "got:\n{t}"
        );
        assert!(
            !t.contains("if (this.Rpm.Phys > 3000"),
            "ungated read must be gone:\n{t}"
        );
    }

    #[test]
    fn standalone_script_hooks_the_raw_can_id_and_needs_no_dbc() {
        let t = generate_tscript(&standalone_config(0x456, false));
        assert!(t.contains("on CanMessage<*> (0x456) {"), "got:\n{t}");
        assert!(!t.contains("<*> DAQ"), "no DBC message name:\n{t}");
        assert!(t.contains("needs no DBC file"), "got:\n{t}");
        assert!(!t.contains("-dbase="), "got:\n{t}");
    }

    #[test]
    fn standalone_script_marks_extended_identifiers() {
        let t = generate_tscript(&standalone_config(0x1ABCDEF, true));
        assert!(t.contains("on CanMessage<*> (0x1ABCDEF)x {"), "got:\n{t}");
    }

    #[test]
    fn standalone_script_latches_per_pid_then_ands_the_conditions() {
        let t = generate_tscript(&standalone_config(0x456, false));

        assert!(
            t.contains("float sig_Rpm(const byte data[]) {"),
            "got:\n{t}"
        );
        assert!(
            t.contains("float sig_Temp(const byte data[]) {"),
            "got:\n{t}"
        );
        assert!(t.contains("    float val_Rpm = 0;"), "got:\n{t}");
        assert!(t.contains("    int got_Rpm = 0;"), "got:\n{t}");

        assert!(
            t.contains("    if (this.data[0] == 0x01) {\n        val_Rpm = sig_Rpm(this.data);"),
            "got:\n{t}"
        );
        assert!(
            t.contains("    if (this.data[0] == 0x02) {\n        val_Temp = sig_Temp(this.data);"),
            "got:\n{t}"
        );

        assert!(
            t.contains(
                "    if (got_Rpm == 1 && got_Temp == 1 && val_Rpm > 3000 && val_Temp < 90) {"
            ),
            "got:\n{t}"
        );

        let latch = t.find("val_Rpm = sig_Rpm").expect("latch");
        let ret = t
            .find("if (logging == 1) { return; }")
            .expect("early return");
        assert!(latch < ret, "latch must precede the early return:\n{t}");
    }

    #[test]
    fn standalone_script_coexists_with_the_daq_init_state_machine() {
        let mut config = standalone_config(0x456, false);
        config.daq_init = Some(DaqInit {
            protocol: Protocol::Xcp,
            master_id: 0x455,
            timeout_ms: 1000,
            commands: vec![cmd(vec![0xFF, 0x00], "CONNECT")],
            stop_commands: vec![stop_cmd(vec![0xFE], "DISCONNECT")],
        });
        let t = generate_tscript(&config);
        assert!(
            t.contains("    else {\n        if (this.data[0] == 0x01) {"),
            "got:\n{t}"
        );
        assert!(
            t.contains(
                "        if (got_Rpm == 1 && got_Temp == 1 && val_Rpm > 3000 && val_Temp < 90) {"
            ),
            "got:\n{t}"
        );
        assert!(t.contains("on CanMessage<*> (0x456) {"), "got:\n{t}");
    }

    #[test]
    fn standalone_script_without_a_trigger_still_drops_the_dbc() {
        let mut config = standalone_config(0x456, false);
        config.signals[0].trigger = None;
        config.decoders = vec![];
        let t = generate_tscript(&config);
        assert!(t.contains("on CanMessage<*> (0x456) {"), "got:\n{t}");
        assert!(t.contains("needs no DBC file"), "got:\n{t}");
        assert!(!t.contains("float sig_"), "got:\n{t}");
        assert!(!t.contains("val_"), "got:\n{t}");
    }

    #[test]
    fn standalone_script_hooks_every_can_id_a_daq_list_transmits_on() {
        let (project, daq_lists) = two_can_id_project();

        let terms = parse_terms("A > 1 && B > 2").unwrap();
        let resolved = resolve_terms(&terms, true, &project, &daq_lists, 0x456).unwrap();
        let signals = resolve_signal_hooks(
            true,
            &project,
            &daq_lists,
            Protocol::Xcp,
            Some((&resolved, TriggerAction::StartLogger)),
            None,
        )
        .unwrap();

        let mut config = dbc_config();
        config.signals = signals;
        config.daq_init = Some(
            build_daq_init(Protocol::Xcp, &project, &daq_lists).expect("master/slave ids present"),
        );
        config.decoders = resolved.decoders;
        let t = generate_tscript(&config);

        assert_eq!(
            t.matches("on CanMessage<*>").count(),
            2,
            "one hook per CAN id:\n{t}"
        );
        assert!(t.contains("on CanMessage<*> (0x456) {"), "got:\n{t}");
        assert!(t.contains("on CanMessage<*> (0x789) {"), "got:\n{t}");

        let start_456 = t.find("on CanMessage<*> (0x456) {").unwrap();
        let start_789 = t.find("on CanMessage<*> (0x789) {").unwrap();
        let hook_456 = &t[start_456..start_789];
        let hook_789 = &t[start_789..];
        assert!(hook_456.contains("init_state++;"), "got:\n{hook_456}");
        assert!(!hook_789.contains("init_state++;"), "got:\n{hook_789}");

        assert!(
            hook_456.contains("val_A = sig_A(this.data);"),
            "got:\n{hook_456}"
        );
        assert!(!hook_456.contains("val_B ="), "got:\n{hook_456}");
        assert!(
            hook_789.contains("val_B = sig_B(this.data);"),
            "got:\n{hook_789}"
        );
        assert!(!hook_789.contains("val_A ="), "got:\n{hook_789}");
        assert_eq!(
            t.matches("if (got_A == 1 && got_B == 1 && val_A > 1 && val_B > 2) {")
                .count(),
            2,
            "the full condition is repeated in every hook:\n{t}"
        );

        assert_eq!(t.matches("float val_A = 0;").count(), 1, "got:\n{t}");
        assert_eq!(t.matches("float val_B = 0;").count(), 1, "got:\n{t}");
    }

    fn timed_config(max_captures: Option<u32>, daq_watchdog_ms: Option<u32>) -> TScriptConfig {
        let mut config = init_config(
            Protocol::Xcp,
            vec![cmd(vec![0xFF, 0x00], "CONNECT")],
            vec![stop_cmd(vec![0xFE], "DISCONNECT")],
        );
        config.timer_ms = Some(8000);
        config.max_captures = max_captures;
        config.daq_watchdog_ms = daq_watchdog_ms;
        config.signals[0].trigger = Some(TriggerCondition {
            signals: vec![signal("Rpm", 1, "this.Rpm.Phys")],
            comparisons: vec![comparison("Rpm", ">", "3000")],
            action: TriggerAction::StartLogger,
        });
        config
    }

    #[test]
    fn a_timed_capture_rearms_once_the_condition_has_been_false() {
        let t = generate_tscript(&timed_config(None, None));
        assert!(
            t.contains("    int armed = 1;\n    int captures = 0;"),
            "got:\n{t}"
        );
        assert!(t.contains("if (armed == 1) {"), "got:\n{t}");
        let capture = t.split("if (armed == 1) {").nth(1).unwrap();
        assert!(
            capture.contains("armed = 0;")
                && capture.contains("captures++;")
                && capture.contains("loggerStart();"),
            "got:\n{t}"
        );
        assert!(
            t.contains("} else {\n            armed = 1;\n        }"),
            "got:\n{t}"
        );
        assert!(
            !t.contains("MAX_CAPTURES"),
            "no limit by default, got:\n{t}"
        );

        let log_timer = function_body(&t, "on Timer logTimer {");
        assert!(
            log_timer.contains("loggerStop();") && log_timer.contains("logging = 0;"),
            "got:\n{t}"
        );
        assert!(
            !log_timer.contains("sendDaqStop"),
            "DAQ keeps running between captures, got:\n{t}"
        );
    }

    #[test]
    fn max_captures_stops_daq_after_the_last_one() {
        let t = generate_tscript(&timed_config(Some(3), Some(2000)));
        assert!(t.contains("const int MAX_CAPTURES = 3;"), "got:\n{t}");
        assert!(
            t.contains("if (armed == 1 && captures < MAX_CAPTURES) {"),
            "got:\n{t}"
        );
        let log_timer = function_body(&t, "on Timer logTimer {");
        let last = log_timer
            .split("if (captures >= MAX_CAPTURES) {")
            .nth(1)
            .expect(&t);
        assert!(last.contains("endDaq();"), "got:\n{t}");
        assert!(
            t.contains("after capture 3 the script stops DAQ on the ECU"),
            "header, got:\n{t}"
        );
    }

    #[test]
    fn untimed_logging_has_no_capture_counting() {
        let mut config = timed_config(None, None);
        config.timer_ms = None;
        let t = generate_tscript(&config);
        assert!(!t.contains("armed"), "got:\n{t}");
        assert!(!t.contains("captures"), "got:\n{t}");
        assert!(
            t.contains("loggerStart();\n            logging = 1;"),
            "got:\n{t}"
        );
    }

    #[test]
    fn the_watchdog_restarts_the_init_when_daq_frames_stop() {
        let t = generate_tscript(&timed_config(None, Some(2000)));
        assert!(t.contains("    Timer daqWatchdog;"), "got:\n{t}");
        assert!(t.contains("    daqWatchdog.timeout = 2000;"), "got:\n{t}");
        assert!(
            t.contains("starts over 5000 ms later: 7000-9000 ms after the last DAQ frame."),
            "got:\n{t}"
        );
        assert!(
            t.contains(" {\n    daq_seen = 1;\n"),
            "every hook feeds it, got:\n{t}"
        );
        assert!(
            t.contains("printf(\"XCP init complete\\n\");\n            daq_seen = 0;\n            timerStart(daqWatchdog);"),
            "armed once the init completes, got:\n{t}"
        );
        let watchdog = t.split("on Timer daqWatchdog {").nth(1).expect(&t);
        assert!(watchdog.contains("if (daq_seen == 0) {"), "got:\n{t}");
        assert!(watchdog.contains("restartDaqInit();"), "got:\n{t}");
        let restart = t.split("void restartDaqInit() {").nth(1).unwrap();
        assert!(restart.contains("timerCancel(daqWatchdog);"), "got:\n{t}");

        let without = generate_tscript(&timed_config(None, None));
        assert!(!without.contains("daqWatchdog"), "got:\n{without}");
        assert!(!without.contains("daq_seen"), "got:\n{without}");
    }

    #[test]
    fn end_daq_stops_everything_once() {
        let t = generate_tscript(&timed_config(Some(2), Some(2000)));
        let end = function_body(&t, "void endDaq() {");
        assert!(
            end.contains("if (daq_ended == 1) { return; }\n    daq_ended = 1;"),
            "got:\n{t}"
        );
        for line in [
            "loggerStop();",
            "timerCancel(logTimer);",
            "timerCancel(daqTimeout);",
            "timerCancel(initRestart);",
            "timerCancel(daqWatchdog);",
            "init_state = -1;",
            "sendDaqStop(master_id);",
        ] {
            assert!(end.contains(line), "endDaq lacks {line}:\n{t}");
        }
        assert!(
            function_body(&t, "on stop {").contains("endDaq();"),
            "stopping the script stops DAQ, got:\n{t}"
        );
    }

    #[test]
    fn a_stop_condition_ends_daq_before_the_trigger_is_checked() {
        let mut config = timed_config(None, None);
        config.signals[0].stop = Some(TriggerCondition {
            signals: vec![signal("Temp", 2, "this.Temp.Phys")],
            comparisons: vec![Comparison {
                ident: "Temp".into(),
                op: CompareOp::Ge,
                value: "110".into(),
            }],
            action: TriggerAction::StopLogger,
        });
        let t = generate_tscript(&config);
        assert!(
            t.contains("float val_Temp = 0;"),
            "stop signals are latched, got:\n{t}"
        );
        assert!(t.contains("if (this.data[0] == 0x02) {"), "got:\n{t}");
        let ended = t.find("if (daq_ended == 1) { return; }").expect(&t);
        let stop = t
            .find("if (got_Temp == 1 && val_Temp >= 110) {\n            printf(\"Stop condition met\\n\");\n            endDaq();\n            return;")
            .expect(&t);
        let trigger = t.find("if (got_Rpm == 1 && val_Rpm > 3000) {").expect(&t);
        assert!(ended < stop && stop < trigger, "got:\n{t}");
        assert!(
            t.contains("Stops logging and DAQ for good when the stop condition is true."),
            "got:\n{t}"
        );

        config.signals[0].trigger = None;
        let only_stop = generate_tscript(&config);
        assert!(
            only_stop.contains("endDaq();\n            return;"),
            "got:\n{only_stop}"
        );
        assert!(!only_stop.contains("armed == 1"), "got:\n{only_stop}");
    }

    #[test]
    fn a_stop_frame_ends_daq_on_classic_and_fd_frames() {
        let mut config = timed_config(None, None);
        config.stop_can_id = Some(0x123);
        let classic = generate_tscript(&config);
        assert!(
            classic.contains("on CanMessage<*> (0x123) {\n    printf(\"Stop frame 0x123 received\\n\");\n    endDaq();\n}"),
            "got:\n{classic}"
        );
        assert!(
            !classic.contains("on CanMessageFd<*> (0x123)"),
            "got:\n{classic}"
        );

        config.channel.data_bitrate = Some(2_000_000);
        config.stop_can_id = Some(0x18FF_0001);
        let fd = generate_tscript(&config);
        assert!(
            fd.contains("on CanMessage<*> (0x18FF0001)x {"),
            "got:\n{fd}"
        );
        assert!(
            fd.contains("on CanMessageFd<*> (0x18FF0001)x {"),
            "got:\n{fd}"
        );
        assert!(
            fd.contains("when a frame with CAN id 0x18FF0001 arrives"),
            "got:\n{fd}"
        );
    }
}
