//! Decoding DAQ signals inside the t-script, so the script needs no DBC.
//!
//! With a DBC, `scc` generates a struct per message and `this.Sig.Phys` reads the right bytes and applies the scaling.
//! Without one, the script has to do that itself: this module turns the [`crate::layout`] placements into
//! `float sig_<name>(const byte data[])` functions plus the globals that latch their values.

use crate::ByteOrder;
use crate::a2l::A2lProject;
use crate::daq::DaqList;
use crate::dbc::sanitize_name;
use crate::layout::{SignalPlacement, effective_bit_length, find_placement, signal_placements};
use std::error::Error;
use std::fmt::{self, Display, Formatter, Write as FmtWrite};

/// The one-float struct the `FLOAT32_IEEE` decoders deserialize through.
const FLOAT_HELPER_TYPE: &str = "TFloat32";

/// Everything needed to emit one signal's decoder.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalDecoder {
    pub ident: String,
    pub source_name: String,
    pub can_id: u32,
    pub pid: u16,
    pub byte_offset: u32,
    pub byte_size: u32,
    pub bit_length: u32,
    pub dtype: DecoderType,
    pub big_endian: bool,
    pub factor: f64,
    pub offset: f64,
    pub unit: String,
}

impl SignalDecoder {
    /// Whether this decoder deserializes a `FLOAT32_IEEE` field.
    pub fn is_float(&self) -> bool {
        matches!(self.dtype, DecoderType::Float)
    }
}

/// How a decoder turns its raw bytes into a value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecoderType {
    Int {
        signed: bool,
        masked: bool,
        shift: u32,
    },
    Float,
}

/// Why a signal cannot be decoded inside the script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerError {
    /// The name is not in any packed DAQ list, so no frame carries it.
    NotInDaqList { name: String },
    /// The signal is split across ODT entries, so no single frame holds all of it and it cannot be decoded from one message.
    SplitAcrossOdts { name: String },
    /// Bigger than 32 bits
    TooWide { name: String, bits: u32 },
    /// A float that is not `FLOAT32_IEEE`
    UnsupportedFloat { name: String, bits: u32 },
}

impl Display for TriggerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInDaqList { name } => write!(
                f,
                "trigger signal '{name}' is not in any DAQ list, so no DAQ frame carries it"
            ),
            Self::SplitAcrossOdts { name } => write!(
                f,
                "trigger signal '{name}' is split across ODT entries; a single DAQ frame does not hold all of it."
            ),
            Self::TooWide { name, bits } => write!(
                f,
                "trigger signal '{name}' is {bits} bits wide; the t language has no integer type wider than 32 bits"
            ),
            Self::UnsupportedFloat { name, bits } => write!(
                f,
                "trigger signal '{name}' is a {bits}-bit float; the t language has no 'double' type, only FLOAT32_IEEE can be decoded in the script. 
                Use DBC mode for this signal."
            ),
        }
    }
}

impl Error for TriggerError {}

/// Build the decoders for `names`, in the order given, looked up in the DAQ layout. Repeated names collapse to one decoder.
///  `default_can_id` resolves a list's CAN id when it declares no `DAQ_LIST_CAN_ID`/`CAN_ID_FIXED`
pub fn decoders_for(
    project: &A2lProject,
    daq_lists: &[DaqList],
    names: &[String],
    default_can_id: u32,
) -> Result<Vec<SignalDecoder>, TriggerError> {
    let groups = crate::layout::group_by_can_id(daq_lists, default_can_id);
    let grouped_placements: Vec<(u32, Vec<SignalPlacement>)> = groups
        .iter()
        .map(|(can_id, lists)| (*can_id, signal_placements(project, lists)))
        .collect();

    let mut decoders: Vec<SignalDecoder> = Vec::new();
    for name in names {
        let ident = sanitize_name(name);
        if decoders.iter().any(|d| d.ident == ident) {
            continue;
        }
        let (can_id, placement) = grouped_placements
            .iter()
            .find_map(|(can_id, placements)| {
                find_placement(placements, name, sanitize_name).map(|p| (*can_id, p))
            })
            .ok_or_else(|| TriggerError::NotInDaqList { name: name.clone() })?;
        decoders.push(decoder_for(can_id, placement, &ident)?);
    }
    Ok(decoders)
}

