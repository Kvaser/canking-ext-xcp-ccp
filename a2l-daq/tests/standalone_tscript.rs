//! End-to-end: a standalone t-script built from a real A2L must be valid t source.
#![cfg(feature = "tscript")]

use a2l_daq::{
    build_daq_init, decoders_for, generate_tscript, inline_trigger_signals, list_protocols,
    pipeline, CanChannel, CompareOp, Comparison, MessageRef, SignalHook, TScriptConfig,
    TriggerAction, TriggerCondition,
};

const SCC: &str = r"C:\Program Files (x86)\Kvaser\Canlib\Bin\scc.exe";
const SAMPLE_CCP: &str = "data/CCP/SampleECU.a2l";

fn build_script(triggers: &[(&str, &str, &str)]) -> String {
    let bytes = std::fs::read(SAMPLE_CCP).expect("bundled A2L");
    let text = a2l_daq::decode_a2l_bytes(&bytes);
    let signal_names: Vec<String> = triggers.iter().map(|(s, _, _)| s.to_string()).collect();
    let mut project = a2l_daq::parse_a2l(&text, &signal_names).expect("parse");
    let protocol = list_protocols(&text).expect("protocols")[0];

    let assignments: Vec<(String, Vec<String>)> = a2l_daq::list_events(&text)
        .unwrap_or_default()
        .first()
        .map(|e| vec![(e.name.clone(), signal_names.clone())])
        .unwrap_or_default();
    let core =
        pipeline::build(&mut project, &assignments, protocol, "SampleECU.a2l").expect("core build");

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
        channels: vec![CanChannel {
            index: 0,
            bitrate: 500_000,
            data_bitrate: None,
        }],
        pretrigger_ms: 200,
        signals: vec![SignalHook {
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
        daq_init: build_daq_init(protocol, &project, &core.daq_lists, 1000),
        can_fd: false,
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

fn compile(source: &str, stem: &str) -> Result<Option<String>, String> {
    if !std::path::Path::new(SCC).exists() {
        return Ok(None);
    }
    let dir = std::env::temp_dir().join("a2l-daq-standalone-tscript");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{stem}.t"));
    std::fs::write(&path, source).map_err(|e| e.to_string())?;

    let out = std::process::Command::new(SCC)
        .arg(&path)
        .current_dir(&dir)
        .output()
        .map_err(|e| e.to_string())?;
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        return Err(log);
    }
    Ok(Some(log))
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

    match compile(&source, "standaln") {
        Ok(None) => eprintln!("scc.exe not installed; skipped the compile check"),
        Ok(Some(log)) => assert!(
            log.contains("Compilation succeeded"),
            "scc did not report success:\n{log}"
        ),
        Err(log) => panic!("generated script does not compile:\n{log}\n--- source ---\n{source}"),
    }
}

#[test]
fn every_decodable_signal_in_a_real_a2l_produces_compilable_source() {
    let bytes = std::fs::read(SAMPLE_CCP).expect("bundled A2L");
    let text = a2l_daq::decode_a2l_bytes(&bytes);
    let names = a2l_daq::list_signal_names(&text).expect("signal names");
    let mut project = a2l_daq::parse_a2l(&text, &names).expect("parse");
    let protocol = list_protocols(&text).expect("protocols")[0];
    let assignments: Vec<(String, Vec<String>)> = a2l_daq::list_events(&text)
        .unwrap_or_default()
        .first()
        .map(|e| vec![(e.name.clone(), names.clone())])
        .unwrap_or_default();
    let core = pipeline::build(&mut project, &assignments, protocol, "src.a2l").expect("core");

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
    let _ = &names;

    let comparisons: Vec<Comparison> = decoders
        .iter()
        .map(|d| Comparison {
            ident: d.ident.clone(),
            op: CompareOp::Gt,
            value: "0".to_string(),
        })
        .collect();
    let config = TScriptConfig {
        channels: vec![CanChannel {
            index: 0,
            bitrate: 500_000,
            data_bitrate: None,
        }],
        pretrigger_ms: 0,
        signals: vec![SignalHook {
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
        can_fd: false,
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

    match compile(&source, "allsigs") {
        Ok(None) => eprintln!("scc.exe not installed; skipped the compile check"),
        Ok(Some(log)) => assert!(
            log.contains("Compilation succeeded"),
            "scc did not report success:\n{log}"
        ),
        Err(log) => panic!("generated script does not compile:\n{log}"),
    }
}

/// Builds a synthetic CCP A2L text with `count` measurements, all on one raster, so DAQ init
/// codegen can be stress-tested at a scale no committed example file needs to carry.
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
    let mut project = a2l_daq::parse_a2l(&text, &names).expect("parse");
    let protocol = list_protocols(&text).expect("protocols")[0];
    let assignments: Vec<(String, Vec<String>)> = a2l_daq::list_events(&text)
        .unwrap_or_default()
        .first()
        .map(|e| vec![(e.name.clone(), names.clone())])
        .unwrap_or_default();
    let core = pipeline::build(&mut project, &assignments, protocol, "src.a2l").expect("core");

    let init = build_daq_init(protocol, &project, &core.daq_lists, 1000)
        .expect("synthetic file declares CCP transport ids");
    let states = init.commands.len();
    assert!(
        states > 1000,
        "expected the full signal set to need thousands of states, got {states}"
    );

    let can_id = a2l_daq::layout::daq_can_id(&project, protocol).expect("DAQ CAN id");
    let config = TScriptConfig {
        channels: vec![CanChannel {
            index: 0,
            bitrate: 500_000,
            data_bitrate: None,
        }],
        pretrigger_ms: 0,
        signals: vec![SignalHook {
            message: MessageRef::Id {
                can_id,
                extended: can_id > 0x7FF,
            },
            trigger: None,
            handles_responses: true,
        }],
        timer_ms: None,
        daq_init: Some(init),
        can_fd: false,
        traceability: vec![],
        decoders: vec![],
    };
    let source = generate_tscript(&config);

    assert!(source.contains("    switch (state) {"), "got:\n{source}");
    assert!(!source.contains("else if (state =="), "no chain:\n{source}");
    assert_eq!(
        source.matches("        break;\n").count(),
        states,
        "one break per case"
    );

    match compile(&source, "manystat") {
        Ok(None) => eprintln!("scc.exe not installed; skipped the compile check"),
        Ok(Some(log)) => assert!(
            log.contains("Compilation succeeded"),
            "scc did not report success:\n{log}"
        ),
        Err(log) => panic!("{states}-state init does not compile:\n{log}"),
    }
}
