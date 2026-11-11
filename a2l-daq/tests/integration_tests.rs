//! End-to-end tests
use a2l_daq::{Protocol, build_daq_list, build_init_commands, create_dbc_file, parse_a2l};

const STATIC_FD_A2L: &str = r#"
/begin MEASUREMENT EngTemp
  "Engine temperature"
  FLOAT32_IEEE
  EngTemp_CM
  0 0 0 250
  ECU_ADDRESS 0x20001000
  FORMAT "%5.2"
/end MEASUREMENT

/begin IF_DATA XCP
  /begin XCP_ON_CAN
    0x0105
    CAN_ID_MASTER 0x0455
    CAN_ID_SLAVE  0x0456
    BAUDRATE 1000000
    /begin CAN_FD
      MAX_DLC 24
      CAN_FD_DATA_TRANSFER_BAUDRATE 2000000
    /end CAN_FD
  /end XCP_ON_CAN
  /begin DAQ
    STATIC
    GRANULARITY_ODT_ENTRY_SIZE_DAQ_BYTE 0x04
    /begin TIMESTAMP_SUPPORTED
      0x0001 SIZE_DWORD UNIT_1MS TIMESTAMP_FIXED
    /end TIMESTAMP_SUPPORTED
    /begin DAQ_LIST
      0x0000
      MAX_ODT 0x10
      MAX_ODT_ENTRIES 0x17
      EVENT_FIXED 0x0000
    /end DAQ_LIST
    /begin EVENT
      "One_Event_Per_Rotation" "Rev" 0x0000 DAQ 0x01 0x00 0x06 0xFF
    /end EVENT
  /end DAQ
/end IF_DATA
"#;

#[test]
fn real_a2l_resolves_tab_verb_text_tables() {
    let content = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/data/XCP/SampleECU.a2l"
    ))
    .expect("example A2L present in repo");

    let selected = vec!["EngineState".to_string(), "DiagErrorCode".to_string()];
    let mut project = parse_a2l(&content, &selected).unwrap();
    assert_eq!(project.signals.len(), 2, "both selected signals parsed");

    let eng_st = &project.compu_methods["EngineState_CM"];
    assert_eq!(eng_st.text_table.len(), 5);
    assert_eq!(eng_st.text_table[0].text, "Off");
    assert_eq!(eng_st.text_table[4].lower, 4.0);
    assert_eq!(eng_st.text_table[4].text, "Fault");

    let err_cod = &project.compu_methods["DiagErrorCode_CM"];
    assert_eq!(err_cod.text_table.len(), 6, "full DiagErrorCode_TT table");
    assert_eq!(err_cod.text_table[0].text, "NoError");

    let event = project.xcp_data.as_ref().unwrap().event_info.events[0]
        .name
        .clone();
    for s in &mut project.signals {
        s.event = Some(event.clone());
    }
    let (daq, unplaced) = build_daq_list(&project.clone(), Protocol::Xcp);
    assert!(unplaced.is_empty());

    let dbc = create_dbc_file(&project, &daq, Protocol::Xcp, "integration.a2l").unwrap();
    assert!(
        dbc.contains("VAL_ ") && dbc.contains("\"Cranking\"") && dbc.contains("\"NoError\""),
        "TAB_VERB tables must appear as VAL_ lines\n{dbc}"
    );
}

const DYNAMIC_CLASSIC_A2L: &str = r#"
/begin MEASUREMENT Rpm
  "Engine speed"
  UWORD Rpm_CM
  0 0 0 8000
  ECU_ADDRESS 0x1000
/end MEASUREMENT

/begin IF_DATA XCP
  /begin XCP_ON_CAN
    0x0105
    CAN_ID_MASTER 0x0455
    CAN_ID_SLAVE  0x0456
    BAUDRATE 500000
  /end XCP_ON_CAN
  /begin DAQ
    DYNAMIC
    /begin DAQ_LIST
      0x0000
      MAX_ODT 0x05
      MAX_ODT_ENTRIES 0x07
    /end DAQ_LIST
    /begin EVENT
      "Task10ms" "10ms" 0x0000 DAQ 0x01 0x0A 0x06 0xFF
    /end EVENT
  /end DAQ
/end IF_DATA
"#;

