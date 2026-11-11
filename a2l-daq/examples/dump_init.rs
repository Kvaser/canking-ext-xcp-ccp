// Test xcp command generation
use a2l_daq::{Protocol, build_daq_list, build_init_commands, parse_a2l};

fn main() {
    let content = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/data/XCP/SampleECU.a2l"
    ))
    .expect("A2L present");

    let names = a2l_daq::list_signal_names(&content).unwrap();
    let selected: Vec<String> = names.into_iter().take(10).collect();
    println!("selected: {selected:?}");

    let mut project = parse_a2l(&content, &selected).unwrap();
    let event = project.xcp_data.as_ref().unwrap().event_info.events[0]
        .name
        .clone();
    for s in &mut project.signals {
        s.event = Some(event.clone());
    }

    let (daq, unplaced) = build_daq_list(&project.clone(), Protocol::Xcp);
    println!("daq lists: {}, unplaced: {}", daq.len(), unplaced.len());
    for d in &daq {
        for (i, odt) in d.odts.iter().enumerate() {
            let total: u16 = odt.entries.iter().map(|e| e.size).sum();
            println!(
                "  list {} odt {}: {} entries, {} data bytes",
                d.id,
                i,
                odt.entries.len(),
                total
            );
        }
    }

    for cmd in build_init_commands(&project, &daq) {
        let bytes: Vec<String> = cmd.bytes.iter().map(|b| format!("{b:02X}")).collect();
        println!(
            "[{:13}] {:24} -> {}",
            cmd.phase.tag(),
            format!("[{}]", bytes.join(" ")),
            cmd.label
        );
    }
}