/// Turn one placement into a decoder, rejecting what t cannot express.
fn decoder_for(
    can_id: u32,
    p: &SignalPlacement,
    ident: &str,
) -> Result<SignalDecoder, TriggerError> {
    if p.chunk_size.is_some() {
        return Err(TriggerError::SplitAcrossOdts {
            name: p.name.clone(),
        });
    }
    if p.is_float() && (p.bit_length != 32 || p.span_bytes != 4) {
        return Err(TriggerError::UnsupportedFloat {
            name: p.name.clone(),
            bits: p.span_bytes as u32 * 8,
        });
    }
    // Check size
    if p.bit_length > 32 || p.span_bytes > 4 {
        return Err(TriggerError::TooWide {
            name: p.name.clone(),
            bits: p.bit_length.max(p.span_bytes as u32 * 8),
        });
    }

    let decoder_type = if p.is_float() {
        DecoderType::Float
    } else {
        let mask = p.bit_mask();
        let shift = mask.map(|m| m.trailing_zeros()).unwrap_or(0);
        let masked =
            mask.is_some_and(|m| shift > 0 || effective_bit_length(m) < p.span_bytes as u32 * 8);
        DecoderType::Int {
            signed: p.is_signed(),
            masked,
            shift,
        }
    };

    Ok(SignalDecoder {
        ident: ident.to_string(),
        source_name: p.name.clone(),
        can_id,
        pid: p.pid,
        byte_offset: p.byte_offset(),
        byte_size: p.span_bytes as u32,
        bit_length: p.bit_length,
        dtype: decoder_type,
        big_endian: p.byte_order() == ByteOrder::MsbFirst,
        factor: p.factor,
        offset: p.offset,
        unit: p.unit.clone(),
    })
}

/// The `typedef` the float decoders need, for the script's `variables` block. Empty when no decoder decodes a float.
pub fn helper_typedef(decoders: &[SignalDecoder]) -> String {
    if !decoders.iter().any(SignalDecoder::is_float) {
        return String::new();
    }
    format!(
        "    /* Deserialization target for IEEE-754 float signals */\n\
         \x20   typedef struct {{\n\
         \x20       float value;\n\
         \x20   }} {FLOAT_HELPER_TYPE};\n"
    )
}

/// The decoder functions, as a top-level block.
pub fn decoder_functions(decoders: &[SignalDecoder]) -> String {
    if decoders.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "/*\n    Signal decoders\n\n\
         \x20   Each function reads one signal out of a DAQ frame payload and returns\n\
         \x20   its physical value. Byte offsets count from the start of the frame,\n\
         \x20   where byte 0 is the PID.\n*/\n\n",
    );
    for d in decoders {
        out.push_str(&decoder_function(d));
        out.push('\n');
    }
    out
}

/// One decoder function.
fn decoder_function(d: &SignalDecoder) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "/* {} */", decoder_comment(d));
    let _ = writeln!(out, "float sig_{}(const byte data[]) {{", d.ident);

    let (signed, masked, shift) = match d.dtype {
        DecoderType::Float => {
            let _ = writeln!(out, "    byte buf[4];");
            let _ = writeln!(out, "    {FLOAT_HELPER_TYPE} value;");
            // A big-endian field is reversed.
            for i in 0..4u32 {
                let src = if d.big_endian {
                    d.byte_offset + 3 - i
                } else {
                    d.byte_offset + i
                };
                let _ = writeln!(out, "    buf[{i}] = data[{src}];");
            }
            let _ = writeln!(out, "    value = buf;");
            let _ = writeln!(out, "    return {};", scale("value.value", d));
            out.push_str("}\n");
            return out;
        }
        DecoderType::Int {
            signed,
            masked,
            shift,
        } => (signed, masked, shift),
    };

    // A 32-bit unsigned field overflows t's signed int, so widen it.
    let widen = d.bit_length == 32 && !signed;

    let _ = writeln!(out, "    int raw;");
    if widen {
        let _ = writeln!(out, "    float value;");
    }
    let _ = writeln!(
        out,
        "    raw = {};",
        assemble(d.byte_offset, d.byte_size, d.big_endian)
    );
    if masked {
        let mask_bits = (1u64 << d.bit_length) - 1;
        let _ = writeln!(out, "    raw = (raw >> {}) & 0x{:X};", shift, mask_bits);
    }
    if signed && d.bit_length < 32 {
        out.push_str(&sign_extend(d.bit_length));
    }
    if widen {
        let _ = writeln!(out, "    value = raw;");
        out.push_str("    /* the field's top bit lands on t's int sign bit */\n");
        out.push_str("    if (raw < 0) {\n");
        out.push_str("        value += 4294967296.0;\n");
        out.push_str("    }\n");
        let _ = writeln!(out, "    return {};", scale("value", d));
    } else {
        let _ = writeln!(out, "    return {};", scale("raw", d));
    }
    out.push_str("}\n");
    out
}