#[test]
fn inline_a2ls_build_end_to_end() {
    for (a2l, event, message) in [
        // CAN FD DLC 12: 4 timestamp + 4 float bytes + the PID, rounded up.
        (
            STATIC_FD_A2L,
            "One_Event_Per_Rotation",
            "BO_ 1110 DAQ: 12 Slave",
        ),
        // Classic DLC = 2 data bytes + the PID.
        (DYNAMIC_CLASSIC_A2L, "Task10ms", "BO_ 1110 DAQ: 3 Slave"),
    ] {
        let mut project = parse_a2l(a2l, &[]).unwrap();
        let names: Vec<String> = project.signals.iter().map(|s| s.name.clone()).collect();
        let assignments = [(event.to_string(), names.clone())];
        let out =
            a2l_daq::build(&mut project, &assignments, Protocol::Xcp, "integration.a2l").unwrap();
        assert!(out.warnings.is_empty(), "{event}: {:?}", out.warnings);
        assert_eq!(out.daq_lists.len(), 1, "{event}");

        assert!(out.dbc.contains(message), "{event}:\n{}", out.dbc);
        for name in &names {
            assert!(
                out.dbc.contains(&format!(" SG_ {name} m0 :")),
                "{event}: {name} missing:\n{}",
                out.dbc
            );
        }

        let entries: usize = out.daq_lists[0].odts.iter().map(|o| o.entries.len()).sum();
        let writes = build_init_commands(&project, &out.daq_lists)
            .iter()
            .filter(|c| c.label.starts_with("WRITE_DAQ"))
            .count();
        assert_eq!(writes, entries, "{event}: one WRITE_DAQ per packed entry");
    }
}

const CCP_SOURCE_A2L: &str = r#"
/begin MEASUREMENT EngineLoad_pct
  "engine load"
  UWORD EngineLoad_pct_CM 0 0 0 65535
  ECU_ADDRESS 0x1000
/end MEASUREMENT
/begin MEASUREMENT OverheatFlag_flg
  "overheat flag"
  UBYTE OverheatFlag_flg_CM 0 0 0 255
  ECU_ADDRESS 0x1010
/end MEASUREMENT
/begin COMPU_METHOD EngineLoad_pct_CM "" RAT_FUNC "%5.0" "" COEFFS 0 1 0 0 0 1 /end COMPU_METHOD
/begin COMPU_METHOD OverheatFlag_flg_CM "" RAT_FUNC "%3.0" "" COEFFS 0 1 0 0 0 1 /end COMPU_METHOD
/begin IF_DATA ASAP1B_CCP
  /begin SOURCE
    "10ms List" 4 1
    /begin QP_BLOB
      0
      LENGTH 30
      FIRST_PID 0x00
    /end QP_BLOB
  /end SOURCE
  /begin SOURCE
    "100ms List" 4 1
    /begin QP_BLOB
      1
      LENGTH 30
      FIRST_PID 0x1E
    /end QP_BLOB
  /end SOURCE
  /begin TP_BLOB 0x0201 0x0206 0x455 0x456 0x0100 0x0002 /end TP_BLOB
/end IF_DATA
"#;

#[test]
fn ccp_source_blocks_drive_list_numbers_and_pids() {
    let selected = vec!["EngineLoad_pct".to_string(), "OverheatFlag_flg".to_string()];
    let mut project = parse_a2l(CCP_SOURCE_A2L, &selected).unwrap();
    let assignments = vec![
        ("10ms List".to_string(), vec!["EngineLoad_pct".to_string()]),
        (
            "100ms List".to_string(),
            vec!["OverheatFlag_flg".to_string()],
        ),
    ];

    let out = a2l_daq::build(&mut project, &assignments, Protocol::Ccp, "source.a2l").unwrap();
    assert!(out.warnings.is_empty(), "warnings: {:?}", out.warnings);
    assert_eq!(out.daq_lists.len(), 2);

    let ten_ms = &out.daq_lists[0];
    let hundred_ms = &out.daq_lists[1];
    assert_eq!(ten_ms.id, 0, "QP_BLOB list number of the 10ms SOURCE");
    assert_eq!(ten_ms.event.as_deref(), Some("10ms List"));
    assert_eq!(hundred_ms.id, 1);
    assert_eq!(hundred_ms.event.as_deref(), Some("100ms List"));
    assert_eq!(ten_ms.first_pid, 0);
    assert_eq!(hundred_ms.first_pid, 30);

    assert!(
        out.dbc.contains(" SG_ EngineLoad_pct m0 :"),
        "got:\n{}",
        out.dbc
    );
    assert!(
        out.dbc.contains(" SG_ OverheatFlag_flg m30 :"),
        "got:\n{}",
        out.dbc
    );
}

