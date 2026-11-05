//! A2L keywords that change what the outputs mean but that the parser doesn't read yet.
//!
//! [`scan`] finds them so the user knows which outputs not to trust. To flag another keyword, add a row to
//! [`EFFECTS`] (and a case to [`keyword_at`] if a bare token match isn't enough).
//! Once the parser handles a keyword, remove its row.

use std::fmt::{self, Display, Formatter};

use super::tokenizer::tokenize_checked;

/// An unread keyword found in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadKeyword {
    /// The keyword as it appears in the A2L, e.g. `/include` or `ADDRESS_GRANULARITY_WORD`.
    pub keyword: String,
    /// How many times it occurs.
    pub count: u32,
    /// What goes wrong because it isn't read.
    pub effect: String,
}

impl Display for UnreadKeyword {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.count {
            1 => write!(f, "{}: {}", self.keyword, self.effect),
            n => write!(f, "{} ({n}x): {}", self.keyword, self.effect),
        }
    }
}

const RELATIVE_PID: &str = "DAQ frames are decoded as if they start with a 1-byte absolute PID, so signals are read from the wrong bytes.";
const GRANULARITY: &str = "WRITE_DAQ sizes are sent in bytes, not in the ECU's address granularity, so the ECU samples the wrong amount of memory.";

/// Every flagged keyword with its effect, in report order.
const EFFECTS: &[(&str, &str)] = &[
    (
        "/include",
        "Included files are not read, so the signals, events and protocol settings they declare are missing.",
    ),
    (
        "INSTANCE",
        "Signals declared as an INSTANCE of a TYPEDEF are not listed.",
    ),
    (
        "MODULE",
        "The file has several MODULEs. Their signals are listed together and one module's protocol settings are used for all.",
    ),
    ("IDENTIFICATION_FIELD_TYPE_RELATIVE_BYTE", RELATIVE_PID),
    ("IDENTIFICATION_FIELD_TYPE_RELATIVE_WORD", RELATIVE_PID),
    (
        "IDENTIFICATION_FIELD_TYPE_RELATIVE_WORD_ALIGNED",
        RELATIVE_PID,
    ),
    ("ADDRESS_GRANULARITY_WORD", GRANULARITY),
    ("ADDRESS_GRANULARITY_DWORD", GRANULARITY),
    (
        "ADDRESS_TYPE",
        "Some measurements hold a pointer to their value (ADDRESS_TYPE other than DIRECT); the pointer is logged instead of the value.",
    ),
    (
        "OVERLOAD_INDICATION_PID",
        "DAQ frames the ECU flags as overloaded (bit 7 of the PID set) are dropped.",
    ),
    (
        "FIXED_EVENT_LIST",
        "Some signals may only be measured on the events their DAQ_EVENT lists. That isn't checked, so the ECU may reject an assignment.",
    ),
];

/// The flagged keyword starting at `tokens[i]`, if any.
fn keyword_at(tokens: &[String], i: usize) -> Option<&'static str> {
    let next = tokens.get(i + 1).map(String::as_str);
    match (tokens[i].as_str(), next) {
        (token, _) if token.eq_ignore_ascii_case("/include") => Some("/include"),
        ("/begin", Some("INSTANCE")) => Some("INSTANCE"),
        ("/begin", Some("MODULE")) => Some("MODULE"),
        ("ADDRESS_TYPE", Some(kind)) if kind != "DIRECT" => Some("ADDRESS_TYPE"),
        ("ADDRESS_TYPE" | "INSTANCE" | "MODULE", _) => None,
        (token, _) => EFFECTS.iter().map(|(k, _)| *k).find(|k| *k == token),
    }
}

/// The unread keywords in `tokens`, in [`EFFECTS`] order. A single `MODULE` is normal and not reported.
pub(crate) fn scan(tokens: &[String]) -> Vec<UnreadKeyword> {
    let mut counts = [0u32; EFFECTS.len()];
    for i in 0..tokens.len() {
        if let Some(keyword) = keyword_at(tokens, i) {
            let row = EFFECTS.iter().position(|(k, _)| *k == keyword);
            counts[row.expect("keyword_at only returns EFFECTS keywords")] += 1;
        }
    }
    EFFECTS
        .iter()
        .zip(counts)
        .filter(|((keyword, _), count)| *count >= if *keyword == "MODULE" { 2 } else { 1 })
        .map(|((keyword, effect), count)| UnreadKeyword {
            keyword: keyword.to_string(),
            count,
            effect: effect.to_string(),
        })
        .collect()
}

/// The unread keywords in an A2L file's text, see [`scan`].
pub fn unread_keywords(content: &str) -> Vec<UnreadKeyword> {
    tokenize_checked(content)
        .map(|tokens| scan(&tokens))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<String> {
        tokenize_checked(text).expect("non-empty")
    }

    fn found(text: &str) -> Vec<(String, u32)> {
        scan(&tokens(text))
            .into_iter()
            .map(|u| (u.keyword, u.count))
            .collect()
    }

    #[test]
    fn reports_each_unread_keyword_with_its_count() {
        let text = r#"
            /include "a.a2l" /INCLUDE "b.a2l"
            /begin MODULE m1 "" /begin INSTANCE i "" t 0x10 /end INSTANCE /end MODULE
            /begin MODULE m2 "" /end MODULE
            STATIC 0x3 0x2 0x0 OPTIMISATION_TYPE_DEFAULT ADDRESS_EXTENSION_FREE IDENTIFICATION_FIELD_TYPE_RELATIVE_WORD
            BYTE_ORDER_MSB_LAST ADDRESS_GRANULARITY_DWORD OVERLOAD_INDICATION_PID
            /begin MEASUREMENT a "" UBYTE NO_COMPU_METHOD 0 0 0 255 ADDRESS_TYPE PLONG /end MEASUREMENT
            /begin MEASUREMENT b "" UBYTE NO_COMPU_METHOD 0 0 0 255 ADDRESS_TYPE PBYTE
              /begin IF_DATA XCP /begin DAQ_EVENT FIXED_EVENT_LIST EVENT 0x1 /end DAQ_EVENT /end IF_DATA
            /end MEASUREMENT
        "#;
        assert_eq!(
            found(text),
            [
                ("/include".into(), 2),
                ("INSTANCE".into(), 1),
                ("MODULE".into(), 2),
                ("IDENTIFICATION_FIELD_TYPE_RELATIVE_WORD".into(), 1),
                ("ADDRESS_GRANULARITY_DWORD".into(), 1),
                ("ADDRESS_TYPE".into(), 2),
                ("OVERLOAD_INDICATION_PID".into(), 1),
                ("FIXED_EVENT_LIST".into(), 1),
            ]
        );
    }

    #[test]
    fn ignores_what_the_parser_already_handles() {
        let text = r#"
            /begin MODULE m "" "INSTANCE" /end MODULE
            IDENTIFICATION_FIELD_TYPE_ABSOLUTE ADDRESS_GRANULARITY_BYTE NO_OVERLOAD_INDICATION
            /begin MEASUREMENT a "" UBYTE NO_COMPU_METHOD 0 0 0 255 ADDRESS_TYPE DIRECT /end MEASUREMENT
            /begin DAQ_EVENT VARIABLE /begin DEFAULT_EVENT_LIST EVENT 0x1 /end DEFAULT_EVENT_LIST /end DAQ_EVENT
        "#;
        assert_eq!(found(text), []);
    }
}