/// The one-line summary above each decoder.
fn decoder_comment(d: &SignalDecoder) -> String {
    let last = d.byte_offset + d.byte_size - 1;
    let bytes = if d.byte_size == 1 {
        format!("byte {}", d.byte_offset)
    } else {
        format!("bytes {}..{}", d.byte_offset, last)
    };
    let kind = match d.dtype {
        DecoderType::Float => "f32".to_string(),
        DecoderType::Int { signed, .. } => {
            format!("{}{}", if signed { "i" } else { "u" }, d.bit_length)
        }
    };
    let order = if d.byte_size == 1 {
        String::new()
    } else if d.big_endian {
        " BE".to_string()
    } else {
        " LE".to_string()
    };
    let unit = if d.unit.is_empty() {
        String::new()
    } else {
        format!("  [{}]", d.unit)
    };
    // Spell the mask out: the read window is wider than the field.
    let mask = match d.dtype {
        DecoderType::Int {
            masked: true,
            shift,
            ..
        } => format!(" (>>{} of {} bits)", shift, d.byte_size * 8),
        _ => String::new(),
    };
    format!(
        "{}  PID {}, {}, {}{}{}, phys = {}{}",
        d.source_name,
        d.pid,
        bytes,
        kind,
        order,
        mask,
        scale("raw", d),
        unit
    )
}

