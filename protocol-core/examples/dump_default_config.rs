//! Prints `Config::sample()` as pretty JSON.
//!
//! Run with: `cargo run -p protocol-core --features serde --example dump_default_config`

fn main() {
    let json = serde_json::to_string_pretty(&protocol_core::config::Config::sample())
        .expect("Config::sample() always serializes");
    println!("{json}");
}
