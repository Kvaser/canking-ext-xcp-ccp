//! The A2L `MEASUREMENT` base datatype and its size in bytes, signedness and floating-point-ness.

/// `(size in bytes, signed, float)` of an A2L datatype keyword
///
/// `None` if `token` isn't one.
pub(super) fn datatype(token: &str) -> Option<(u16, bool, bool)> {
    Some(match token {
        "UBYTE" => (1, false, false),
        "SBYTE" => (1, true, false),
        "UWORD" => (2, false, false),
        "SWORD" => (2, true, false),
        "ULONG" => (4, false, false),
        "SLONG" => (4, true, false),
        "A_UINT64" => (8, false, false),
        "A_INT64" => (8, true, false),
        "FLOAT32_IEEE" => (4, true, true),
        "FLOAT64_IEEE" => (8, true, true),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::datatype;

    #[test]
    fn every_datatype_token_round_trips_through_from_token() {
        for (token, size, signed, float) in [
            ("UBYTE", 1, false, false),
            ("SBYTE", 1, true, false),
            ("UWORD", 2, false, false),
            ("SWORD", 2, true, false),
            ("ULONG", 4, false, false),
            ("SLONG", 4, true, false),
            ("A_UINT64", 8, false, false),
            ("A_INT64", 8, true, false),
            ("FLOAT32_IEEE", 4, true, true),
            ("FLOAT64_IEEE", 8, true, true),
        ] {
            assert_eq!(datatype(token), Some((size, signed, float)), "{token}");
        }
    }

    #[test]
    fn unrecognised_token_is_not_a_datatype() {
        assert_eq!(datatype("Rpm"), None);
        assert_eq!(datatype(""), None);
    }
}
