//! Shared data types for the DAQ pipeline.
//!
//! These types are the hand-off points between the A2L parser, the configurator GUI, and the CanKing data processor (`canking-service`).
//! With the `serde` feature enabled they also define the JSON contract. See [`crate::config`].

use std::collections::HashMap;

// ------------------------------- A2L / XCP metadata ------------------------------------

/// Byte order for a signal: Big Endian (MSB first) or Little Endian (MSB last).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ByteOrder {
    MsbFirst,
    MsbLast,
}

/// Conversion method identifier for a signal.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Conversion {
    pub identifier: String,
}

/// A parsed signal from a MEASUREMENT block.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParsedSignal {
    pub name: String,
    pub long_identifier: String,
    pub conversion: Option<Conversion>,
    pub lower_limit: f64,
    pub upper_limit: f64,
    pub ecu_address: u64,
    pub size: u16,
    pub bit_mask: Option<u64>,
    pub byte_order: Option<ByteOrder>,
    pub is_signed: bool,
    pub is_float: bool,
    pub event: Option<String>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub virtual_inputs: Vec<String>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub virtual_conversion: Option<String>,
}

impl ParsedSignal {
    /// Physical display range as `(min, max)`, for DBC `[min|max]` and UI axes.
    ///
    /// Float signals keep their declared range.
    ///
    /// `bits` is the signal's width in bits.
    pub fn physical_limits(&self, bits: u32, factor: f64, offset: f64) -> (f64, f64) {
        let degenerate = !matches!(
            self.lower_limit.partial_cmp(&self.upper_limit),
            Some(std::cmp::Ordering::Less)
        );
        let filler = self.lower_limit == 0.0 && self.upper_limit == 255.0 && bits != 8;

        if self.is_float || !(degenerate || filler) {
            return (self.lower_limit, self.upper_limit);
        }

        let (raw_lo, raw_hi) = raw_range(bits, self.is_signed);
        let (a, b) = (raw_lo * factor + offset, raw_hi * factor + offset);
        if a <= b {
            (a, b)
        } else {
            (b, a)
        }
    }
}

/// Range representable by a `bits`-wide integer, as floats.
fn raw_range(bits: u32, is_signed: bool) -> (f64, f64) {
    let bits = bits.clamp(1, 64);
    if is_signed {
        let half = 2f64.powi(bits as i32 - 1);
        (-half, half - 1.0)
    } else {
        (0.0, 2f64.powi(bits as i32) - 1.0)
    }
}

/// One row of a TAB_VERB text table: raw values in `[lower, upper]` map to `text`.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VTabEntry {
    pub lower: f64,
    pub upper: f64,
    pub text: String,
}

/// Physical-value scaling extracted from a COMPU_METHOD block.
/// physical = factor * raw + offset
///
/// For TAB_VERB methods `text_table` holds the raw-value-to-text rows resolved
/// via COMPU_TAB_REF (factor/offset stay 1/0).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CompuMethod {
    pub factor: f64,
    pub offset: f64,
    pub unit: String,
    pub text_table: Vec<VTabEntry>,
    pub default_text: Option<String>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub formula: Option<String>,
}

/// How DAQ lists are configured in the slave, declared by the first keyword
/// after `/begin DAQ`.
///
/// - `Dynamic`: the master allocates DAQ lists/ODTs/entries at runtime (`FREE_DAQ`/`ALLOC_DAQ`/`ALLOC_ODT`/`ALLOC_ODT_ENTRY`).
/// - `Static`: DAQ lists and ODTs are permanently defined in the ECU; the master only fills predefined slots and must not send allocation commands.
/// - `StaticAndDynamic`: the slave supports both; this pipeline drives it as dynamic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DaqMode {
    #[default]
    Dynamic,
    Static,
    StaticAndDynamic,
}

impl DaqMode {
    pub fn is_static(self) -> bool {
        matches!(self, DaqMode::Static)
    }
}

