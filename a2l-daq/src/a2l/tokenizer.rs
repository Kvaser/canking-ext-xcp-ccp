//! Tokenizing raw A2L source text

use super::{A2lParseError, EventEntry, Protocol};

/// Splits A2L content into tokens, stripping `/* ... */` block comments and `// ...` line comments.
/// Quoted strings (`"..."`) are emitted as a single token including the surrounding quotes.
fn tokenize(content: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = content.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }

        if c == '/' {
            chars.next();
            if chars.peek() == Some(&'*') {
                // Block comment. Consume until */
                chars.next();
                loop {
                    match chars.next() {
                        Some('*') if chars.peek() == Some(&'/') => {
                            chars.next();
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
                continue;
            }
            if chars.peek() == Some(&'/') {
                // Line comment. Consume to the end of the line.
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            // Slash-prefixed keyword (/begin, /end)
            let mut token = String::from('/');
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                token.push(c);
                chars.next();
            }
            tokens.push(token);
            continue;
        }

        if c == '"' {
            chars.next();
            let mut s = String::from('"');
            loop {
                match chars.next() {
                    Some('"') => {
                        s.push('"');
                        break;
                    }
                    Some(ch) => s.push(ch),
                    None => break,
                }
            }
            tokens.push(s);
            continue;
        }

        let mut token = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            token.push(c);
            chars.next();
        }
        if !token.is_empty() {
            tokens.push(token);
        }
    }

    tokens
}

/// Splits A2L content into tokens, returning an error if the content is empty.
pub(super) fn tokenize_checked(content: &str) -> Result<Vec<String>, A2lParseError> {
    if content.trim().is_empty() {
        return Err(A2lParseError::EmptyInput);
    }
    Ok(tokenize(content))
}

/// Scans pre-tokenized A2L for MEASUREMENT signal names.
pub(super) fn scan_signal_names(tokens: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "/begin" && tokens.get(i + 1).map(String::as_str) == Some("MEASUREMENT") {
            if let Some(name) = tokens.get(i + 2) {
                // i+2-> technical name, i+3 -> descriptive name
                names.push(name.clone());
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    names
}

/// Scans pre-tokenized A2L for EVENT channel definitions
pub(super) fn scan_events(tokens: &[String]) -> Vec<EventEntry> {
    let mut events = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "/begin" && tokens.get(i + 1).map(String::as_str) == Some("EVENT") {
            i += 2;
            let block = extract_block(tokens, &mut i);
            if let Some(entry) = super::xcp::parse_event_block(block) {
                events.push(entry);
            }
        } else {
            i += 1;
        }
    }
    events
}

/// Scans pre-tokenized A2L for the measurement protocols declared via `/begin IF_DATA <tag>` blocks, returned in order.
/// Interface tags that are not a measurement protocol are ignored.
pub(super) fn scan_protocols(tokens: &[String]) -> Vec<Protocol> {
    let mut present = [false; Protocol::ALL.len()];
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "/begin" && tokens.get(i + 1).map(String::as_str) == Some("IF_DATA") {
            if let Some(p) = tokens
                .get(i + 2)
                .and_then(|t| Protocol::from_if_data_tag(t))
            {
                if let Some(idx) = Protocol::ALL.iter().position(|&q| q == p) {
                    present[idx] = true;
                }
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    Protocol::ALL
        .iter()
        .zip(present)
        .filter_map(|(&p, seen)| seen.then_some(p))
        .collect()
}

/// Starting at `tokens[i]` (the first content token after a `/begin KEYWORD` header),
/// advances `i` past the matching `/end KEYWORD` and returns the slice of content tokens.
pub(super) fn extract_block<'a>(tokens: &'a [String], i: &mut usize) -> &'a [String] {
    let start = *i;
    let mut depth: u16 = 1;
    while *i < tokens.len() {
        if tokens[*i] == "/begin" {
            depth += 1;
            *i += 2;
        } else if tokens[*i] == "/end" {
            depth -= 1;
            *i += 2;
            if depth == 0 {
                break;
            }
        } else {
            *i += 1;
        }
    }
    let end = (*i).saturating_sub(2);
    &tokens[start..end.max(start)]
}

pub(super) fn parse_u16_token(token: &str) -> Option<u16> {
    parse_u64_token(token).and_then(|n| u16::try_from(n).ok())
}

pub(super) fn parse_u32_token(token: &str) -> Option<u32> {
    parse_u64_token(token).and_then(|n| u32::try_from(n).ok())
}

pub(super) fn parse_f64_token(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .or_else(|| parse_u64_token(value).map(|n| n as f64))
}

pub(super) fn parse_u64_token(value: &str) -> Option<u64> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else {
        value.parse::<u64>().ok()
    }
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_comments_are_stripped() {
        let tokens = tokenize("A // trailing note\nB\n// whole line\nC");
        assert_eq!(tokens, ["A", "B", "C"]);
    }

    #[test]
    fn a_line_comment_cannot_inject_keywords() {
        let tokens = tokenize("UWORD // ECU_ADDRESS 0x1000 BIT_MASK 0xFF\n0 8000");
        assert_eq!(tokens, ["UWORD", "0", "8000"]);
    }

    #[test]
    fn block_comments_are_stripped() {
        assert_eq!(tokenize("A /* note\nspanning lines */ B"), ["A", "B"]);
    }

    #[test]
    fn slashes_inside_a_string_are_not_comments() {
        assert_eq!(
            tokenize(r#""http://x" "a /* b" C"#),
            [r#""http://x""#, r#""a /* b""#, "C"]
        );
    }

    #[test]
    fn slash_keywords_still_tokenize() {
        assert_eq!(
            tokenize("/begin MEASUREMENT /end MEASUREMENT"),
            ["/begin", "MEASUREMENT", "/end", "MEASUREMENT"]
        );
    }

    #[test]
    fn a_comment_at_end_of_input_terminates_cleanly() {
        assert_eq!(tokenize("A // no trailing newline"), ["A"]);
    }
}
