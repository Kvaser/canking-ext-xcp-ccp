//! Decoding raw A2L file bytes into UTF-8, falling back to Windows-1252

/// Decode raw A2L file bytes into a `String`.
pub fn decode_a2l_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.trim_start_matches('\u{FEFF}').to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252_char(b)).collect(),
    }
}

/// Windows-1252 byte to `char`.
fn cp1252_char(b: u8) -> char {
    const C1: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}',
        '\u{017D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}',
        '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}',
        '\u{9D}', '\u{017E}', '\u{0178}',
    ];
    match b {
        0x80..=0x9F => C1[(b - 0x80) as usize],
        _ => b as char,
    }
}