/// Where a DAQ list's CAN identifier comes from.
///
/// XCP lets the A2L declare, per list, that it transmits on something other than the
/// transport's shared default id (`CAN_ID_SLAVE` / CCP `DTO_ID`): either a fixed id the
/// ECU always uses (`DAQ_LIST_CAN_ID ... FIXED <id>`, or CCP's `CAN_ID_FIXED`), or a
/// runtime-assignable one (`DAQ_LIST_CAN_ID ... VARIABLE`, XCP only) that the master picks
/// via `SET_DAQ_ID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "kind", rename_all = "snake_case"))]
pub enum CanIdSource {
    /// No override: the list transmits on the transport's shared default id, multiplexed by PID.
    #[default]
    Default,
    /// Always transmits on `id`; not user-overridable.
    Fixed { id: u32 },
    /// The ECU accepts `SET_DAQ_ID` to assign this list's CAN id at runtime. `assigned` is the id
    /// chosen in the configurator; `None` means none has been chosen, so no `SET_DAQ_ID` is sent
    /// and the list stays on the transport's shared default id, same as `Default`.
    Dynamic { assigned: Option<u32> },
}

impl CanIdSource {
    /// The CAN id this list actually transmits on right now, given the transport's shared default id.
    pub fn resolve(&self, default_id: u32) -> u32 {
        match *self {
            CanIdSource::Default => default_id,
            CanIdSource::Fixed { id } => id,
            CanIdSource::Dynamic { assigned } => assigned.unwrap_or(default_id),
        }
    }

    /// The id to send via `SET_DAQ_ID` for this list, if it needs one sent at all.
    pub fn set_daq_id(&self) -> Option<u32> {
        match *self {
            CanIdSource::Dynamic { assigned: Some(id) } => Some(id),
            _ => None,
        }
    }
}

impl From<Option<u32>> for CanIdSource {
    /// A bare "fixed id, or none" source, as CCP's `CAN_ID_FIXED` declares it: CCP has no
    /// runtime-assignable equivalent, so there is no `Dynamic` case here.
    fn from(id: Option<u32>) -> Self {
        match id {
            Some(id) => CanIdSource::Fixed { id },
            None => CanIdSource::Default,
        }
    }
}

/// Configuration of a single DAQ list as declared in a `/begin DAQ_LIST` block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DaqListDef {
    pub identifier: u16,
    pub nr_odt: u16,
    pub max_odt_entries: u16,
    pub event_fixed: Option<u16>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub first_pid: Option<u16>,
    /// This list's own CAN id, from an A2L `DAQ_LIST_CAN_ID` entry.
    #[cfg_attr(feature = "serde", serde(default))]
    pub can_id: CanIdSource,
}

/// Global DAQ configuration parsed from the `/begin DAQ` header block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DaqInfo {
    pub mode: DaqMode,
    pub daq_lists: Vec<DaqListDef>,
    pub max_odt_entry_size: u16,
    pub timestamp_size: u16,
    pub timestamp_fixed: bool,
}

/// A single event channel as declared in an `/begin EVENT` block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EventEntry {
    pub channel: u32,
    pub name: String,
    pub time_cycle: u32,
    pub time_unit: u32,
}

/// All event channel definitions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EventInfo {
    pub events: Vec<EventEntry>,
}

/// CAN-FD transport parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CanFdInfo {
    pub max_dlc: u16,
    pub data_baudrate: Option<u32>,
}

/// XCP-on-CAN transport layer configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct XcpInfo {
    pub version: u32,
    pub can_id_broadcast: Option<u32>,
    pub can_id_master: Option<u32>,
    pub can_id_slave: Option<u32>,
    pub baudrate: Option<u32>,
    pub can_fd: Option<CanFdInfo>,
}

/// XCP PROTOCOL_LAYER parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProtocolLayerInfo {
    pub byte_order: Option<ByteOrder>,
    pub max_cto: u16,
    pub max_dto: u16,
}

