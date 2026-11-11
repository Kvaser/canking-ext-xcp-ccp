//! End-to-end: a standalone t-script built from a real A2L must be valid t source.

use a2l_daq::{
    A2lProject, CanChannel, CompareOp, Comparison, CoreOutput, MessageRef, Protocol, SignalHook,
    TScriptConfig, TriggerAction, TriggerCondition, build_daq_init, decoders_for, generate_tscript,
    inline_trigger_signals, list_protocols, pipeline,
};

const SCC: &str = r"C:\Program Files (x86)\Kvaser\Canlib\Bin\scc.exe";
const SAMPLE_CCP: &str = "data/CCP/SampleECU.a2l";

/// The text of a bundled A2L.
fn read_a2l(path: &str) -> String {
    a2l_daq::decode_a2l_bytes(&std::fs::read(path).expect("bundled A2L"))
}

/// Parse `text` and run the pipeline with the signals of `groups[i]` on the A2L's `i`th XCP event, if it declares one.
fn load(text: &str, groups: &[&[String]]) -> (A2lProject, Protocol, CoreOutput) {
    let mut project = a2l_daq::parse_a2l(text, &groups.concat()).expect("parse");
    let protocol = list_protocols(text).expect("protocols")[0];
    let events = a2l_daq::list_events(text).unwrap_or_default();
    let assignments: Vec<(String, Vec<String>)> = events
        .iter()
        .zip(groups)
        .map(|(event, signals)| (event.name.clone(), signals.to_vec()))
        .collect();
    let core = pipeline::build(&mut project, &assignments, protocol, "src.a2l").expect("core");
    (project, protocol, core)
}

fn build_script(triggers: &[(&str, &str, &str)]) -> String {
    let signal_names: Vec<String> = triggers.iter().map(|(s, _, _)| s.to_string()).collect();
    let (project, protocol, core) = load(&read_a2l(SAMPLE_CCP), &[&signal_names]);

    let can_id = a2l_daq::layout::daq_can_id(&project, protocol).expect("DAQ CAN id");
    let decoders = decoders_for(&project, &core.daq_lists, &signal_names, can_id)
        .unwrap_or_else(|e| panic!("decoders for {signal_names:?}: {e}"));
    let comparisons = triggers
        .iter()
        .map(|(signal, op, value)| Comparison {
            ident: a2l_daq::sanitize_name(signal),
            op: CompareOp::from_symbol(op)
                .unwrap_or_else(|| panic!("'{op}' is not a comparison operator")),
            value: (*value).to_string(),
        })
        .collect();

    let config = TScriptConfig {
        max_captures: Some(3),
        daq_watchdog_ms: Some(2000),
        stop_can_id: Some(0x123),
        channel: CanChannel {
            index: 0,
            bitrate: 500_000,
            data_bitrate: None,
        },
        pretrigger_ms: 200,
        signals: vec![SignalHook {
            stop: Some(TriggerCondition {
                signals: inline_trigger_signals(&decoders),
                comparisons: vec![Comparison {
                    ident: a2l_daq::sanitize_name(triggers[0].0),
                    op: CompareOp::Lt,
                    value: "0".to_string(),
                }],
                action: TriggerAction::StopLogger,
            }),
            message: MessageRef::Id {
                can_id,
                extended: can_id > 0x7FF,
            },
            trigger: Some(TriggerCondition {
                signals: inline_trigger_signals(&decoders),
                comparisons,
                action: TriggerAction::StartLogger,
            }),
            handles_responses: true,
        }],
        timer_ms: Some(8000),
        daq_init: build_daq_init(protocol, &project, &core.daq_lists),
        traceability: a2l_daq::traceability_lines(
            "SampleECU.a2l",
            &project,
            &core.daq_lists,
            protocol,
        ),
        decoders,
    };
    generate_tscript(&config)
}

/// Compile `source` with scc, against `dbc` when given, and assert it succeeds. Skipped when scc isn't installed.
///
/// The `.t`/`.dbc` files stay in the temp dir for inspection.
fn assert_compiles(source: &str, stem: &str, dbc: Option<&str>) {
    if !std::path::Path::new(SCC).exists() {
        eprintln!("scc.exe not installed; skipped the compile check");
        return;
    }
    let dir = std::env::temp_dir().join("a2l-daq-standalone-tscript");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{stem}.t"));
    std::fs::write(&path, source).unwrap();

    let mut scc = std::process::Command::new(SCC);
    if let Some(dbc) = dbc {
        let dbc_path = dir.join(format!("{stem}.dbc"));
        std::fs::write(&dbc_path, dbc).unwrap();
        scc.arg(format!("-dbase={}", dbc_path.display()));
    }
    let out = scc.arg(&path).current_dir(&dir).output().expect("run scc");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success() && log.contains("Compilation succeeded"),
        "{} does not compile:\n{log}",
        path.display()
    );
}