/// Assemble `byte_size` bytes starting at `byte_offset` into an int expression.
fn assemble(byte_offset: u32, byte_size: u32, big_endian: bool) -> String {
    (0..byte_size)
        .map(|i| {
            let shift = if big_endian {
                (byte_size - 1 - i) * 8
            } else {
                i * 8
            };
            let idx = byte_offset + i;
            if shift == 0 {
                format!("data[{idx}]")
            } else {
                format!("(data[{idx}] << {shift})")
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Sign-extend a `bits`-wide two's-complement value held in an int.
fn sign_extend(bits: u32) -> String {
    let half: u64 = 1 << (bits - 1);
    if bits <= 30 {
        format!(
            "    if (raw >= {half}) {{\n        raw -= {};\n    }}\n",
            half * 2
        )
    } else {
        format!(
            "    if (raw >= {half}) {{\n        raw -= {half};\n        raw -= {half};\n    }}\n"
        )
    }
}

/// Apply `factor`/`offset` to a decoded value expression.
fn scale(expr: &str, d: &SignalDecoder) -> String {
    let mut out = expr.to_string();
    if d.factor != 1.0 {
        out = format!("{out} * {}", t_float(d.factor));
    }
    if d.offset > 0.0 {
        out = format!("{out} + {}", t_float(d.offset));
    } else if d.offset < 0.0 {
        out = format!("{out} - {}", t_float(-d.offset));
    }
    out
}

/// Format a float for a t source literal
fn t_float(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{v:.1}");
    }
    let plain = format!("{v}");
    if !plain.contains('e') && !plain.contains('E') {
        return plain;
    }
    let mut expanded = format!("{v:.17}");
    while expanded.ends_with('0') && !expanded.ends_with(".0") {
        expanded.pop();
    }
    expanded
}

// ------------------------------- Unit tests ------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2l::{CanIdSource, CompuMethod, Conversion, ParsedSignal};
    use crate::daq::{DaqList, Odt, OdtEntry};

    const TEST_CAN_ID: u32 = 0x456;

    fn project(signals: Vec<ParsedSignal>) -> A2lProject {
        A2lProject {
            signals,
            ..Default::default()
        }
    }

    fn sig(name: &str, size: u16) -> ParsedSignal {
        ParsedSignal {
            name: name.into(),
            size,
            ecu_address: 0x10,
            ..Default::default()
        }
    }

    fn daq(entries: Vec<(&str, u16)>) -> Vec<DaqList> {
        vec![DaqList {
            id: 0,
            first_pid: 1,
            event: None,
            odts: vec![Odt {
                entries: entries
                    .into_iter()
                    .map(|(name, size)| OdtEntry {
                        names: vec![name.into()],
                        ecu_address: 0x10,
                        size,
                    })
                    .collect(),
            }],
            can_id: CanIdSource::Default,
        }]
    }

    fn build(project: &A2lProject, lists: &[DaqList], name: &str) -> SignalDecoder {
        decoders_for(project, lists, &[name.to_string()], TEST_CAN_ID)
            .expect("decoder builds")
            .remove(0)
    }

    #[test]
    fn unsigned_little_endian_assembles_low_byte_first() {
        let p = project(vec![sig("Rpm", 2)]);
        let d = build(&p, &daq(vec![("Rpm", 2)]), "Rpm");
        let f = decoder_function(&d);
        assert!(
            f.contains("float sig_Rpm(const byte data[]) {"),
            "got:\n{f}"
        );
        assert!(f.contains("raw = data[1] | (data[2] << 8);"), "got:\n{f}");
        assert!(
            f.contains("return raw;"),
            "identity scaling stays bare:\n{f}"
        );
        assert!(!f.contains("if (raw"), "unsigned needs no fixup:\n{f}");
    }

    #[test]
    fn big_endian_assembles_high_byte_first() {
        let mut s = sig("Be", 4);
        s.byte_order = Some(ByteOrder::MsbFirst);
        let p = project(vec![s]);
        let d = build(&p, &daq(vec![("Be", 4)]), "Be");
        let f = decoder_function(&d);
        assert!(
            f.contains("raw = (data[1] << 24) | (data[2] << 16) | (data[3] << 8) | data[4];"),
            "got:\n{f}"
        );
    }

    #[test]
    fn signed_field_is_sign_extended_with_literals() {
        let mut s = sig("Temp", 2);
        s.is_signed = true;
        let p = project(vec![s]);
        let d = build(&p, &daq(vec![("Temp", 2)]), "Temp");
        let f = decoder_function(&d);
        assert!(
            f.contains("if (raw >= 32768) {\n        raw -= 65536;\n    }"),
            "got:\n{f}"
        );
    }

    #[test]
    fn signed_32_bit_needs_no_extension() {
        let mut s = sig("Big", 4);
        s.is_signed = true;
        let p = project(vec![s]);
        let d = build(&p, &daq(vec![("Big", 4)]), "Big");
        let f = decoder_function(&d);
        assert!(!f.contains("raw -="), "already two's complement:\n{f}");
        assert!(!f.contains("float value;"), "signed fits an int:\n{f}");
    }

    #[test]
    fn signed_31_bit_avoids_an_unrepresentable_literal() {
        let mut s = sig("Odd", 4);
        s.is_signed = true;
        s.bit_mask = Some(0x7FFF_FFFF);
        let p = project(vec![s]);
        let d = build(&p, &daq(vec![("Odd", 4)]), "Odd");
        let f = decoder_function(&d);
        assert_eq!(d.bit_length, 31);
        assert!(
            f.contains("raw -= 1073741824;\n        raw -= 1073741824;"),
            "got:\n{f}"
        );
        assert!(!f.contains("2147483648"), "got:\n{f}");
    }

    #[test]
    fn unsigned_32_bit_widens_through_a_float() {
        let p = project(vec![sig("Counter", 4)]);
        let d = build(&p, &daq(vec![("Counter", 4)]), "Counter");
        let f = decoder_function(&d);
        assert!(f.contains("float value;"), "got:\n{f}");
        assert!(f.contains("value = raw;"), "got:\n{f}");
        assert!(
            f.contains("if (raw < 0) {\n        value += 4294967296.0;\n    }"),
            "got:\n{f}"
        );
        assert!(f.contains("return value;"), "got:\n{f}");
    }

    #[test]
    fn bit_mask_reads_the_whole_span_before_shifting() {
        let mut s = sig("Nibble", 2);
        s.bit_mask = Some(0x0FF0);
        let p = project(vec![s]);
        let d = build(&p, &daq(vec![("Nibble", 2)]), "Nibble");
        let f = decoder_function(&d);
        let DecoderType::Int { shift, .. } = d.dtype else {
            panic!("expected an Int decoder, got {:?}", d.dtype)
        };
        assert_eq!((shift, d.bit_length, d.byte_size), (4, 8, 2));
        assert!(
            f.contains("raw = data[1] | (data[2] << 8);"),
            "must read both bytes:\n{f}"
        );
        assert!(f.contains("raw = (raw >> 4) & 0xFF;"), "got:\n{f}");
    }

    #[test]
    fn full_width_mask_emits_no_masking_step() {
        let mut s = sig("Full", 2);
        s.bit_mask = Some(0xFFFF);
        let p = project(vec![s]);
        let d = build(&p, &daq(vec![("Full", 2)]), "Full");
        assert!(!matches!(d.dtype, DecoderType::Int { masked: true, .. }));
        assert!(!decoder_function(&d).contains(">>"));
    }

    #[test]
    fn masking_never_needs_a_literal_wider_than_an_int() {
        let mut s = sig("Wide", 4);
        s.bit_mask = Some(0xFFFF_FFFF);
        let p = project(vec![s]);
        let d = build(&p, &daq(vec![("Wide", 4)]), "Wide");
        assert_eq!(d.bit_length, 32);
        assert!(
            !matches!(d.dtype, DecoderType::Int { masked: true, .. }),
            "a full-width mask needs no masking step"
        );
        assert!(!decoder_function(&d).contains("0xFFFFFFFF"));
    }

    #[test]
    fn span_wider_than_an_int_is_rejected_even_when_the_mask_would_fit() {
        let mut s = sig("Masked6", 6);
        s.bit_mask = Some(0xFF);
        let p = project(vec![s]);
        let err = decoders_for(
            &p,
            &daq(vec![("Masked6", 6)]),
            &["Masked6".into()],
            TEST_CAN_ID,
        )
        .unwrap_err();
        assert_eq!(
            err,
            TriggerError::TooWide {
                name: "Masked6".into(),
                bits: 48
            }
        );
    }

    #[test]
    fn scaling_renders_factor_and_signed_offset() {
        let mut s = sig("Scaled", 2);
        s.conversion = Some(Conversion {
            identifier: "CM".into(),
        });
        let mut p = project(vec![s]);
        p.compu_methods.insert(
            "CM".into(),
            CompuMethod {
                factor: 0.1,
                offset: -40.0,
                unit: "degC".into(),
                ..Default::default()
            },
        );
        let d = build(&p, &daq(vec![("Scaled", 2)]), "Scaled");
        let f = decoder_function(&d);
        assert!(f.contains("return raw * 0.1 - 40.0;"), "got:\n{f}");
        assert!(f.contains("[degC]"), "unit reaches the comment:\n{f}");
    }

    #[test]
    fn float32_deserializes_through_the_helper_struct() {
        let mut s = sig("Volt", 4);
        s.is_float = true;
        let p = project(vec![s]);
        let d = build(&p, &daq(vec![("Volt", 4)]), "Volt");
        let f = decoder_function(&d);
        assert!(f.contains("byte buf[4];"), "got:\n{f}");
        assert!(f.contains("TFloat32 value;"), "got:\n{f}");
        assert!(f.contains("buf[0] = data[1];"), "got:\n{f}");
        assert!(f.contains("buf[3] = data[4];"), "got:\n{f}");
        assert!(f.contains("value = buf;"), "got:\n{f}");
        assert!(f.contains("return value.value;"), "got:\n{f}");
        assert!(helper_typedef(&[d]).contains("} TFloat32;"));
    }

    #[test]
    fn big_endian_float32_reverses_the_bytes() {
        let mut s = sig("Volt", 4);
        s.is_float = true;
        s.byte_order = Some(ByteOrder::MsbFirst);
        let p = project(vec![s]);
        let d = build(&p, &daq(vec![("Volt", 4)]), "Volt");
        let f = decoder_function(&d);
        assert!(f.contains("buf[0] = data[4];"), "got:\n{f}");
        assert!(f.contains("buf[3] = data[1];"), "got:\n{f}");
    }

    #[test]
    fn helper_typedef_omitted_without_floats() {
        let p = project(vec![sig("Int", 2)]);
        let d = build(&p, &daq(vec![("Int", 2)]), "Int");
        assert!(helper_typedef(&[d]).is_empty());
    }

    #[test]
    fn float64_is_rejected_because_t_has_no_double() {
        let mut s = sig("Precise", 8);
        s.is_float = true;
        let p = project(vec![s]);
        let err = decoders_for(
            &p,
            &daq(vec![("Precise", 8)]),
            &["Precise".into()],
            TEST_CAN_ID,
        )
        .unwrap_err();
        assert_eq!(
            err,
            TriggerError::UnsupportedFloat {
                name: "Precise".into(),
                bits: 64
            }
        );
        assert!(err.to_string().contains("no 'double'"), "got: {err}");
    }

    #[test]
    fn wider_than_32_bits_is_rejected() {
        let p = project(vec![sig("Wide", 8)]);
        let err =
            decoders_for(&p, &daq(vec![("Wide", 8)]), &["Wide".into()], TEST_CAN_ID).unwrap_err();
        assert_eq!(
            err,
            TriggerError::TooWide {
                name: "Wide".into(),
                bits: 64
            }
        );
    }

    #[test]
    fn split_signal_is_rejected_as_undecodable_from_one_frame() {
        let p = project(vec![sig("Long", 6)]);
        let lists = vec![DaqList {
            id: 0,
            first_pid: 1,
            event: None,
            odts: vec![Odt {
                entries: vec![OdtEntry {
                    names: vec!["Long[0]".into()],
                    ecu_address: 0x10,
                    size: 4,
                }],
            }],
            can_id: CanIdSource::Default,
        }];
        let err = decoders_for(&p, &lists, &["Long".into()], TEST_CAN_ID).unwrap_err();
        assert!(
            matches!(err, TriggerError::SplitAcrossOdts { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn signal_absent_from_the_daq_lists_is_rejected() {
        let p = project(vec![sig("Rpm", 2)]);
        let err =
            decoders_for(&p, &daq(vec![("Rpm", 2)]), &["Ghost".into()], TEST_CAN_ID).unwrap_err();
        assert_eq!(
            err,
            TriggerError::NotInDaqList {
                name: "Ghost".into()
            }
        );
    }

    #[test]
    fn repeated_signal_yields_one_decoder() {
        let p = project(vec![sig("Rpm", 2)]);
        let ds = decoders_for(
            &p,
            &daq(vec![("Rpm", 2)]),
            &["Rpm".into(), "Rpm".into()],
            TEST_CAN_ID,
        )
        .unwrap();
        assert_eq!(ds.len(), 1);
    }

    #[test]
    fn t_float_never_emits_exponent_notation() {
        assert_eq!(t_float(1.0), "1.0");
        assert_eq!(t_float(-40.0), "-40.0");
        assert_eq!(t_float(0.1), "0.1");
        assert_eq!(t_float(1e-7), "0.0000001");
        let tiny = t_float(2.5e-9);
        assert!(!tiny.contains('e'), "got: {tiny}");
        assert!(tiny.starts_with("0.0000000025"), "got: {tiny}");
    }
}
