//! The A2L `MEASUREMENT` base datatype and its size in bytes, signedness and floating-point-ness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum A2lDataType {
    UByte,
    SByte,
    UWord,
    SWord,
    ULong,
    SLong,
    AUInt64,
    AInt64,
    Float32Ieee,
    Float64Ieee,
}

impl A2lDataType {
    /// Parse the A2L keyword for a datatype, or `None` if `token` isn't one.
    pub(super) fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "UBYTE" => Self::UByte,
            "SBYTE" => Self::SByte,
            "UWORD" => Self::UWord,
            "SWORD" => Self::SWord,
            "ULONG" => Self::ULong,
            "SLONG" => Self::SLong,
            "A_UINT64" => Self::AUInt64,
            "A_INT64" => Self::AInt64,
            "FLOAT32_IEEE" => Self::Float32Ieee,
            "FLOAT64_IEEE" => Self::Float64Ieee,
            _ => return None,
        })
    }

    pub(super) fn size_bytes(self) -> u16 {
        match self {
            Self::UByte | Self::SByte => 1,
            Self::UWord | Self::SWord => 2,
            Self::ULong | Self::SLong | Self::Float32Ieee => 4,
            Self::AUInt64 | Self::AInt64 | Self::Float64Ieee => 8,
        }
    }

    pub(super) fn is_signed(self) -> bool {
        matches!(
            self,
            Self::SByte
                | Self::SWord
                | Self::SLong
                | Self::AInt64
                | Self::Float32Ieee
                | Self::Float64Ieee
        )
    }

    pub(super) fn is_float(self) -> bool {
        matches!(self, Self::Float32Ieee | Self::Float64Ieee)
    }
}

#[cfg(test)]
mod tests {
    use super::A2lDataType;

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
            let dt =
                A2lDataType::from_token(token).unwrap_or_else(|| panic!("{token} did not parse"));
            assert_eq!(dt.size_bytes(), size, "{token} size");
            assert_eq!(dt.is_signed(), signed, "{token} signedness");
            assert_eq!(dt.is_float(), float, "{token} float-ness");
        }
    }

    #[test]
    fn unrecognised_token_is_not_a_datatype() {
        assert_eq!(A2lDataType::from_token("Rpm"), None);
        assert_eq!(A2lDataType::from_token(""), None);
    }
}