#[test]
fn standalone_script_from_a_real_a2l_compiles_without_a_database() {
    let source = build_script(&[
        ("Raw_Throttle1", ">", "2000"),
        ("v_10", "<", "11.5"),
        ("Pedal1", ">=", "0.5"),
    ]);

    assert!(source.contains("on CanMessage<*> (0x"), "got:\n{source}");
    for signal in ["Raw_Throttle1", "v_10", "Pedal1"] {
        assert!(
            source.contains(&format!("float sig_{signal}(const byte data[]) {{")),
            "no decoder for {signal}:\n{source}"
        );
    }
    assert!(
        source.contains("val_Raw_Throttle1 > 2000 && val_v_10 < 11.5 && val_Pedal1 >= 0.5"),
        "got:\n{source}"
    );
    assert!(source.contains("TFloat32 value;"), "got:\n{source}");
    assert!(source.contains("value = buf;"), "got:\n{source}");
    assert!(!source.contains("-dbase="), "got:\n{source}");
    assert!(
        source.contains("raw * 0.0009155832265152902"),
        "got:\n{source}"
    );
    assert!(
        source.contains("void sendDaqCommand(int state, int id) {"),
        "got:\n{source}"
    );
    assert!(source.contains("    switch (state) {"), "got:\n{source}");

    assert_compiles(&source, "standaln", None);
}

#[test]
fn every_decodable_signal_in_a_real_a2l_produces_compilable_source() {
    let text = read_a2l(SAMPLE_CCP);
    let names = a2l_daq::list_signal_names(&text).expect("signal names");
    let (project, protocol, core) = load(&text, &[&names]);

    let placed: Vec<String> = a2l_daq::layout::daq_layout(&project, &core.daq_lists)
        .into_iter()
        .filter_map(|f| match f {
            a2l_daq::layout::Field::Signal(p) => Some(p.name),
            a2l_daq::layout::Field::Timestamp(_) => None,
        })
        .collect();
    let can_id = a2l_daq::layout::daq_can_id(&project, protocol).expect("DAQ CAN id");
    let decoders = decoders_for(&project, &core.daq_lists, &placed, can_id).expect("decoders");
    assert!(
        decoders.len() >= 10,
        "expected the whole sample's mix of decodable signals, got {}",
        decoders.len()
    );
    assert!(
        decoders.iter().any(a2l_daq::SignalDecoder::is_float),
        "sample should include FLOAT32_IEEE signals"
    );
    assert!(
        decoders
            .iter()
            .any(|d| matches!(d.dtype, a2l_daq::DecoderType::Int { signed: true, .. })),
        "sample should include signed signals"
    );
    assert!(
        decoders.iter().any(|d| d.bit_length == 32
            && matches!(d.dtype, a2l_daq::DecoderType::Int { signed: false, .. })),
        "sample should include a 32-bit unsigned signal"
    );
    assert!(
        decoders.iter().any(|d| d.factor != 1.0),
        "sample should include scaled signals"
    );

    let comparisons: Vec<Comparison> = decoders
        .iter()
        .map(|d| Comparison {
            ident: d.ident.clone(),
            op: CompareOp::Gt,
            value: "0".to_string(),
        })
        .collect();
    let config = TScriptConfig {
        stop_can_id: None,
        max_captures: None,
        daq_watchdog_ms: None,
        channel: CanChannel {
            index: 0,
            bitrate: 500_000,
            data_bitrate: None,
        },
        pretrigger_ms: 0,
        signals: vec![SignalHook {
            stop: None,
            message: MessageRef::Id {
                can_id,
                extended: can_id > 0x7FF,
            },
            trigger: Some(TriggerCondition {
                signals: inline_trigger_signals(&decoders),
                comparisons,
                action: TriggerAction::StartLogger,
            }),
            handles_responses: true,
        }],
        timer_ms: None,
        daq_init: None,
        traceability: vec![],
        decoders,
    };
    let source = generate_tscript(&config);

    let exponent = |line: &str| {
        line.as_bytes()
            .windows(2)
            .any(|w| w[0].is_ascii_digit() && (w[1] == b'e' || w[1] == b'E'))
    };
    for line in source
        .lines()
        .filter(|l| l.trim_start().starts_with("return "))
    {
        assert!(!exponent(line), "exponent notation in a literal: {line}");
    }
    assert!(
        exponent("return raw * 1.5e-7;"),
        "the exponent check must actually catch one"
    );

    assert_compiles(&source, "allsigs", None);
}

