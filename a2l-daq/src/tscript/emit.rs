//! Rendering a [`TScriptConfig`] into `.t` source.

use super::inline_signals;
use super::trigger::{TriggerSignal, condition_expression, latch_block, latch_variables};
use super::{DaqInit, MessageRef, SignalHook, TScriptConfig};
use protocol_core::protocol::BytePattern;
use std::fmt::Write as FmtWrite;

const DAQ_INIT_MAX_RETRIES: u32 = 10;

/// Per-channel identifier suffix
fn channel_suffix(i: usize) -> String {
    if i == 0 { String::new() } else { i.to_string() }
}

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
    out.push_str(&on_start_block(config));
    out.push_str(&signal_hook_block(config));
    if config.timer_ms.is_some() {
        out.push_str(&log_timer_block(config));
    }
    if let Some(init) = &config.daq_init {
        out.push_str(&daq_timeout_block(init));
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

/// The `variables{}` block: variables used in the t-script
fn variables_block(config: &TScriptConfig) -> String {
    let mut out = String::new();
    out.push_str("variables{\n");
    for (i, ch) in config.channels.iter().enumerate() {
        let suffix = channel_suffix(i);
        let _ = writeln!(out, "    const int can_channel{} = {};", suffix, ch.index);
        match ch.data_bitrate {
            Some(data_bitrate) => {
                let _ = writeln!(
                    out,
                    "    const int bitrate{} = {};",
                    suffix,
                    fd_bitrate_token(ch.bitrate)
                );
                let _ = writeln!(
                    out,
                    "    const int data_bitrate{} = {};",
                    suffix,
                    fd_bitrate_token(data_bitrate)
                );
            }
            None => {
                let _ = writeln!(out, "    const int bitrate{} = {};", suffix, ch.bitrate);
            }
        }
    }
    if let Some(init) = &config.daq_init {
        let _ = writeln!(out, "    const int master_id = 0x{:X};", init.master_id);
        let _ = writeln!(out, "    const int slave_id = 0x{:X};", init.slave_id);
        let _ = writeln!(out, "    const int INIT_STATES = {};", init.commands.len());
    }
    let _ = writeln!(out, "    int logging = 0;");
    if config.daq_init.is_some() {
        let _ = writeln!(out, "    int count = 0;");
        let _ = writeln!(out, "    int init_state = 0;");
        let _ = writeln!(out, "    Timer daqTimeout;");
    }
    if config.timer_ms.is_some() {
        let _ = writeln!(out, "    Timer logTimer;");
    }
    out.push_str(&inline_signals::helper_typedef(&config.decoders));
    out.push_str(&latch_variables(&all_trigger_signals(config)));
    out.push_str("}\n\n");
    out
}

/// Every distinct trigger signal across all of a config's hooks.
fn all_trigger_signals(config: &TScriptConfig) -> Vec<TriggerSignal> {
    let mut signals: Vec<TriggerSignal> = Vec::new();
    for hook in &config.signals {
        if let Some(trigger) = &hook.trigger {
            for s in &trigger.signals {
                if !signals.iter().any(|existing| existing.ident == s.ident) {
                    signals.push(s.clone());
                }
            }
        }
    }
    signals
}

/// The `send`/`sendDaqCommand`/`sendDaqStop` functions that drive the DAQ-init state machine.
fn daq_functions_block(config: &TScriptConfig, init: &DaqInit) -> String {
    let mut out = String::new();
    out.push_str("void send(int id, const byte data[]) {\n");

    if config.can_fd {
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
            "    case {}:  /* [{}] {} */",
            i,
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
                "    byte stopData{}[{}] = {{{}}};  /* [{}] {} */",
                i,
                cmd.bytes.len(),
                byte_literals(&cmd.bytes),
                cmd.phase.tag(),
                cmd.label
            );
            let _ = writeln!(out, "    send(id, stopData{});", i);
        }
        out.push_str("}\n\n");
    }
    out
}

