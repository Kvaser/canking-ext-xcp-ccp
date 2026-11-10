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

        let mut token = String::new();
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
            // Slash-prefixed keyword (/begin, /end): read on as a plain token.
            token.push('/');
        } else if c == '"' {
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
    // `/begin MEASUREMENT <technical name> <descriptive name> ...`
    tokens
        .windows(3)
        .filter(|w| w[0] == "/begin" && w[1] == "MEASUREMENT")
        .map(|w| w[2].clone())
        .collect()
}

/// Scans pre-tokenized A2L for EVENT channel definitions
pub(super) fn scan_events(tokens: &[String]) -> Vec<EventEntry> {
    blocks(tokens, "EVENT")
        .filter_map(super::xcp::parse_xcp_event)
        .collect()
}

/// Scans pre-tokenized A2L for the measurement protocols declared via `/begin IF_DATA <tag>` blocks, returned in order.
///
/// Interface tags that are not a measurement protocol are ignored.
pub(super) fn scan_protocols(tokens: &[String]) -> Vec<Protocol> {
    Protocol::ALL
        .into_iter()
        .filter(|&p| {
            tokens.windows(3).any(|w| {
                w[0] == "/begin"
                    && w[1] == "IF_DATA"
                    && Protocol::from_if_data_tag(&w[2]) == Some(p)
            })
        })
        .collect()
}

/// Starting at the first token after a `/begin` header, advances `i` past  the corresponding balanced `/end` and returns the content slice.
///
/// Nested `/begin` ... `/end` pairs are tracked and included in the returned slice.
pub(super) fn blocks<'a>(tokens: &'a [String], kw: &'a str) -> impl Iterator<Item = &'a [String]> {
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < tokens.len() {
            if tokens[i] == "/begin" && tokens.get(i + 1).is_some_and(|t| t == kw) {
                i += 2;
                return Some(extract_block(tokens, &mut i));
            }
            i += 1;
        }
        None
    })
}

/// The `(keyword, contents)` of each block directly inside `tokens`, in order.
pub(super) fn child_blocks(tokens: &[String]) -> impl Iterator<Item = (&str, &[String])> {
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < tokens.len() {
            if tokens[i] == "/begin" {
                let kw = tokens.get(i + 1).map_or("", String::as_str);
                i += 2;
                return Some((kw, extract_block(tokens, &mut i)));
            }
            i += 1;
        }
        None
    })
}

/// The token right after the first `keyword` in `tokens`.
pub(super) fn value_after<'a>(tokens: &'a [String], keyword: &str) -> Option<&'a String> {
    let at = tokens.iter().position(|t| t == keyword)?;
    tokens.get(at + 1)
}

/// Starting at `tokens[i]`, advances `i` past the matching `/end KEYWORD` and returns the slice of content tokens.
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

/// A CAN id as the XCP/CCP `IF_DATA` blocks write it, where bit 31 flags an extended (29-bit) id.
pub(super) fn parse_can_id_token(token: &str) -> Option<u32> {
    parse_u32_token(token).map(|id| id & !0x8000_0000)
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