/// Top-level XCP configuration data.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct XcpData {
    pub daq_info: DaqInfo,
    pub event_info: EventInfo,
    pub xcp_info: Option<XcpInfo>,
    pub protocol_layer: Option<ProtocolLayerInfo>,
}

// ------------------------------- CCP metadata ------------------------------------------

/// CCP transport information
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CcpInfo {
    pub version: u16,
    pub cro_id: u32,
    pub dto_id: u32,
    pub station_address: u16,
    pub baudrate: Option<u32>,
    pub bytes_only: bool,
}

/// CCP DAQ source: one `/begin SOURCE` block in the A2L file, which may contain one or more rasters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CcpSource {
    pub name: String,
    pub daq_list: u16,
    pub rasters: Vec<u32>,
    pub first_pid: Option<u16>,
    pub length: Option<u16>,
    pub can_id: Option<u32>,
}

/// Top-level CCP configuration data
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CcpData {
    pub info: CcpInfo,
    pub rasters: Vec<EventEntry>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub sources: Vec<CcpSource>,
}

/// All data parsed from an A2L file.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct A2lProject {
    pub signals: Vec<ParsedSignal>,
    pub xcp_data: Option<XcpData>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub ccp_data: Option<CcpData>,
    #[cfg_attr(feature = "serde", serde(with = "compu_methods_serde"))]
    pub compu_methods: HashMap<String, CompuMethod>,
    pub default_byte_order: Option<ByteOrder>,
}

/// (De)serializes `compu_methods` as a sorted array of named entries.
#[cfg(feature = "serde")]
mod compu_methods_serde {
    use std::collections::HashMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{CompuMethod, VTabEntry};

    #[derive(Serialize, Deserialize)]
    struct Entry {
        name: String,
        factor: f64,
        offset: f64,
        unit: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        text_table: Vec<VTabEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_text: Option<String>,
    }

    pub fn serialize<S: Serializer>(
        map: &HashMap<String, CompuMethod>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut entries: Vec<Entry> = map
            .iter()
            .map(|(name, cm)| Entry {
                name: name.clone(),
                factor: cm.factor,
                offset: cm.offset,
                unit: cm.unit.clone(),
                text_table: cm.text_table.clone(),
                default_text: cm.default_text.clone(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<HashMap<String, CompuMethod>, D::Error> {
        let entries = Vec::<Entry>::deserialize(deserializer)?;
        Ok(entries
            .into_iter()
            .map(|e| {
                (
                    e.name,
                    CompuMethod {
                        factor: e.factor,
                        offset: e.offset,
                        unit: e.unit,
                        text_table: e.text_table,
                        default_text: e.default_text,
                        formula: None,
                    },
                )
            })
            .collect())
    }
}

// ------------------------------- Packed DAQ lists --------------------------------------

/// An ODT entry corresponds to a single measured variable mapping in DAQ.
///
/// An entry may cover more than one logical signal when adjacent signals are
/// merged into a contiguous block, or part of a logical signal when a large signal is split into chunks.
///
/// `names`: Logical signal name(s); multiple names appear when adjacent signals are merged into one block.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OdtEntry {
    pub names: Vec<String>,
    pub ecu_address: u64,
    pub size: u16,
}

impl OdtEntry {
    /// Combined label for the signal(s) in this entry.
    pub fn name(&self) -> String {
        self.names.join("/")
    }
}

/// An ordered collection of ODT entries.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Odt {
    pub entries: Vec<OdtEntry>,
}

/// A DAQ list contains one or more ODTs and is associated with an event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DaqList {
    pub id: u16,
    pub first_pid: u16,
    pub odts: Vec<Odt>,
    pub event: Option<String>,
    /// This list's own CAN id, distinct from the module's default DAQ CAN id.
    #[cfg_attr(feature = "serde", serde(default))]
    pub can_id: CanIdSource,
}