/// The `on start` hook. Initialize the CAN bus, Timers
fn on_start_block(config: &TScriptConfig) -> String {
    let mut out = String::new();
    out.push_str("on start {\n");
    out.push_str("    printf(\"Starting script...\\n\");\n\n");
    for (i, ch) in config.channels.iter().enumerate() {
        let suffix = channel_suffix(i);
        let _ = writeln!(out, "    canBusOff(can_channel{});", suffix);
        if ch.data_bitrate.is_some() {
            // Specific bus params more important for CAN FD
            let _ = writeln!(
                out,
                "    canSetBusParams(can_channel{0}, bitrate{0}, 0, 0, 0, 0);",
                suffix
            );
            let _ = writeln!(
                out,
                "    canSetBusParamsFd(can_channel{0}, data_bitrate{0}, 0, 0, 0);",
                suffix
            );
            let _ = writeln!(
                out,
                "    canSetCommunicationMode(can_channel{}, canMODE_CAN_FD);",
                suffix
            );
        } else {
            let _ = writeln!(
                out,
                "    canSetBitrate(can_channel{0}, bitrate{0});",
                suffix
            );
        }
        let _ = writeln!(
            out,
            "    canSetBusOutputControl(can_channel{}, canDRIVER_NORMAL);",
            suffix
        );
        let _ = writeln!(out, "    canBusOn(can_channel{});", suffix);
    }
    out.push('\n');
    let _ = writeln!(out, "    loggerSetPretrigger({});", config.pretrigger_ms);
    if let Some(ms) = config.timer_ms {
        let _ = writeln!(out, "    logTimer.timeout = {};", ms);
    }

    if let Some(init) = &config.daq_init {
        let _ = writeln!(out, "\n    daqTimeout.timeout = {};", init.timeout_ms);
        out.push_str("    sendDaqCommand(init_state, master_id);\n");
        out.push_str("    timerStart(daqTimeout);\n");
    }
    out.push_str("}\n\n");
    out
}

/// The `on CanMessage(Fd)<*>` hook(s)
fn signal_hook_block(config: &TScriptConfig) -> String {
    let mut out = String::from("/*\n    Signal Hook\n*/\n\n");
    for hook in &config.signals {
        out.push_str(&one_signal_hook_block(config, hook));
    }
    out
}

fn advance_state_lines(indent: &str, protocol_name: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{indent}timerCancel(daqTimeout);");
    let _ = writeln!(out, "{indent}init_state++;");
    let _ = writeln!(out, "{indent}count = 0;");
    let _ = writeln!(out, "{indent}if (init_state < INIT_STATES) {{");
    let _ = writeln!(out, "{indent}    sendDaqCommand(init_state, master_id);");
    let _ = writeln!(out, "{indent}    timerStart(daqTimeout);");
    let _ = writeln!(out, "{indent}}} else {{");
    let _ = writeln!(
        out,
        "{indent}    printf(\"{protocol_name} init complete\\n\");"
    );
    let _ = writeln!(out, "{indent}}}");
    out
}

fn fatal_error_lines(
    indent: &str,
    protocol_name: &str,
    error_code_byte: usize,
    has_stop: bool,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{indent}printf(\"{protocol_name} error at state %d: 0x%02x\\n\", init_state, this.data[{error_code_byte}]);"
    );
    let _ = writeln!(out, "{indent}timerCancel(daqTimeout);");
    if has_stop {
        let _ = writeln!(out, "{indent}sendDaqStop(master_id);");
    }
    out
}

/// One `on CanMessage(Fd)<*>` block
fn one_signal_hook_block(config: &TScriptConfig, hook: &SignalHook) -> String {
    let mut out = String::new();
    let has_init = config.daq_init.is_some() && hook.handles_responses;
    let msg_kind = if config.can_fd {
        "CanMessageFd"
    } else {
        "CanMessage"
    };
    let _ = writeln!(
        out,
        "on {}<*> {} {{",
        msg_kind,
        hook.message.hook_selector()
    );

    // Slave response handler.
    if has_init {
        let init = config.daq_init.as_ref().expect("has_init implies daq_init");
        let format = init.protocol.response_format();
        let name = init.protocol.name();
        let has_optional = init.commands.iter().any(|c| c.optional);
        let has_stop = !init.stop_commands.is_empty();

        let _ = writeln!(out, "    if ({}) {{", frame_condition(format.positive));
        out.push_str(&advance_state_lines("        ", name));
        let _ = writeln!(out, "    }} else if ({}) {{", frame_condition(format.error));
        if has_optional {
            out.push_str("        if (isOptionalState(init_state)) {\n");
            let _ = writeln!(
                out,
                "            printf(\"{name} optional command not supported at state %d (0x%02x), continuing\\n\", init_state, this.data[{}]);",
                format.error_code_byte
            );
            out.push_str(&advance_state_lines("            ", name));
            out.push_str("        } else {\n");
            out.push_str(&fatal_error_lines(
                "            ",
                name,
                format.error_code_byte,
                has_stop,
            ));
            out.push_str("        }\n");
        } else {
            out.push_str(&fatal_error_lines(
                "        ",
                name,
                format.error_code_byte,
                has_stop,
            ));
        }
        out.push_str("    }\n");
    }

    // DAQ message trigger condition.
    if let Some(trigger) = &hook.trigger {
        let indent = if has_init { "        " } else { "    " };
        if has_init {
            out.push_str("    else {\n");
        }

        out.push_str(&latch_block(&trigger.signals, indent));
        let _ = writeln!(out, "{indent}if (logging == 1) {{ return; }}");
        let _ = writeln!(out, "{indent}if ({}) {{", condition_expression(trigger));
        for statement in trigger.action.statements() {
            let _ = writeln!(out, "{indent}    {statement}");
        }
        let _ = writeln!(out, "{indent}    logging = 1;");
        if config.timer_ms.is_some() {
            let _ = writeln!(out, "{indent}    timerStart(logTimer);");
        }
        let _ = writeln!(out, "{indent}}}");
        if has_init {
            out.push_str("    }\n");
        }
    }
    out.push_str("}\n\n");
    out
}