#[test]
fn ccp_daq_writes_every_element_and_scales_limits() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/data/CCP/SampleECU.a2l"
    ))
    .expect("example A2L present in repo");
    let content = a2l_daq::decode_a2l_bytes(&bytes);

    let selected: Vec<String> = [
        "CpuLoad_pct",
        "v_10V_Raw",
        "SupplyVoltage_V",
        "v_5V_Raw",
        "SerialNumber",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let project = parse_a2l(&content, &selected).unwrap();
    assert_eq!(project.signals.len(), 5, "all five selected signals parsed");

    let addr = |n: &str| {
        project
            .signals
            .iter()
            .find(|s| s.name == n)
            .unwrap()
            .ecu_address
    };
    assert_eq!(addr("CpuLoad_pct"), 0x3F000100);
    assert_eq!(addr("v_10V_Raw"), 0x3F000160);
    assert_eq!(addr("SupplyVoltage_V"), 0x3F000110);
    assert_eq!(addr("v_5V_Raw"), 0x3F000162);
    assert_eq!(addr("SerialNumber"), 0x3F000120);

    let (daq_lists, unplaced) = build_daq_list(&project.clone(), Protocol::Ccp);
    assert!(unplaced.is_empty(), "unplaced: {unplaced:?}");

    let cmds = protocol_core::protocol::build_init_commands(Protocol::Ccp, &project, &daq_lists);
    let ptr_and_write: Vec<(u8, Vec<u8>)> = cmds
        .iter()
        .filter(|c| c.bytes[0] == 0x15 || c.bytes[0] == 0x16)
        .map(|c| (c.bytes[0], c.bytes.clone()))
        .collect();
    assert!(
        ptr_and_write
            .chunks(2)
            .all(|w| w[0].0 == 0x15 && w[1].0 == 0x16),
        "each WRITE_DAQ must be preceded by its own SET_DAQ_PTR"
    );

    let writes: Vec<&Vec<u8>> = ptr_and_write
        .iter()
        .filter(|(code, _)| *code == 0x16)
        .map(|(_, b)| b)
        .collect();
    assert_eq!(writes.len(), 14, "one element per signal byte");
    assert_eq!(ptr_and_write.len(), 28, "one SET_DAQ_PTR per element");
    assert!(writes.iter().all(|b| b[2] == 1), "BYTES_ONLY element size");

    let mut written: Vec<u32> = writes
        .iter()
        .map(|b| u32::from_be_bytes([b[4], b[5], b[6], b[7]]))
        .collect();
    written.sort_unstable();
    let mut expected: Vec<u32> = Vec::new();
    for (base, len) in [
        (0x3F000100u32, 4), // CpuLoad_pct
        (0x3F000110, 4),    // SupplyVoltage_V
        (0x3F000120, 2),    // SerialNumber
        (0x3F000160, 2),    // v_10V_Raw
        (0x3F000162, 2),    // v_5V_Raw
    ] {
        expected.extend((0..len).map(|i| base + i));
    }
    expected.sort_unstable();
    assert_eq!(written, expected, "no element written twice, none missing");

    let elements: Vec<(u8, u8)> = ptr_and_write
        .iter()
        .filter(|(code, _)| *code == 0x15)
        .map(|(_, b)| (b[3], b[4]))
        .collect();
    for (odt, group) in elements.chunk_by(|a, b| a.0 == b.0).map(|g| (g[0].0, g)) {
        let seen: Vec<u8> = group.iter().map(|(_, e)| *e).collect();
        let want: Vec<u8> = (0..seen.len() as u8).collect();
        assert_eq!(seen, want, "ODT {odt} element indices");
    }

    let dbc = create_dbc_file(&project, &daq_lists, Protocol::Ccp, "SampleECU.a2l").unwrap();

    assert!(dbc.contains("BO_ 17 DAQ: 8 Slave"), "got:\n{dbc}");
    assert!(dbc.contains("[0|65535]"), "u16 limits widened:\n{dbc}");
    assert!(
        !dbc.contains("[0|255] \"Counts\""),
        "filler limits survived:\n{dbc}"
    );
    assert!(!dbc.contains(",-0)"), "negative zero offset:\n{dbc}");
    assert!(
        dbc.contains("SIG_VALTYPE_ 17 CpuLoad_pct : 1;"),
        "got:\n{dbc}"
    );
    assert!(
        dbc.contains("SIG_VALTYPE_ 17 SupplyVoltage_V : 1;"),
        "got:\n{dbc}"
    );
    assert!(dbc.contains("|32@0-"), "big-endian float layout:\n{dbc}");
}