/// Builds a synthetic CCP A2L text with `count` measurements, all on one raster, so DAQ init codegen can be stress-tested
fn synthetic_ccp_a2l(count: usize) -> String {
    let mut text = String::from(
        r#"
/begin MOD_COMMON ""
  BYTE_ORDER MSB_FIRST
/end MOD_COMMON
/begin IF_DATA ASAP1B_CCP
  /begin RASTER
    "Bulk" "Bulk" 0x1 0x1 0x1
  /end RASTER
  /begin TP_BLOB
    0x0201 0x0206 0x00000300 0x00000301 0x0001 0x0001
    BAUDRATE 500000
    DAQ_MODE BURST
    BYTES_ONLY
  /end TP_BLOB
/end IF_DATA
"#,
    );
    for i in 0..count {
        let addr = 0x2000_0000u32 + (i as u32) * 2;
        text.push_str(&format!(
            "/begin MEASUREMENT Sig_{i} \"\" UWORD Sig_{i}_CM 0 0 0 65535 ECU_ADDRESS 0x{addr:08X} /end MEASUREMENT\n\
             /begin COMPU_METHOD Sig_{i}_CM \"\" RAT_FUNC \"%5.0\" \"\" COEFFS 0 1 0 0 0 1 /end COMPU_METHOD\n"
        ));
    }
    text
}

#[test]
fn daq_init_compiles_with_thousands_of_states() {
    let text = synthetic_ccp_a2l(1200);
    let names = a2l_daq::list_signal_names(&text).expect("signal names");
    let (project, protocol, core) = load(&text, &[&names]);

    let init = build_daq_init(protocol, &project, &core.daq_lists)
        .expect("synthetic file declares CCP transport ids");
    let states = init.commands.len();
    assert!(
        states > 1000,
        "expected the full signal set to need thousands of states, got {states}"
    );

    let can_id = a2l_daq::layout::daq_can_id(&project, protocol).expect("DAQ CAN id");
    let config = TScriptConfig {
        stop_can_id: None,
        max_captures: None,
        daq_watchdog_ms: None,
        channel: CanChannel {
            index: 0,
            bitrate: 500_000,
            data_bitrate: None,
        },
        pretrigger_ms: 0,
        signals: vec![SignalHook {
            stop: None,
            message: MessageRef::Id {
                can_id,
                extended: can_id > 0x7FF,
            },
            trigger: None,
            handles_responses: true,
        }],
        timer_ms: None,
        daq_init: Some(init),
        traceability: vec![],
        decoders: vec![],
    };
    let source = generate_tscript(&config);

    assert!(source.contains("    switch (state) {"), "got:\n{source}");
    assert_eq!(
        source.matches("        break;\n").count(),
        states,
        "one break per case"
    );

    assert_compiles(&source, "manystat", None);
}

#[test]
fn dbc_mode_script_with_two_daq_messages_compiles_against_its_dbc() {
    let signals = ["EngineSpeed_rpm", "VehicleSpeed_kmh"].map(String::from);
    // One signal per event: two DAQ lists.
    let (project, protocol, mut core) = load(
        &read_a2l("data/XCP/SampleECU.a2l"),
        &[&signals[..1], &signals[1..]],
    );
    assert_eq!(core.daq_lists.len(), 2);
    // A second DAQ message: the DBC then names both `DAQ_<id>` instead of `DAQ`.
    core.daq_lists[1].can_id = a2l_daq::CanIdSource::Fixed { id: 0x7E0 };
    let dbc = a2l_daq::create_dbc_file(&project, &core.daq_lists, protocol, "SampleECU.a2l")
        .expect("DBC");

    let default_id = a2l_daq::layout::daq_can_id(&project, protocol).expect("DAQ CAN id");
    let trigger = a2l_daq::build_trigger(
        "EngineSpeed_rpm.Phys > 2000 && VehicleSpeed_kmh.Phys >= 0.5",
        false,
        &project,
        &core.daq_lists,
        default_id,
    )
    .expect("trigger");
    let signals = a2l_daq::resolve_signal_hooks(
        false,
        &project,
        &core.daq_lists,
        protocol,
        Some((&trigger, TriggerAction::StartLogger)),
        None,
    )
    .expect("DBC hooks");
    let bus = a2l_daq::bus_defaults(&project, protocol);
    let config = TScriptConfig {
        max_captures: Some(2),
        daq_watchdog_ms: Some(2000),
        stop_can_id: None,
        channel: CanChannel {
            index: 0,
            bitrate: bus.bitrate,
            data_bitrate: bus.can_fd_data_bitrate,
        },
        pretrigger_ms: 200,
        signals,
        timer_ms: Some(8000),
        daq_init: build_daq_init(protocol, &project, &core.daq_lists),
        traceability: Vec::new(),
        decoders: Vec::new(),
    };
    let source = generate_tscript(&config);
    assert!(
        !source.contains("<*> DAQ {"),
        "hooks a message the DBC lacks:
{source}"
    );

    assert_compiles(&source, "dbc2msg", Some(&dbc));
}