/// The `on Timer logTimer` hook: stop the logger once the post-trigger timer expires.
/// Only emitted when `config.timer_ms` is `Some`.
fn log_timer_block(config: &TScriptConfig) -> String {
    let mut out = String::new();
    out.push_str("on Timer logTimer {\n");
    out.push_str("    printf(\"Log timer expired\\n\");\n");
    out.push_str("    if (loggerStatus() == LOGGER_STATE_LOGGING) {\n");
    out.push_str("        printf(\"Stopping logger...\\n\");\n");
    if config
        .daq_init
        .as_ref()
        .is_some_and(|init| !init.stop_commands.is_empty())
    {
        out.push_str("        sendDaqStop(master_id);\n");
    }
    out.push_str("        loggerStop();\n");
    out.push_str("        logging = 0;\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
    out
}

/// The `on Timer daqTimeout` hook: no response in time -> resend the current DAQ-init
/// command, up to [`DAQ_INIT_MAX_RETRIES`] times, then give up.
fn daq_timeout_block(init: &DaqInit) -> String {
    let mut out = String::new();
    out.push_str("on Timer daqTimeout {\n");
    let _ = writeln!(out, "    if (count < {DAQ_INIT_MAX_RETRIES}) {{");
    out.push_str("        count++;\n");
    out.push_str("    } else {\n");
    let _ = writeln!(
        out,
        "        printf(\"DAQ timeout at state %d, giving up after {DAQ_INIT_MAX_RETRIES} attempts\\n\", init_state);"
    );
    if !init.stop_commands.is_empty() {
        out.push_str("        sendDaqStop(master_id);\n");
    }
    out.push_str("        return;\n");
    out.push_str("    }\n");
    out.push_str("    printf(\"DAQ timeout at state %d, retrying...\\n\", init_state);\n");
    out.push_str("    sendDaqCommand(init_state, master_id);\n");
    out.push_str("    timerStart(daqTimeout);\n");
    out.push_str("}\n\n");
    out
}

/// The `on stop`/`on exception` hooks: fixed regardless of config.
fn stop_and_exception_block() -> String {
    let mut out = String::new();
    out.push_str("on stop {\n");
    out.push_str("    printf(\"Stopping script...\\n\");\n");
    out.push_str("    loggerStop();\n");
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
        .map(|b| format!("0x{:02X}", b))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a [`BytePattern`] as a t-script condition over the received frame.
fn frame_condition(pattern: BytePattern) -> String {
    if pattern.is_empty() {
        return String::from("1");
    }
    pattern
        .iter()
        .map(|(index, value)| format!("this.data[{}] == 0x{:02X}", index, value))
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

    #[test]
    fn generates_basic_tscript() {
        let config = TScriptConfig {
            channels: vec![
                CanChannel {
                    index: 0,
                    bitrate: 1_000_000,
                    data_bitrate: None,
                },
                CanChannel {
                    index: 1,
                    bitrate: 500_000,
                    data_bitrate: None,
                },
            ],
            pretrigger_ms: 200,
            signals: vec![SignalHook {
                message: MessageRef::Dbc("PowertrainStatus".into()),
                trigger: Some(TriggerCondition {
                    signals: vec![signal("EngineSpeed", 0, "this.EngineSpeed.Phys")],
                    comparisons: vec![comparison("EngineSpeed", ">", "4000")],
                    action: TriggerAction::StartLogger,
                }),
                handles_responses: true,
            }],
            timer_ms: Some(8000),
            daq_init: None,
            can_fd: false,
            traceability: vec![],
            decoders: vec![],
        };
        let tscript = generate_tscript(&config);
        println!("{}", tscript);
        assert!(tscript.contains("const int can_channel = 0;"));
        assert!(tscript.contains("const int bitrate = 1000000;"));
        assert!(tscript.contains("const int can_channel1 = 1;"));
        assert!(tscript.contains("const int bitrate1 = 500000;"));
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
        let config = TScriptConfig {
            channels: vec![CanChannel {
                index: 0,
                bitrate: 500_000,
                data_bitrate: None,
            }],
            pretrigger_ms: 0,
            signals: vec![SignalHook {
                message: MessageRef::Dbc("DAQ".into()),
                trigger: None,
                handles_responses: true,
            }],
            timer_ms: None,
            daq_init: None,
            can_fd: false,
            traceability: vec!["Source A2L: MyEcu.a2l".into(), "Protocol: XCP".into()],
            decoders: vec![],
        };
        let t = generate_tscript(&config);
        assert!(t.contains("--- Generated from ---"), "got:\n{t}");
        assert!(t.contains("Source A2L: MyEcu.a2l"), "got:\n{t}");
        let header_end = t.find("*/").expect("header comment close");
        assert!(t.find("Source A2L").unwrap() < header_end, "got:\n{t}");
    }

    #[test]
    fn generates_can_fd_bus_config_and_frame_flags() {
        let config = TScriptConfig {
            channels: vec![CanChannel {
                index: 0,
                bitrate: 1_000_000,
                data_bitrate: Some(2_000_000),
            }],
            pretrigger_ms: 200,
            signals: vec![SignalHook {
                message: MessageRef::Dbc("Test".into()),
                trigger: None,
                handles_responses: true,
            }],
            timer_ms: None,
            daq_init: Some(DaqInit {
                protocol: Protocol::Xcp,
                master_id: 0x455,
                slave_id: 0x456,
                timeout_ms: 1000,
                commands: vec![cmd(vec![0xFF, 0x00], "CONNECT")],
                stop_commands: vec![stop_cmd(vec![0xFE], "DISCONNECT")],
            }),
            can_fd: true,
            traceability: vec![],
            decoders: vec![],
        };
        let t = generate_tscript(&config);
        println!("{}", t);
        assert!(t.contains("const int bitrate = canFD_BITRATE_1M_80P;"));
        assert!(t.contains("const int data_bitrate = canFD_BITRATE_2M_80P;"));
        assert!(t.contains("canSetBusParams(can_channel, bitrate, 0, 0, 0, 0);"));
        assert!(t.contains("canSetBusParamsFd(can_channel, data_bitrate, 0, 0, 0);"));
        assert!(t.contains("canSetCommunicationMode(can_channel, canMODE_CAN_FD);"));
        assert!(t.contains("CanMessageFd msg;"));
        assert!(t.contains("msg.flags = canFDMSG_FDF | canFDMSG_BRS;"));
    }

    #[test]
    fn multi_signal_trigger_expression_emitted_correctly() {
        let config = TScriptConfig {
            channels: vec![CanChannel {
                index: 0,
                bitrate: 500_000,
                data_bitrate: None,
            }],
            pretrigger_ms: 0,
            signals: vec![SignalHook {
                message: MessageRef::Dbc("DAQ".into()),
                trigger: Some(TriggerCondition {
                    signals: vec![
                        signal("RPM", 0, "this.RPM.Phys"),
                        signal("PRESSURE_raw", 1, "this.PRESSURE.Raw"),
                    ],
                    comparisons: vec![
                        comparison("RPM", ">", "3000"),
                        comparison("PRESSURE_raw", "<", "50"),
                    ],
                    action: TriggerAction::StartLogger,
                }),
                handles_responses: true,
            }],
            timer_ms: None,
            daq_init: None,
            can_fd: false,
            traceability: vec![],
            decoders: vec![],
        };
        let t = generate_tscript(&config);
        assert!(
            t.contains(
                "if (got_RPM == 1 && got_PRESSURE_raw == 1 \
                 && val_RPM > 3000 && val_PRESSURE_raw < 50) {"
            ),
            "got:\n{t}"
        );
    }

    #[test]
    fn unknown_fd_bitrate_falls_back_to_raw_frequency() {
        assert_eq!(fd_bitrate_token(2_000_000), "canFD_BITRATE_2M_80P");
        assert_eq!(fd_bitrate_token(1_500_000), "1500000");
    }

    #[test]
    fn no_xcp_init_omits_response_handler() {
        let config = TScriptConfig {
            channels: vec![CanChannel {
                index: 0,
                bitrate: 500_000,
                data_bitrate: None,
            }],
            pretrigger_ms: 200,
            signals: vec![SignalHook {
                message: MessageRef::Dbc("DAQ".into()),
                trigger: Some(TriggerCondition {
                    signals: vec![signal("Speed", 0, "this.Speed.Phys")],
                    comparisons: vec![comparison("Speed", ">", "10")],
                    action: TriggerAction::StartLogger,
                }),
                handles_responses: true,
            }],
            timer_ms: None,
            daq_init: None,
            can_fd: false,
            traceability: vec![],
            decoders: vec![],
        };
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
        let config = TScriptConfig {
            channels: vec![CanChannel {
                index: 0,
                bitrate: 500_000,
                data_bitrate: None,
            }],
            pretrigger_ms: 200,
            signals: vec![SignalHook {
                message: MessageRef::Dbc("Test".into()),
                trigger: None,
                handles_responses: true,
            }],
            timer_ms: None,
            daq_init: None,
            can_fd: false,
            traceability: vec![],
            decoders: vec![],
        };
        let t = generate_tscript(&config);
        assert!(!t.contains("canSetBitrateFd"));
        assert!(!t.contains("canMODE_CAN_FD"));
        assert!(!t.contains("data_bitrate"));
    }

    #[test]
    fn generates_xcp_init_state_machine() {
        let config = TScriptConfig {
            channels: vec![CanChannel {
                index: 0,
                bitrate: 500_000,
                data_bitrate: None,
            }],
            pretrigger_ms: 200,
            signals: vec![SignalHook {
                message: MessageRef::Dbc("Test".into()),
                trigger: None,
                handles_responses: true,
            }],
            timer_ms: None,
            daq_init: Some(DaqInit {
                protocol: Protocol::Xcp,
                master_id: 0x51,
                slave_id: 0x52,
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
            }),
            can_fd: false,
            traceability: vec![],
            decoders: vec![],
        };
        let t = generate_tscript(&config);
        println!("{}", t);
        assert!(t.contains("const int master_id = 0x51;"));
        assert!(t.contains("const int slave_id = 0x52;"));
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

        let xcp = generate_tscript(&init_config(
            Protocol::Xcp,
            vec![cmd(vec![0xFF, 0x00], "CONNECT")],
            vec![stop_cmd(vec![0xFE], "DISCONNECT")],
        ));
        assert!(xcp.contains("if (this.data[0] == 0xFF) {"), "got:\n{xcp}");
        assert!(
            xcp.contains("} else if (this.data[0] == 0xFE) {"),
            "got:\n{xcp}"
        );
    }

    #[test]
    fn every_abort_path_replays_the_protocol_stop_commands() {
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
            assert_eq!(
                t.matches("sendDaqStop(master_id);").count(),
                3,
                "{protocol:?}:\n{t}"
            );
        }
    }

    #[test]
    fn abort_calls_omitted_when_there_are_no_stop_commands() {
        let t = generate_tscript(&init_config(
            Protocol::Xcp,
            vec![cmd(vec![0xFF, 0x00], "CONNECT")],
            vec![],
        ));
        assert!(!t.contains("sendDaqStop"), "got:\n{t}");
        assert!(
            t.contains("} else if (this.data[0] == 0xFE) {"),
            "got:\n{t}"
        );
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
        assert!(!optional_branch.contains("sendDaqStop"), "got:\n{t}");
        assert_eq!(t.matches("sendDaqStop(master_id);").count(), 3, "got:\n{t}");
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
        let config = TScriptConfig {
            channels: vec![CanChannel {
                index: 0,
                bitrate: 500_000,
                data_bitrate: None,
            }],
            pretrigger_ms: 0,
            signals: vec![SignalHook {
                message: MessageRef::Dbc("DAQ".into()),
                trigger: Some(TriggerCondition {
                    signals: dbc_trigger_signals(&project, &daq_lists, &reads).unwrap(),
                    comparisons: vec![
                        comparison("Rpm", ">", "3000"),
                        comparison("Temp", "<", "90"),
                    ],
                    action: TriggerAction::StartLogger,
                }),
                handles_responses: true,
            }],
            timer_ms: None,
            daq_init: None,
            can_fd: false,
            traceability: vec![],
            decoders: vec![],
        };
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
    fn dbc_script_still_documents_the_dbase_argument() {
        let config = TScriptConfig {
            channels: vec![CanChannel {
                index: 0,
                bitrate: 500_000,
                data_bitrate: None,
            }],
            pretrigger_ms: 0,
            signals: vec![SignalHook {
                message: MessageRef::Dbc("DAQ".into()),
                trigger: None,
                handles_responses: true,
            }],
            timer_ms: None,
            daq_init: None,
            can_fd: false,
            traceability: vec![],
            decoders: vec![],
        };
        let t = generate_tscript(&config);
        assert!(t.contains("scc.exe -dbase="), "got:\n{t}");
        assert!(t.contains("on CanMessage<*> DAQ {"), "got:\n{t}");
        assert!(!t.contains("float sig_"), "no decoders to emit:\n{t}");
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
    fn standalone_decoders_read_the_bytes_the_dbc_would_describe() {
        let t = generate_tscript(&standalone_config(0x456, false));
        assert!(t.contains("raw = data[1] | (data[2] << 8);"), "got:\n{t}");
        assert!(
            t.contains("return raw * 0.1 - 40.0;"),
            "scaled signal:\n{t}"
        );
        assert!(
            t.contains("if (raw >= 32768) {\n        raw -= 65536;\n    }"),
            "signed signal:\n{t}"
        );
    }

    #[test]
    fn standalone_script_coexists_with_the_daq_init_state_machine() {
        let mut config = standalone_config(0x456, false);
        config.daq_init = Some(DaqInit {
            protocol: Protocol::Xcp,
            master_id: 0x455,
            slave_id: 0x456,
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
        use crate::a2l::{CanIdSource, DaqInfo, EventInfo, ParsedSignal, XcpData, XcpInfo};
        use crate::daq::{Odt, OdtEntry};

        let project = A2lProject {
            signals: vec![
                ParsedSignal {
                    name: "A".into(),
                    size: 2,
                    ecu_address: 0x10,
                    ..Default::default()
                },
                ParsedSignal {
                    name: "B".into(),
                    size: 2,
                    ecu_address: 0x2000,
                    ..Default::default()
                },
            ],
            xcp_data: Some(XcpData {
                daq_info: DaqInfo::default(),
                event_info: EventInfo::default(),
                xcp_info: Some(XcpInfo {
                    can_id_master: Some(0x455),
                    can_id_slave: Some(0x456),
                    ..Default::default()
                }),
                protocol_layer: None,
            }),
            ..Default::default()
        };
        let daq_lists = vec![
            DaqList {
                id: 0,
                first_pid: 1,
                event: None,
                odts: vec![Odt {
                    entries: vec![OdtEntry {
                        names: vec!["A".into()],
                        ecu_address: 0x10,
                        size: 2,
                    }],
                }],
                can_id: CanIdSource::Default,
            },
            DaqList {
                id: 1,
                first_pid: 2,
                event: None,
                odts: vec![Odt {
                    entries: vec![OdtEntry {
                        names: vec!["B".into()],
                        ecu_address: 0x2000,
                        size: 2,
                    }],
                }],
                can_id: CanIdSource::Fixed { id: 0x789 },
            },
        ];

        let terms = parse_terms("A > 1 && B > 2").unwrap();
        let resolved = resolve_terms(&terms, true, &project, &daq_lists, 0x456).unwrap();
        let signals = resolve_signal_hooks(
            true,
            &project,
            &daq_lists,
            Protocol::Xcp,
            Some((&resolved, TriggerAction::StartLogger)),
        )
        .unwrap();

        let config = TScriptConfig {
            channels: vec![CanChannel {
                index: 0,
                bitrate: 500_000,
                data_bitrate: None,
            }],
            pretrigger_ms: 0,
            signals,
            timer_ms: None,
            daq_init: Some(
                build_daq_init(Protocol::Xcp, &project, &daq_lists, 1000)
                    .expect("master/slave ids present"),
            ),
            can_fd: false,
            traceability: vec![],
            decoders: resolved.decoders,
        };
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
}
