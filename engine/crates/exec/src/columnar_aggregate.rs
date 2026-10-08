//! The columnar hash aggregate: keys as typed vectors, accumulators as flat
//! columns, one open-addressed index of slot ids. A batch is processed in
//! three passes — key words per row, slot per row with the index buckets
//! prefetched ahead, then one tight loop per aggregate over the batch —
//! so the per-row cost is a probe and a few stores, not an enum dispatch
//! and a pointer chase per accumulator.
//!
//! Covers every key type the exchange can carry (integers, dates, booleans,
//! text, dictionary-encoded text) and the accumulators that update in place:
//! COUNT, integer and floating SUM/AVG/MIN/MAX, text MIN/MAX. Distinct,
//! exact-decimal and decimal states stay on the row path.
use std::sync::Arc;

use ahash::RandomState;
use arrow::array::{
    Array, ArrayRef, AsArray, BinaryArray, BinaryBuilder, BooleanArray, Float64Array, Int32Array,
    Int32DictionaryArray, Int64Array, StringArray, StringViewArray, UInt64Array,
};
use arrow::datatypes::{DataType, Date32Type, Float64Type, Int32Type, Int64Type};
use hashbrown::{HashTable, hash_table::Entry};
use kaveon_core::{KaveonError, Result};

use crate::aggregate::compact_state;
use crate::aggregate::{
    AggExpr, AggFunc, AggregateState, AggregateValue, GroupKey, VALUE_BOOL, VALUE_INT32,
    VALUE_INT64, VALUE_NULL, VALUE_UTF8, exchanged_group_key_type, exec_err,
};

/// The most key columns a row packs into its hash; wider GROUP BYs take the
/// row path.
pub const MAX_KEYS: usize = 8;
/// Versioned compact key frame. Schema metadata supplies logical key types.
pub(crate) const COMPACT_KEY_MAGIC: &[u8; 3] = b"KP2";
/// Fixed-width non-null primitive key frame. Schema metadata supplies types.
pub(crate) const FIXED_KEY_MAGIC: &[u8; 3] = b"KF3";
pub(crate) const COMPACT_KEY_INT64: u8 = 1;
pub(crate) const COMPACT_KEY_INT32: u8 = 2;
pub(crate) const COMPACT_KEY_BOOL: u8 = 3;
pub(crate) const COMPACT_KEY_TEXT: u8 = 4;

/// One group key column at rest.
enum KeyColumn {
    /// Int64, Int32, Date32 and Boolean keys as their bits; `narrow`
    /// remembers the width the key leaves with.
    Integer {
        values: Vec<i64>,
        nulls: Vec<bool>,
        data_type: DataType,
    },
    /// Text keys as ids into the arena: equal text shares an id, so
    /// comparison is an integer compare.
    Text {
        words: Vec<u32>,
        nulls: Vec<bool>,
        arena: Arena,
        large: bool,
    },
}

/// Distinct strings, each stored once, addressable by id.
#[derive(Default)]
struct Arena {
    bytes: Vec<u8>,
    offsets: Vec<u32>,
    hashes: Vec<u64>,
    index: HashTable<u32>,
}

impl Arena {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            offsets: vec![0],
            hashes: Vec::new(),
            index: HashTable::new(),
        }
    }
    fn get(&self, id: u32) -> &str {
        let (start, end) = (
            self.offsets[id as usize] as usize,
            self.offsets[id as usize + 1] as usize,
        );
        // SAFETY: only `&str` bytes are appended.
        unsafe { std::str::from_utf8_unchecked(&self.bytes[start..end]) }
    }
    fn intern(&mut self, _hasher: &RandomState, text: &str) -> Result<(u32, bool)> {
        // Group keys are query-local. Equality is still checked byte-for-byte
        // by the HashTable probe, so a fast non-cryptographic hash is safe here
        // and avoids running the keyed hasher over every high-cardinality URL.
        // Keep the RandomState parameter for the legacy call sites and for the
        // security-sensitive paths outside this aggregate.
        let hash = text_hash(text.as_bytes());
        let bytes = &self.bytes;
        let offsets = &self.offsets;
        let id = u32::try_from(self.offsets.len() - 1)
            .map_err(|_| exec_err("too many distinct strings for one task"))?;
        match self.index.entry(
            hash,
            |&other| {
                let (start, end) = (
                    offsets[other as usize] as usize,
                    offsets[other as usize + 1] as usize,
                );
                &bytes[start..end] == text.as_bytes()
            },
            |&other| self.hashes[other as usize],
        ) {
            Entry::Occupied(entry) => Ok((*entry.get(), false)),
            Entry::Vacant(entry) => {
                self.bytes.extend_from_slice(text.as_bytes());
                let end = u32::try_from(self.bytes.len())
                    .map_err(|_| exec_err("string arena exceeds 4 GiB"))?;
                self.offsets.push(end);
                self.hashes.push(hash);
                entry.insert(id);
                Ok((id, true))
            }
        }
    }
    fn bytes(&self) -> u64 {
        (self.bytes.capacity() + self.offsets.capacity() * 4 + self.index.capacity() * 8) as u64
    }
    /// Forget every string, keeping the allocations.
    fn clear(&mut self) {
        self.bytes.clear();
        self.offsets.truncate(1);
        self.hashes.clear();
        self.index.clear();
    }
}

/// Fast query-local hash for variable-width group keys. The table always
/// verifies equality after a hash match, so this deliberately trades keyed
/// hashing for fewer instructions and sequential byte reads. The avalanche is
/// sufficient to keep the open-addressed arena index distributed even for URL
/// prefixes shared by many rows.
#[inline(always)]
fn text_hash(bytes: &[u8]) -> u64 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("sse4.2") {
        // CRC32C is used only to choose an open-addressing bucket; byte
        // equality remains the authority on collisions. On the benchmark's
        // x86 hosts this removes several scalar mixing instructions from the
        // high-cardinality URL path, while the scalar implementation below
        // remains the portable fallback.
        return unsafe { text_hash_crc32(bytes) };
    }
    text_hash_scalar(bytes)
}

#[inline(always)]
fn text_hash_scalar(bytes: &[u8]) -> u64 {
    let mut hash = 0x9e37_79b9_7f4a_7c15u64 ^ (bytes.len() as u64);
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        let mut word = u64::from_le_bytes(chunk.try_into().expect("chunks_exact is eight bytes"));
        word ^= word >> 30;
        word = word.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        word ^= word >> 27;
        hash ^= word;
        hash = hash.rotate_left(23).wrapping_mul(0x2127_599b_0f5d_6d3d);
    }
    let remainder = chunks.remainder();
    if !remainder.is_empty() {
        let mut word = 0u64;
        for (shift, byte) in remainder.iter().enumerate() {
            word |= (*byte as u64) << (shift * 8);
        }
        word ^= word >> 30;
        word = word.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        word ^= word >> 27;
        hash ^= word;
        hash = hash.rotate_left(23).wrapping_mul(0x2127_599b_0f5d_6d3d);
    }
    hash ^ (hash >> 29)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
unsafe fn text_hash_crc32(bytes: &[u8]) -> u64 {
    use std::arch::x86_64::{_mm_crc32_u8, _mm_crc32_u64};

    let mut hash = 0x9e37_79b9_7f4a_7c15u64 ^ bytes.len() as u64;
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes(chunk.try_into().expect("chunks_exact is eight bytes"));
        hash = _mm_crc32_u64(hash, word);
    }
    for &byte in chunks.remainder() {
        hash = _mm_crc32_u8(hash as u32, byte) as u64;
    }
    // Widen the 32-bit CRC into a well-mixed 64-bit bucket hash.
    let mut mixed = hash ^ (hash >> 29);
    mixed = mixed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    mixed ^ (mixed >> 32)
}

#[cfg(target_arch = "x86")]
#[target_feature(enable = "sse4.2")]
unsafe fn text_hash_crc32(bytes: &[u8]) -> u64 {
    use std::arch::x86::{_mm_crc32_u8, _mm_crc32_u32};

    let mut hash = 0x9e37_79b9u32 ^ bytes.len() as u32;
    let mut chunks = bytes.chunks_exact(4);
    for chunk in &mut chunks {
        let word = u32::from_le_bytes(chunk.try_into().expect("chunks_exact is four bytes"));
        hash = _mm_crc32_u32(hash, word);
    }
    for &byte in chunks.remainder() {
        hash = _mm_crc32_u8(hash, byte);
    }
    let mixed = (hash as u64) ^ ((hash as u64) << 32);
    mixed ^ (mixed >> 29)
}

/// One aggregate's accumulators for every slot.
enum AccColumn {
    Count(Vec<u64>),
    IntegerSum {
        sums: Vec<i128>,
        counts: Vec<u64>,
    },
    IntegerMin {
        values: Vec<i64>,
        present: Vec<bool>,
    },
    IntegerMax {
        values: Vec<i64>,
        present: Vec<bool>,
    },
    /// SUM and AVG over floats (and AVG over integers) share the layout.
    Float {
        sums: Vec<f64>,
        counts: Vec<u64>,
        avg: bool,
    },
    FloatMin {
        values: Vec<f64>,
        present: Vec<bool>,
    },
    FloatMax {
        values: Vec<f64>,
        present: Vec<bool>,
    },
    TextMin(Vec<Option<Box<str>>>),
    TextMax(Vec<Option<Box<str>>>),
}

impl AccColumn {
    fn for_state(state: &AggregateState) -> Option<Self> {
        Some(match state {
            AggregateState::Count(_) => Self::Count(Vec::new()),
            AggregateState::IntegerSum { .. } => Self::IntegerSum {
                sums: Vec::new(),
                counts: Vec::new(),
            },
            AggregateState::IntegerMin(_) => Self::IntegerMin {
                values: Vec::new(),
                present: Vec::new(),
            },
            AggregateState::IntegerMax(_) => Self::IntegerMax {
                values: Vec::new(),
                present: Vec::new(),
            },
            AggregateState::Sum { .. } => Self::Float {
                sums: Vec::new(),
                counts: Vec::new(),
                avg: false,
            },
            AggregateState::Avg { .. } => Self::Float {
                sums: Vec::new(),
                counts: Vec::new(),
                avg: true,
            },
            AggregateState::Min(_) => Self::FloatMin {
                values: Vec::new(),
                present: Vec::new(),
            },
            AggregateState::Max(_) => Self::FloatMax {
                values: Vec::new(),
                present: Vec::new(),
            },
            AggregateState::Utf8Min(_) => Self::TextMin(Vec::new()),
            AggregateState::Utf8Max(_) => Self::TextMax(Vec::new()),
            _ => return None,
        })
    }
    fn push_identity(&mut self) {
        match self {
            Self::Count(counts) => counts.push(0),
            Self::IntegerSum { sums, counts } => {
                sums.push(0);
                counts.push(0);
            }
            Self::IntegerMin { values, present } | Self::IntegerMax { values, present } => {
                values.push(0);
                present.push(false);
            }
            Self::Float { sums, counts, .. } => {
                sums.push(0.0);
                counts.push(0);
            }
            Self::FloatMin { values, present } | Self::FloatMax { values, present } => {
                values.push(0.0);
                present.push(false);
            }
            Self::TextMin(values) | Self::TextMax(values) => values.push(None),
        }
    }
    /// Bytes one slot occupies in this column.
    fn slot_bytes(&self) -> u64 {
        match self {
            Self::Count(_) => 8,
            Self::IntegerSum { .. } => 24,
            Self::IntegerMin { .. } | Self::IntegerMax { .. } => 9,
            Self::Float { .. } => 16,
            Self::FloatMin { .. } | Self::FloatMax { .. } => 9,
            Self::TextMin(_) | Self::TextMax(_) => 16,
        }
    }

    /// Drop every slot, keeping the allocations.
    fn clear(&mut self) {
        match self {
            Self::Count(counts) => counts.clear(),
            Self::IntegerSum { sums, counts } => {
                sums.clear();
                counts.clear();
            }
            Self::IntegerMin { values, present } | Self::IntegerMax { values, present } => {
                values.clear();
                present.clear();
            }
            Self::Float { sums, counts, .. } => {
                sums.clear();
                counts.clear();
            }
            Self::FloatMin { values, present } | Self::FloatMax { values, present } => {
                values.clear();
                present.clear();
            }
            Self::TextMin(values) | Self::TextMax(values) => values.clear(),
        }
    }

    /// Apply one batch column to the slots in `slots` (one per row).
    fn update(&mut self, column: Option<&ArrayRef>, slots: &[u32]) -> Result<()> {
        match (self, column) {
            (Self::Count(counts), None) => {
                for &slot in slots {
                    counts[slot as usize] += 1;
                }
            }
            (Self::Count(counts), Some(array)) => match array.nulls() {
                None => {
                    for &slot in slots {
                        counts[slot as usize] += 1;
                    }
                }
                Some(nulls) => {
                    for (row, &slot) in slots.iter().enumerate() {
                        if nulls.is_valid(row) {
                            counts[slot as usize] += 1;
                        }
                    }
                }
            },
            (Self::IntegerSum { sums, counts }, Some(array)) => {
                for_each_integer(array, slots, |slot, value| {
                    sums[slot] += value as i128;
                    counts[slot] += 1;
                })?;
            }
            (Self::IntegerMin { values, present }, Some(array)) => {
                for_each_integer(array, slots, |slot, value| {
                    if !present[slot] || value < values[slot] {
                        values[slot] = value;
                        present[slot] = true;
                    }
                })?;
            }
            (Self::IntegerMax { values, present }, Some(array)) => {
                for_each_integer(array, slots, |slot, value| {
                    if !present[slot] || value > values[slot] {
                        values[slot] = value;
                        present[slot] = true;
                    }
                })?;
            }
            (Self::Float { sums, counts, .. }, Some(array)) => {
                for_each_float(array, slots, |slot, value| {
                    sums[slot] += value;
                    counts[slot] += 1;
                })?;
            }
            (Self::FloatMin { values, present }, Some(array)) => {
                for_each_float(array, slots, |slot, value| {
                    if !present[slot] || value < values[slot] {
                        values[slot] = value;
                        present[slot] = true;
                    }
                })?;
            }
            (Self::FloatMax { values, present }, Some(array)) => {
                for_each_float(array, slots, |slot, value| {
                    if !present[slot] || value > values[slot] {
                        values[slot] = value;
                        present[slot] = true;
                    }
                })?;
            }
            (Self::TextMin(values), Some(array)) => {
                for_each_text(array, slots, |slot, text| {
                    if values[slot].as_deref().is_none_or(|old| text < old) {
                        values[slot] = Some(Box::from(text));
                    }
                })?;
            }
            (Self::TextMax(values), Some(array)) => {
                for_each_text(array, slots, |slot, text| {
                    if values[slot].as_deref().is_none_or(|old| text > old) {
                        values[slot] = Some(Box::from(text));
                    }
                })?;
            }
            _ => return Err(exec_err("aggregate input column missing")),
        }
        Ok(())
    }

    /// Merge one decoded partial state into a slot.
    fn merge(&mut self, slot: usize, state: &AggregateState) -> Result<()> {
        match (self, state) {
            (Self::Count(counts), AggregateState::Count(count)) => {
                counts[slot] = counts[slot]
                    .checked_add(*count)
                    .ok_or_else(|| exec_err("COUNT overflow"))?;
            }
            (Self::IntegerSum { sums, counts }, AggregateState::IntegerSum { sum, count }) => {
                sums[slot] = sums[slot]
                    .checked_add(*sum)
                    .ok_or_else(|| exec_err("integer SUM overflow"))?;
                counts[slot] += count;
            }
            (Self::IntegerMin { values, present }, AggregateState::IntegerMin(value)) => {
                if let Some(value) = value
                    && (!present[slot] || *value < values[slot])
                {
                    values[slot] = *value;
                    present[slot] = true;
                }
            }
            (Self::IntegerMax { values, present }, AggregateState::IntegerMax(value)) => {
                if let Some(value) = value
                    && (!present[slot] || *value > values[slot])
                {
                    values[slot] = *value;
                    present[slot] = true;
                }
            }
            (
                Self::Float { sums, counts, .. },
                AggregateState::Sum { sum, count } | AggregateState::Avg { sum, count },
            ) => {
                sums[slot] += sum;
                counts[slot] += count;
            }
            (Self::FloatMin { values, present }, AggregateState::Min(value)) => {
                if let Some(value) = value
                    && (!present[slot] || *value < values[slot])
                {
                    values[slot] = *value;
                    present[slot] = true;
                }
            }
            (Self::FloatMax { values, present }, AggregateState::Max(value)) => {
                if let Some(value) = value
                    && (!present[slot] || *value > values[slot])
                {
                    values[slot] = *value;
                    present[slot] = true;
                }
            }
            (Self::TextMin(values), AggregateState::Utf8Min(Some(text))) => {
                if values[slot]
                    .as_deref()
                    .is_none_or(|old| text.as_str() < old)
                {
                    values[slot] = Some(Box::from(text.as_str()));
                }
            }
            (Self::TextMax(values), AggregateState::Utf8Max(Some(text))) => {
                if values[slot]
                    .as_deref()
                    .is_none_or(|old| text.as_str() > old)
                {
                    values[slot] = Some(Box::from(text.as_str()));
                }
            }
            (Self::TextMin(_), AggregateState::Utf8Min(None))
            | (Self::TextMax(_), AggregateState::Utf8Max(None)) => {}
            _ => return Err(exec_err("aggregate state layout mismatch in merge")),
        }
        Ok(())
    }

    /// Merge one compact-encoded partial state, read straight from
    /// `input`, into a slot: the tag must be this column's.
    #[inline]
    fn merge_bytes(&mut self, slot: usize, input: &mut compact_state::Input<'_>) -> Result<()> {
        use compact_state::{
            TAG_AVG, TAG_COUNT, TAG_INTEGER_MAX, TAG_INTEGER_MIN, TAG_INTEGER_SUM, TAG_MAX,
            TAG_MIN, TAG_SUM, TAG_UTF8_MAX, TAG_UTF8_MIN,
        };
        let tag = input.byte()?;
        match self {
            Self::Count(counts) if tag == TAG_COUNT => {
                let count = u64::from_le_bytes(input.fixed()?);
                counts[slot] = counts[slot]
                    .checked_add(count)
                    .ok_or_else(|| exec_err("COUNT overflow"))?;
            }
            Self::IntegerSum { sums, counts } if tag == TAG_INTEGER_SUM => {
                let sum = i128::from_le_bytes(input.fixed()?);
                let count = u64::from_le_bytes(input.fixed()?);
                sums[slot] = sums[slot]
                    .checked_add(sum)
                    .ok_or_else(|| exec_err("integer SUM overflow"))?;
                counts[slot] += count;
            }
            Self::IntegerMin { values, present } if tag == TAG_INTEGER_MIN => {
                if input.flag()? {
                    let value = i64::from_le_bytes(input.fixed()?);
                    if !present[slot] || value < values[slot] {
                        values[slot] = value;
                        present[slot] = true;
                    }
                }
            }
            Self::IntegerMax { values, present } if tag == TAG_INTEGER_MAX => {
                if input.flag()? {
                    let value = i64::from_le_bytes(input.fixed()?);
                    if !present[slot] || value > values[slot] {
                        values[slot] = value;
                        present[slot] = true;
                    }
                }
            }
            Self::Float { sums, counts, .. } if tag == TAG_SUM || tag == TAG_AVG => {
                let sum = f64::from_le_bytes(input.fixed()?);
                let count = u64::from_le_bytes(input.fixed()?);
                sums[slot] += sum;
                counts[slot] += count;
            }
            Self::FloatMin { values, present } if tag == TAG_MIN => {
                if input.flag()? {
                    let value = f64::from_le_bytes(input.fixed()?);
                    if !present[slot] || value < values[slot] {
                        values[slot] = value;
                        present[slot] = true;
                    }
                }
            }
            Self::FloatMax { values, present } if tag == TAG_MAX => {
                if input.flag()? {
                    let value = f64::from_le_bytes(input.fixed()?);
                    if !present[slot] || value > values[slot] {
                        values[slot] = value;
                        present[slot] = true;
                    }
                }
            }
            Self::TextMin(values) if tag == TAG_UTF8_MIN => {
                if input.flag()? {
                    let text = compact_state::utf8_extremum(input.payload()?)?;
                    if values[slot].as_deref().is_none_or(|old| text < old) {
                        values[slot] = Some(Box::from(text));
                    }
                }
            }
            Self::TextMax(values) if tag == TAG_UTF8_MAX => {
                if input.flag()? {
                    let text = compact_state::utf8_extremum(input.payload()?)?;
                    if values[slot].as_deref().is_none_or(|old| text > old) {
                        values[slot] = Some(Box::from(text));
                    }
                }
            }
            _ => return Err(exec_err("aggregate state layout mismatch in merge")),
        }
        Ok(())
    }

    /// The slot's state as the enum, for the encoders and outputs that
    /// speak it.
    fn state(&self, slot: usize) -> AggregateState {
        match self {
            Self::Count(counts) => AggregateState::Count(counts[slot]),
            Self::IntegerSum { sums, counts } => AggregateState::IntegerSum {
                sum: sums[slot],
                count: counts[slot],
            },
            Self::IntegerMin { values, present } => {
                AggregateState::IntegerMin(present[slot].then_some(values[slot]))
            }
            Self::IntegerMax { values, present } => {
                AggregateState::IntegerMax(present[slot].then_some(values[slot]))
            }
            Self::Float { sums, counts, avg } => {
                if *avg {
                    AggregateState::Avg {
                        sum: sums[slot],
                        count: counts[slot],
                    }
                } else {
                    AggregateState::Sum {
                        sum: sums[slot],
                        count: counts[slot],
                    }
                }
            }
            Self::FloatMin { values, present } => {
                AggregateState::Min(present[slot].then_some(values[slot]))
            }
            Self::FloatMax { values, present } => {
                AggregateState::Max(present[slot].then_some(values[slot]))
            }
            Self::TextMin(values) => {
                AggregateState::Utf8Min(values[slot].as_deref().map(str::to_owned))
            }
            Self::TextMax(values) => {
                AggregateState::Utf8Max(values[slot].as_deref().map(str::to_owned))
            }
        }
    }
}

fn for_each_integer(
    array: &ArrayRef,
    slots: &[u32],
    mut apply: impl FnMut(usize, i64),
) -> Result<()> {
    match array.data_type() {
        DataType::Int64 => {
            let values = array.as_primitive::<Int64Type>();
            match values.nulls() {
                None => {
                    for (&slot, &value) in slots.iter().zip(values.values().iter()) {
                        apply(slot as usize, value);
                    }
                }
                Some(nulls) => {
                    for (row, (&slot, &value)) in
                        slots.iter().zip(values.values().iter()).enumerate()
                    {
                        if nulls.is_valid(row) {
                            apply(slot as usize, value);
                        }
                    }
                }
            }
        }
        DataType::Int32 => {
            let values = array.as_primitive::<Int32Type>();
            match values.nulls() {
                None => {
                    for (&slot, &value) in slots.iter().zip(values.values().iter()) {
                        apply(slot as usize, i64::from(value));
                    }
                }
                Some(nulls) => {
                    for (row, (&slot, &value)) in
                        slots.iter().zip(values.values().iter()).enumerate()
                    {
                        if nulls.is_valid(row) {
                            apply(slot as usize, i64::from(value));
                        }
                    }
                }
            }
        }
        other => return Err(exec_err(format!("integer aggregate over {other}"))),
    }
    Ok(())
}

fn for_each_float(
    array: &ArrayRef,
    slots: &[u32],
    mut apply: impl FnMut(usize, f64),
) -> Result<()> {
    match array.data_type() {
        DataType::Float64 => {
            let values = array.as_primitive::<Float64Type>();
            match values.nulls() {
                None => {
                    for (&slot, &value) in slots.iter().zip(values.values().iter()) {
                        apply(slot as usize, value);
                    }
                }
                Some(nulls) => {
                    for (row, (&slot, &value)) in
                        slots.iter().zip(values.values().iter()).enumerate()
                    {
                        if nulls.is_valid(row) {
                            apply(slot as usize, value);
                        }
                    }
                }
            }
            Ok(())
        }
        DataType::Int64 | DataType::Int32 => {
            for_each_integer(array, slots, |slot, value| apply(slot, value as f64))
        }
        other => Err(exec_err(format!("numeric aggregate over {other}"))),
    }
}

fn for_each_text<'a>(
    array: &'a ArrayRef,
    slots: &[u32],
    mut apply: impl FnMut(usize, &'a str),
) -> Result<()> {
    match array.data_type() {
        DataType::Utf8 => {
            let values = array.as_string::<i32>();
            for (row, &slot) in slots.iter().enumerate() {
                if !values.is_null(row) {
                    apply(slot as usize, values.value(row));
                }
            }
        }
        DataType::LargeUtf8 => {
            let values = array.as_string::<i64>();
            for (row, &slot) in slots.iter().enumerate() {
                if !values.is_null(row) {
                    apply(slot as usize, values.value(row));
                }
            }
        }
        DataType::Dictionary(_, _) => {
            let dictionary = array
                .as_any()
                .downcast_ref::<Int32DictionaryArray>()
                .expect("dictionary key type must match schema");
            let keys = dictionary.keys();
            match dictionary.values().data_type() {
                DataType::Utf8 => {
                    let values = dictionary.values().as_string::<i32>();
                    for (row, &slot) in slots.iter().enumerate() {
                        if keys.is_valid(row) {
                            let code = keys.value(row) as usize;
                            if !values.is_null(code) {
                                apply(slot as usize, values.value(code));
                            }
                        }
                    }
                }
                DataType::LargeUtf8 => {
                    let values = dictionary.values().as_string::<i64>();
                    for (row, &slot) in slots.iter().enumerate() {
                        if keys.is_valid(row) {
                            let code = keys.value(row) as usize;
                            if !values.is_null(code) {
                                apply(slot as usize, values.value(code));
                            }
                        }
                    }
                }
                other => {
                    return Err(exec_err(format!(
                        "text aggregate over dictionary of {other}"
                    )));
                }
            }
        }
        other => return Err(exec_err(format!("text aggregate over {other}"))),
    }
    Ok(())
}

/// Whether the columnar path carries this key column type.
pub fn supports_key(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Int64
            | DataType::Int32
            | DataType::Date32
            | DataType::Boolean
            | DataType::Utf8
            | DataType::LargeUtf8
            | DataType::Utf8View
    ) || matches!(data_type, DataType::Dictionary(key, values)
        if key.as_ref() == &DataType::Int32 && matches!(values.as_ref(), DataType::Utf8 | DataType::LargeUtf8))
}

/// Whether the columnar path carries this aggregate over this input type.
pub fn supports_aggregate(
    aggregate: &AggExpr,
    input: Option<&DataType>,
    state: &AggregateState,
) -> bool {
    if aggregate.distinct || AccColumn::for_state(state).is_none() {
        return false;
    }
    match input {
        None => matches!(aggregate.func, AggFunc::Count),
        Some(DataType::Int64 | DataType::Int32 | DataType::Float64) => !matches!(
            state,
            AggregateState::Utf8Min(_) | AggregateState::Utf8Max(_)
        ),
        // Dates fold as their day numbers; the caller hands them in as Int32.
        Some(DataType::Date32) => matches!(
            state,
            AggregateState::IntegerMin(_)
                | AggregateState::IntegerMax(_)
                | AggregateState::Count(_)
        ),
        Some(DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View) => {
            matches!(aggregate.func, AggFunc::Min | AggFunc::Max | AggFunc::Count)
        }
        Some(DataType::Dictionary(key, values)) => {
            key.as_ref() == &DataType::Int32
                && matches!(values.as_ref(), DataType::Utf8 | DataType::LargeUtf8)
                && matches!(aggregate.func, AggFunc::Min | AggFunc::Max | AggFunc::Count)
        }
        _ => false,
    }
}

/// Words per row for one key column of a batch, written into the row-major
/// scratch at `position` of each row's `stride` words: the key's bits, or
/// an arena id for text. A null writes `u64::MAX` and sets its bit in the
/// row's last word.
fn key_words(
    column: &mut KeyColumn,
    hasher: &RandomState,
    array: &ArrayRef,
    packed: &mut [u64],
    stride: usize,
    position: usize,
    new_bytes: &mut u64,
) -> Result<()> {
    let rows = array.len();
    let null_at = stride - 1;
    let mut write = |row: usize, word: Option<u64>| {
        let base = row * stride;
        match word {
            Some(word) => packed[base + position] = word,
            None => {
                packed[base + position] = u64::MAX;
                packed[base + null_at] |= 1 << position;
            }
        }
    };
    match column {
        KeyColumn::Integer { .. } => match array.data_type() {
            DataType::Int64 => {
                let values = array.as_primitive::<Int64Type>();
                for (row, value) in values.values().iter().enumerate() {
                    write(row, (!values.is_null(row)).then_some(*value as u64));
                }
            }
            DataType::Int32 => {
                let values = array.as_primitive::<Int32Type>();
                for (row, value) in values.values().iter().enumerate() {
                    write(row, (!values.is_null(row)).then_some(*value as i64 as u64));
                }
            }
            DataType::Date32 => {
                let values = array.as_primitive::<Date32Type>();
                for (row, value) in values.values().iter().enumerate() {
                    write(row, (!values.is_null(row)).then_some(*value as i64 as u64));
                }
            }
            DataType::Boolean => {
                let values = array.as_boolean();
                for row in 0..rows {
                    write(
                        row,
                        (!values.is_null(row)).then_some(values.value(row) as u64),
                    );
                }
            }
            other => return Err(exec_err(format!("group key column type changed: {other}"))),
        },
        KeyColumn::Text { arena, .. } => match array.data_type() {
            DataType::Utf8 | DataType::LargeUtf8 => {
                let before = arena.bytes();
                let mut intern =
                    |text: &str| -> Result<u64> { Ok(arena.intern(hasher, text)?.0 as u64) };
                if array.data_type() == &DataType::Utf8 {
                    let values = array.as_string::<i32>();
                    for row in 0..rows {
                        if values.is_null(row) {
                            write(row, None);
                        } else {
                            write(row, Some(intern(values.value(row))?));
                        }
                    }
                } else {
                    let values = array.as_string::<i64>();
                    for row in 0..rows {
                        if values.is_null(row) {
                            write(row, None);
                        } else {
                            write(row, Some(intern(values.value(row))?));
                        }
                    }
                }
                *new_bytes += arena.bytes().saturating_sub(before);
            }
            DataType::Utf8View => {
                let before = arena.bytes();
                let values = array
                    .as_any()
                    .downcast_ref::<StringViewArray>()
                    .ok_or_else(|| exec_err("UTF-8 view key array has an invalid type"))?;
                for row in 0..rows {
                    if values.is_null(row) {
                        write(row, None);
                    } else {
                        write(row, Some(arena.intern(hasher, values.value(row))?.0 as u64));
                    }
                }
                *new_bytes += arena.bytes().saturating_sub(before);
            }
            DataType::Dictionary(_, _) => {
                let dictionary = array
                    .as_any()
                    .downcast_ref::<Int32DictionaryArray>()
                    .expect("dictionary key type must match schema");
                let before = arena.bytes();
                // One arena lookup per dictionary value the batch uses.
                let values = dictionary.values();
                let mut by_code: Vec<Option<u64>> = vec![None; values.len()];
                let keys = dictionary.keys();
                for row in 0..rows {
                    if keys.is_null(row) {
                        write(row, None);
                        continue;
                    }
                    let code = keys.value(row) as usize;
                    let word = match by_code[code] {
                        Some(word) => word,
                        None => {
                            let text = match values.data_type() {
                                DataType::Utf8 => {
                                    let strings = values.as_string::<i32>();
                                    (!strings.is_null(code)).then(|| strings.value(code))
                                }
                                DataType::LargeUtf8 => {
                                    let strings = values.as_string::<i64>();
                                    (!strings.is_null(code)).then(|| strings.value(code))
                                }
                                _ => None,
                            };
                            match text {
                                Some(text) => {
                                    let word = arena.intern(hasher, text)?.0 as u64;
                                    by_code[code] = Some(word);
                                    word
                                }
                                None => {
                                    write(row, None);
                                    continue;
                                }
                            }
                        }
                    };
                    write(row, Some(word));
                }
                *new_bytes += arena.bytes().saturating_sub(before);
            }
            other => return Err(exec_err(format!("group key column type changed: {other}"))),
        },
    }
    Ok(())
}

/// The slot index: open addressing with linear probing over a tag byte and
/// a slot per bucket. The tag is seven bits of the hash with the high bit
/// set (zero marks an empty bucket), so a probe reads the key columns only
/// on a tag match; the buckets a batch will probe are prefetched a few
/// rows ahead, so their cache misses overlap instead of serialising. A
/// doubling re-derives every hash from the columns in slot order —
/// sequential reads — where a bucket-order rehash reads each column at a
/// random slot per group.
struct SlotIndex {
    tags: Vec<u8>,
    slots: Vec<u32>,
    len: usize,
}

/// Buckets per group before a doubling: the index doubles at three
/// quarters full, which keeps a linear probe within a cache line or two.
const INDEX_LOAD_NUMERATOR: usize = 3;
const INDEX_LOAD_DENOMINATOR: usize = 4;
const INDEX_MIN_BUCKETS: usize = 16;
/// Large high-cardinality aggregates spend substantial time rehashing while
/// an index doubles. Grow those tables by four once they are already large;
/// small tables retain the lower-memory doubling behavior.
const INDEX_LARGE_GROWTH_THRESHOLD: usize = 1 << 16;
/// Rows between a bucket's prefetch and its probe.
const PREFETCH_DISTANCE: usize = 32;

/// Where a probe ended: the slot whose key matched, or the empty bucket a
/// new slot takes.
enum Probe {
    Found(u32),
    Vacant(usize),
}

impl SlotIndex {
    fn new() -> Self {
        Self {
            tags: Vec::new(),
            slots: Vec::new(),
            len: 0,
        }
    }

    /// Groups the index holds before it doubles.
    fn capacity(&self) -> usize {
        self.tags.len() / INDEX_LOAD_DENOMINATOR * INDEX_LOAD_NUMERATOR
    }

    /// Empty every bucket, keeping the buckets.
    fn clear(&mut self) {
        self.tags.fill(0);
        self.len = 0;
    }

    fn is_full(&self) -> bool {
        self.len >= self.capacity()
    }

    #[inline]
    fn tag(hash: u64) -> u8 {
        (hash >> 57) as u8 | 0x80
    }

    #[inline]
    fn prefetch(&self, hash: u64) {
        if !self.tags.is_empty() {
            let index = hash as usize & (self.tags.len() - 1);
            prefetch_read(self.tags.as_ptr().wrapping_add(index));
            prefetch_read(self.slots.as_ptr().wrapping_add(index));
        }
    }

    /// Probe for `hash`. The index must not be full.
    #[inline]
    fn probe(&self, hash: u64, mut matches: impl FnMut(u32) -> bool) -> Probe {
        debug_assert!(!self.is_full());
        let tag = Self::tag(hash);
        let mask = self.tags.len() - 1;
        let mut index = hash as usize & mask;
        loop {
            let found = self.tags[index];
            if found == 0 {
                return Probe::Vacant(index);
            }
            if found == tag {
                let slot = self.slots[index];
                if matches(slot) {
                    return Probe::Found(slot);
                }
            }
            index = (index + 1) & mask;
        }
    }

    /// Fill the vacant bucket a probe returned.
    #[inline]
    fn occupy(&mut self, index: usize, hash: u64, slot: u32) {
        debug_assert_eq!(self.tags[index], 0);
        self.tags[index] = Self::tag(hash);
        self.slots[index] = slot;
        self.len += 1;
    }

    /// Double the buckets and place every slot again from `hash_of`,
    /// called in slot order.
    fn grow(&mut self, large_integer_growth: bool, mut hash_of: impl FnMut(u32) -> u64) {
        let multiplier = if large_integer_growth && self.tags.len() >= INDEX_LARGE_GROWTH_THRESHOLD
        {
            4
        } else {
            2
        };
        let buckets = (self.tags.len() * multiplier).max(INDEX_MIN_BUCKETS);
        let mask = buckets - 1;
        let mut tags = vec![0u8; buckets];
        let mut slots = vec![0u32; buckets];
        let mut hashes = [0u64; 64];
        let mut slot = 0u32;
        while (slot as usize) < self.len {
            let block = ((self.len - slot as usize).min(hashes.len())) as u32;
            for (offset, hash) in hashes[..block as usize].iter_mut().enumerate() {
                *hash = hash_of(slot + offset as u32);
                prefetch_read(tags.as_ptr().wrapping_add(*hash as usize & mask));
                prefetch_read(slots.as_ptr().wrapping_add(*hash as usize & mask));
            }
            for (offset, hash) in hashes[..block as usize].iter().enumerate() {
                let mut index = *hash as usize & mask;
                while tags[index] != 0 {
                    index = (index + 1) & mask;
                }
                tags[index] = Self::tag(*hash);
                slots[index] = slot + offset as u32;
            }
            slot += block;
        }
        self.tags = tags;
        self.slots = slots;
    }
}

/// Ask for the cache line at `pointer` ahead of its use. A hint only:
/// nothing is read, and targets without the instruction skip it.
#[inline(always)]
fn prefetch_read<T>(pointer: *const T) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: prefetch is a hint that never faults, whatever the address.
    unsafe {
        std::arch::x86_64::_mm_prefetch::<{ std::arch::x86_64::_MM_HINT_T0 }>(pointer as *const i8);
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = pointer;
    }
}

/// Index bytes charged per group: five bytes a bucket (tag and slot) at
/// the lowest occupancy a doubling leaves, three eighths.
const INDEX_BUCKET_BYTES_PER_SLOT: u64 = 16;

/// The hash of the group at `slot`, from the columns: the same words the
/// batch hashed when the group was created.
fn slot_hash(keys: &[KeyColumn], slot: usize) -> u64 {
    let key_count = keys.len();
    let mut packed = [0u64; MAX_KEYS + 1];
    let mut null_bits = 0u64;
    for (position, key) in keys.iter().enumerate() {
        let (word, null) = match key {
            KeyColumn::Integer { values, nulls, .. } => (values[slot] as u64, nulls[slot]),
            KeyColumn::Text { words, nulls, .. } => (words[slot] as u64, nulls[slot]),
        };
        packed[position] = if null { u64::MAX } else { word };
        if null {
            null_bits |= 1 << position;
        }
    }
    packed[key_count] = null_bits;
    // Group keys are query-local and equality is checked after probing.  A
    // keyed hash is useful for untrusted map keys, but it is needlessly
    // expensive in this hot path (the same words are hashed for every input
    // row and again while growing the table).  Keep the keyed hasher for the
    // string arena; use the compact, high-diffusion hash for the packed key.
    packed_hash(&packed[..=key_count])
}

/// Fast hash for query-local packed group keys.
///
/// This is a SplitMix-style finalizer over all key words.  It is deliberately
/// independent of process-randomized hash state: a collision only adds a
/// probe because `key_matches` always verifies the complete key.  Keeping the
/// function inline removes the `Hasher` trait dispatch and slice iteration
/// overhead from high-cardinality GROUP BYs while retaining good avalanche
/// behaviour for integer and interned-text keys.
#[inline(always)]
fn packed_hash(words: &[u64]) -> u64 {
    let mut hash = 0x9e37_79b9_7f4a_7c15u64 ^ (words.len() as u64);
    for &word in words {
        let mut value = word.wrapping_add(0x9e37_79b9_7f4a_7c15);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 31;
        hash ^= value;
        hash = hash
            .rotate_left(27)
            .wrapping_mul(5)
            .wrapping_add(0x52dce729);
    }
    hash ^ (hash >> 29)
}

/// Whether the group at `slot` has the packed key `words` (the key words,
/// then the null bits).
#[inline]
fn key_matches(keys: &[KeyColumn], slot: usize, words: &[u64]) -> bool {
    let null_bits = words[keys.len()];
    keys.iter().enumerate().all(|(position, key)| {
        let null = null_bits & (1 << position) != 0;
        match key {
            KeyColumn::Integer { values, nulls, .. } => {
                nulls[slot] == null && (null || values[slot] as u64 == words[position])
            }
            KeyColumn::Text {
                words: stored,
                nulls,
                ..
            } => nulls[slot] == null && (null || stored[slot] as u64 == words[position]),
        }
    })
}

/// Parse one exchange-encoded group key (a count, then per key a length,
/// a tag and the value) into packed `words`: the key words then the null
/// bits. Text is interned; returns the arena bytes that added.
fn parse_key_words(
    keys: &mut [KeyColumn],
    hasher: &RandomState,
    key: &[u8],
    words: &mut [u64],
) -> Result<u64> {
    let key_count = keys.len();
    if key.starts_with(FIXED_KEY_MAGIC) {
        let mut input = KeyInput(&key[FIXED_KEY_MAGIC.len()..]);
        if input.byte()? as usize != key_count {
            return Err(exec_err("fixed partial group key count mismatch"));
        }
        let tags = (0..key_count).map(|_| input.byte()).collect::<Result<Vec<_>>>()?;
        for (position, column) in keys.iter().enumerate() {
            words[position] = match (column, tags[position]) {
                (KeyColumn::Integer { data_type: DataType::Int64, .. }, COMPACT_KEY_INT64) => {
                    i64::from_le_bytes(input.take(8)?.try_into().unwrap()) as u64
                }
                (KeyColumn::Integer { data_type: DataType::Int32 | DataType::Date32, .. }, COMPACT_KEY_INT32) => {
                    i32::from_le_bytes(input.take(4)?.try_into().unwrap()) as i64 as u64
                }
                (KeyColumn::Integer { data_type: DataType::Boolean, .. }, COMPACT_KEY_BOOL) => match input.byte()? {
                    0 => 0,
                    1 => 1,
                    _ => return Err(exec_err("invalid fixed boolean group key")),
                },
                _ => return Err(exec_err("fixed group key type mismatch")),
            };
        }
        if !input.0.is_empty() {
            return Err(exec_err("trailing fixed partial group key bytes"));
        }
        words[key_count] = 0;
        return Ok(0);
    }
    if key.starts_with(COMPACT_KEY_MAGIC) {
        let mut input = KeyInput(&key[COMPACT_KEY_MAGIC.len()..]);
        if input.byte()? as usize != key_count {
            return Err(exec_err("compact partial group key count mismatch"));
        }
        let mut null_bits = 0u64;
        let mut new_bytes = 0u64;
        for (position, column) in keys.iter_mut().enumerate() {
            let tag = input.byte()?;
            let present = input.byte()?;
            if present == 0 {
                null_bits |= 1 << position;
                words[position] = u64::MAX;
                continue;
            }
            if present != 1 {
                return Err(exec_err("invalid compact partial group key null flag"));
            }
            words[position] = match column {
                KeyColumn::Integer { data_type, .. } => match data_type {
                    DataType::Int64 if tag == COMPACT_KEY_INT64 => {
                        i64::from_le_bytes(input.take(8)?.try_into().unwrap()) as u64
                    }
                    DataType::Int32 | DataType::Date32 if tag == COMPACT_KEY_INT32 => {
                        i32::from_le_bytes(input.take(4)?.try_into().unwrap()) as i64 as u64
                    }
                    DataType::Boolean if tag == COMPACT_KEY_BOOL => match input.byte()? {
                        0 => 0,
                        1 => 1,
                        _ => return Err(exec_err("invalid compact boolean group key")),
                    },
                    _ => return Err(exec_err("compact group key type mismatch")),
                },
                KeyColumn::Text { arena, .. } if tag == COMPACT_KEY_TEXT => {
                    let payload = if position + 1 == key_count {
                        input.take(input.0.len())?
                    } else {
                        let length = input.u32()? as usize;
                        input.take(length)?
                    };
                    let text = std::str::from_utf8(payload)
                        .map_err(|_| exec_err("partial group key is not valid UTF-8"))?;
                    let before = arena.bytes();
                    let id = arena.intern(hasher, text)?.0 as u64;
                    new_bytes += arena.bytes().saturating_sub(before);
                    id
                }
                KeyColumn::Text { .. } => {
                    return Err(exec_err("compact group key type mismatch"));
                }
            };
        }
        if !input.0.is_empty() {
            return Err(exec_err("trailing compact partial group key bytes"));
        }
        words[key_count] = null_bits;
        return Ok(new_bytes);
    }
    let mut input = KeyInput(key);
    if input.u64()? != key_count as u64 {
        return Err(exec_err(
            "partial group key count does not match the aggregate",
        ));
    }
    let mut null_bits = 0u64;
    let mut new_bytes = 0u64;
    for (position, column) in keys.iter_mut().enumerate() {
        let length = usize::try_from(input.u64()?)
            .map_err(|_| exec_err("partial group key is too large"))?;
        let (tag, payload) = input
            .take(length)?
            .split_first()
            .ok_or_else(|| exec_err("empty partial group key value"))?;
        let mismatch = || exec_err("partial group key type does not match the aggregate");
        words[position] = match (column, *tag) {
            (_, VALUE_NULL) if payload.is_empty() => {
                null_bits |= 1 << position;
                u64::MAX
            }
            (
                KeyColumn::Integer {
                    data_type: DataType::Int64,
                    ..
                },
                VALUE_INT64,
            ) => i64::from_le_bytes(payload.try_into().map_err(|_| mismatch())?) as u64,
            (
                KeyColumn::Integer {
                    data_type: DataType::Int32 | DataType::Date32,
                    ..
                },
                VALUE_INT32,
            ) => i32::from_le_bytes(payload.try_into().map_err(|_| mismatch())?) as i64 as u64,
            (
                KeyColumn::Integer {
                    data_type: DataType::Boolean,
                    ..
                },
                VALUE_BOOL,
            ) => match payload {
                [0] => 0,
                [1] => 1,
                _ => return Err(mismatch()),
            },
            (KeyColumn::Text { arena, .. }, VALUE_UTF8) => {
                let text = std::str::from_utf8(payload)
                    .map_err(|_| exec_err("partial group key is not valid UTF-8"))?;
                let before = arena.bytes();
                let id = arena.intern(hasher, text)?.0 as u64;
                new_bytes += arena.bytes().saturating_sub(before);
                id
            }
            _ => return Err(mismatch()),
        };
    }
    if !input.0.is_empty() {
        return Err(exec_err("trailing partial group key bytes"));
    }
    words[key_count] = null_bits;
    Ok(new_bytes)
}

/// A cursor over one encoded group key.
struct KeyInput<'a>(&'a [u8]);

impl<'a> KeyInput<'a> {
    #[inline]
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > self.0.len() {
            return Err(exec_err("truncated partial group key"));
        }
        let (value, tail) = self.0.split_at(length);
        self.0 = tail;
        Ok(value)
    }
    #[inline]
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("eight bytes"),
        ))
    }
    #[inline]
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }
    #[inline]
    fn byte(&mut self) -> Result<u8> {
        self.take(1).map(|bytes| bytes[0])
    }
}

/// Groups keyed by `keys` with `accumulators`, plus the slot index.
pub struct ColumnarGroups {
    keys: Vec<KeyColumn>,
    accumulators: Vec<AccColumn>,
    template: Vec<AggregateState>,
    index: SlotIndex,
    hasher: RandomState,
    /// Scratch per batch, row-major: each row's key words then its null
    /// bits (`keys.len() + 1` words), the row's hash, and its slot.
    packed: Vec<u64>,
    hashes: Vec<u64>,
    slots: Vec<u32>,
    len: usize,
}

impl ColumnarGroups {
    /// Build for the given key types (the batch's, dictionary or not) and
    /// the accumulator template. None when a type is not carried.
    pub fn new(key_types: &[DataType], template: &[AggregateState]) -> Option<Self> {
        if key_types.len() > MAX_KEYS {
            return None;
        }
        let mut keys = Vec::with_capacity(key_types.len());
        for data_type in key_types {
            keys.push(match data_type {
                DataType::Int64 | DataType::Int32 | DataType::Date32 | DataType::Boolean => {
                    KeyColumn::Integer {
                        values: Vec::new(),
                        nulls: Vec::new(),
                        data_type: data_type.clone(),
                    }
                }
                DataType::Utf8 => KeyColumn::Text {
                    words: Vec::new(),
                    nulls: Vec::new(),
                    arena: Arena::new(),
                    large: false,
                },
                DataType::LargeUtf8 => KeyColumn::Text {
                    words: Vec::new(),
                    nulls: Vec::new(),
                    arena: Arena::new(),
                    large: true,
                },
                DataType::Utf8View => KeyColumn::Text {
                    words: Vec::new(),
                    nulls: Vec::new(),
                    arena: Arena::new(),
                    large: false,
                },
                DataType::Dictionary(_, values) if supports_key(data_type) => KeyColumn::Text {
                    words: Vec::new(),
                    nulls: Vec::new(),
                    arena: Arena::new(),
                    large: values.as_ref() == &DataType::LargeUtf8,
                },
                _ => return None,
            });
        }
        let accumulators = template
            .iter()
            .map(AccColumn::for_state)
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            keys,
            accumulators,
            template: template.to_vec(),
            index: SlotIndex::new(),
            hasher: RandomState::new(),
            packed: Vec::new(),
            hashes: Vec::new(),
            slots: Vec::new(),
            len: 0,
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Empty the table for the next batch, keeping what it has allocated:
    /// the pass-through partial encodes every batch through one small
    /// table rather than a table of every group the task has seen.
    pub fn clear(&mut self) {
        for key in &mut self.keys {
            match key {
                KeyColumn::Integer { values, nulls, .. } => {
                    values.clear();
                    nulls.clear();
                }
                KeyColumn::Text {
                    words,
                    nulls,
                    arena,
                    ..
                } => {
                    words.clear();
                    nulls.clear();
                    arena.clear();
                }
            }
        }
        for accumulator in &mut self.accumulators {
            accumulator.clear();
        }
        self.index.clear();
        self.len = 0;
    }

    /// The accumulator template every group started from.
    pub fn template(&self) -> &[AggregateState] {
        &self.template
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Words per row in the packed scratch: the keys, then the null bits.
    fn stride(&self) -> usize {
        self.keys.len() + 1
    }

    /// Bytes one more group costs: its key words, its accumulators, the
    /// index bucket with the load factor's slack.
    pub fn slot_bytes(&self) -> u64 {
        let keys = self
            .keys
            .iter()
            .map(|key| match key {
                KeyColumn::Integer { .. } => 9,
                KeyColumn::Text { .. } => 5,
            })
            .sum::<u64>();
        keys + self
            .accumulators
            .iter()
            .map(AccColumn::slot_bytes)
            .sum::<u64>()
            + INDEX_BUCKET_BYTES_PER_SLOT
    }

    /// Scratch bytes a batch of `rows` takes while it is applied: the
    /// packed key words, a hash and a slot per row.
    pub fn scratch_bytes(&self, rows: usize) -> u64 {
        (rows as u64).saturating_mul((self.stride() as u64 + 1) * 8 + 4)
    }

    /// Bytes the largest doubling of the index and the columns needs at
    /// once if `incoming` more groups can arrive, or 0 when they fit: the
    /// old buffers stay alive until the copy is done. Small tables are
    /// covered by the per-group figure.
    pub fn growth_bytes(&self, incoming: usize) -> u64 {
        let mut capacity = self.index.capacity();
        if capacity < 1 << 16 || self.len + incoming <= capacity {
            return 0;
        }
        // Find the largest table that will be copied by a growth within the
        // batch. The reservation is the additional allocation: one current
        // table for a doubling, three current tables for a fourfold growth.
        while self.len + incoming > capacity {
            let multiplier = if self.uses_large_integer_growth()
                && capacity / INDEX_LOAD_NUMERATOR * INDEX_LOAD_DENOMINATOR
                    >= INDEX_LARGE_GROWTH_THRESHOLD
            {
                4
            } else {
                2
            };
            let next = capacity.saturating_mul(multiplier);
            if next >= self.len + incoming {
                return (capacity as u64)
                    .saturating_mul((multiplier - 1) as u64)
                    .saturating_mul(self.slot_bytes());
            }
            capacity = next;
        }
        0
    }

    #[inline]
    fn uses_large_integer_growth(&self) -> bool {
        self.keys
            .iter()
            .all(|key| matches!(key, KeyColumn::Integer { .. }))
    }

    /// Bytes the encoded partial batch takes: per group its length-prefixed
    /// key values, its compact states (a tag and up to sixteen bytes each)
    /// and the two offset entries. Text is sized from the arena's average.
    pub fn encoded_bytes(&self) -> u64 {
        let key = 8 + self
            .keys
            .iter()
            .map(|key| match key {
                KeyColumn::Integer { .. } => 17,
                KeyColumn::Text { arena, .. } => 9 + arena.bytes() / (self.len as u64).max(1),
            })
            .sum::<u64>();
        let states = 8 + 17 * self.accumulators.len() as u64;
        (self.len as u64).saturating_mul(key + states + 8)
    }

    /// Groups the index holds before it doubles.
    pub fn capacity(&self) -> usize {
        self.index.capacity()
    }

    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// The keys' types as the exchange carries them: text for dictionary keys.
    pub fn logical_key_types(&self) -> Vec<DataType> {
        self.keys
            .iter()
            .map(|key| match key {
                KeyColumn::Integer { data_type, .. } => data_type.clone(),
                KeyColumn::Text { large: true, .. } => DataType::LargeUtf8,
                KeyColumn::Text { .. } => DataType::Utf8,
            })
            .collect()
    }

    /// Aggregate one batch. `key_columns` are the batch's group key arrays in
    /// key order, `value_columns` the aggregate inputs (None for COUNT(*)).
    /// Returns the number of new groups and the arena bytes added.
    pub fn push_batch(
        &mut self,
        key_columns: &[ArrayRef],
        value_columns: &[Option<&ArrayRef>],
        rows: usize,
    ) -> Result<(usize, u64)> {
        let mut new_bytes = 0u64;
        let stride = self.stride();
        self.packed.clear();
        self.packed.resize(rows * stride, 0);
        for (position, array) in key_columns.iter().enumerate() {
            key_words(
                &mut self.keys[position],
                &self.hasher,
                array,
                &mut self.packed,
                stride,
                position,
                &mut new_bytes,
            )?;
        }
        let created = self.resolve_slots(rows)?;
        // One tight loop per aggregate over the batch.
        for (accumulator, column) in self.accumulators.iter_mut().zip(value_columns) {
            accumulator.update(*column, &self.slots)?;
        }
        Ok((created, new_bytes))
    }

    /// Hash every packed row, then find or create its group: `slots` holds
    /// one slot per row after. Returns the number of groups created.
    fn resolve_slots(&mut self, rows: usize) -> Result<usize> {
        let stride = self.stride();
        let key_count = self.keys.len();
        self.hashes.clear();
        self.hashes
            .extend(self.packed.chunks_exact(stride).take(rows).map(packed_hash));
        self.slots.clear();
        self.slots.reserve(rows);
        let mut created = 0usize;
        for row in 0..rows {
            if self.index.is_full() {
                let keys = &self.keys;
                self.index.grow(self.uses_large_integer_growth(), |slot| {
                    slot_hash(keys, slot as usize)
                });
            }
            if let Some(&ahead) = self.hashes.get(row + PREFETCH_DISTANCE) {
                self.index.prefetch(ahead);
            }
            let hash = self.hashes[row];
            let words = &self.packed[row * stride..(row + 1) * stride];
            let keys = &self.keys;
            let slot = match self
                .index
                .probe(hash, |slot| key_matches(keys, slot as usize, words))
            {
                Probe::Found(slot) => slot,
                Probe::Vacant(bucket) => {
                    let slot = u32::try_from(self.len)
                        .map_err(|_| exec_err("too many groups for one task"))?;
                    let null_bits = words[key_count];
                    for (position, word) in words.iter().enumerate().take(key_count) {
                        let null = null_bits & (1 << position) != 0;
                        match &mut self.keys[position] {
                            KeyColumn::Integer { values, nulls, .. } => {
                                values.push(if null { 0 } else { *word as i64 });
                                nulls.push(null);
                            }
                            KeyColumn::Text {
                                words: stored,
                                nulls,
                                ..
                            } => {
                                stored.push(if null { 0 } else { *word as u32 });
                                nulls.push(null);
                            }
                        }
                    }
                    for accumulator in &mut self.accumulators {
                        accumulator.push_identity();
                    }
                    self.index.occupy(bucket, hash, slot);
                    self.len += 1;
                    created += 1;
                    slot
                }
            };
            self.slots.push(slot);
        }
        Ok(created)
    }

    /// Push a batch of keys with no aggregates and report the rows that
    /// began a group: the first row of every key the table had not seen —
    /// DISTINCT as the grouped aggregate with nothing to accumulate.
    /// Returns those row indices and the text bytes added.
    pub fn push_batch_new_rows(
        &mut self,
        key_columns: &[ArrayRef],
        rows: usize,
    ) -> Result<(Vec<u32>, u64)> {
        let before = self.len;
        let (_, new_bytes) = self.push_batch(key_columns, &[], rows)?;
        let mut kept = Vec::with_capacity(self.len - before);
        let mut next = before as u32;
        for (row, &slot) in self.slots.iter().enumerate() {
            // Slots are handed out in row order, so the first row of a new
            // slot carries the next unseen id.
            if slot == next {
                kept.push(row as u32);
                next += 1;
            }
        }
        Ok((kept, new_bytes))
    }

    /// Merge one partial group (decoded key words + states) into the table.
    /// Keys arrive as the exchange's logical values.
    pub fn merge_group(
        &mut self,
        key: &[AggregateValue],
        states: &[AggregateState],
    ) -> Result<bool> {
        let key_count = self.keys.len();
        if key.len() != key_count || states.len() != self.accumulators.len() {
            return Err(exec_err(
                "partial group does not match the aggregate layout",
            ));
        }
        let stride = self.stride();
        self.packed.clear();
        self.packed.resize(stride, 0);
        for (position, value) in key.iter().enumerate() {
            let word = match (value, &mut self.keys[position]) {
                (AggregateValue::Null, _) => {
                    self.packed[key_count] |= 1 << position;
                    u64::MAX
                }
                (AggregateValue::Int64(v), KeyColumn::Integer { .. }) => *v as u64,
                (AggregateValue::Int32(v), KeyColumn::Integer { .. }) => *v as i64 as u64,
                (AggregateValue::Bool(v), KeyColumn::Integer { .. }) => *v as u64,
                (AggregateValue::Utf8(text), KeyColumn::Text { arena, .. }) => {
                    arena.intern(&self.hasher, text)?.0 as u64
                }
                _ => {
                    return Err(exec_err(
                        "partial group key type does not match the aggregate",
                    ));
                }
            };
            self.packed[position] = word;
        }
        let created = self.resolve_slots(1)? == 1;
        let slot = self.slots[0] as usize;
        for (accumulator, state) in self.accumulators.iter_mut().zip(states) {
            accumulator.merge(slot, state)?;
        }
        Ok(created)
    }

    /// Merge a batch of partial groups still in the exchange's encoding —
    /// the final stage's hot path. The key bytes are parsed straight into
    /// key words, the compact states fold straight into the accumulator
    /// columns: nothing is materialised per row. Returns the number of
    /// groups created and the text bytes added.
    pub fn merge_encoded_batch(
        &mut self,
        keys: &BinaryArray,
        states: &BinaryArray,
        rows: usize,
    ) -> Result<(usize, u64)> {
        let stride = self.stride();
        let mut new_bytes = 0u64;
        self.packed.clear();
        self.packed.resize(rows * stride, 0);
        for row in 0..rows {
            if row % 4096 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            if keys.is_null(row) || states.is_null(row) {
                return Err(exec_err("grouped aggregate state row cannot contain nulls"));
            }
            let words = &mut self.packed[row * stride..(row + 1) * stride];
            new_bytes += parse_key_words(&mut self.keys, &self.hasher, keys.value(row), words)?;
        }
        let created = self.resolve_slots(rows)?;
        for row in 0..rows {
            if row % 4096 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            let slot = self.slots[row] as usize;
            let mut input = compact_state::begin(states.value(row))?;
            if input.count != self.accumulators.len() {
                return Err(exec_err(
                    "partial group does not match the aggregate layout",
                ));
            }
            for accumulator in &mut self.accumulators {
                accumulator.merge_bytes(slot, &mut input.bytes)?;
            }
            input.finish()?;
        }
        Ok((created, new_bytes))
    }
    /// The group at `slot` as the row representation.
    pub(crate) fn group(&self, slot: usize) -> (Vec<GroupKey>, Vec<AggregateState>) {
        let keys = self
            .keys
            .iter()
            .map(|key| match key {
                KeyColumn::Integer {
                    values,
                    nulls,
                    data_type,
                } => {
                    if nulls[slot] {
                        GroupKey::Null
                    } else {
                        match data_type {
                            DataType::Int32 | DataType::Date32 => {
                                GroupKey::Int32(values[slot] as i32)
                            }
                            DataType::Boolean => GroupKey::Bool(values[slot] != 0),
                            _ => GroupKey::Int64(values[slot]),
                        }
                    }
                }
                KeyColumn::Text {
                    words,
                    nulls,
                    arena,
                    ..
                } => {
                    if nulls[slot] {
                        GroupKey::Null
                    } else {
                        GroupKey::Utf8(Arc::from(arena.get(words[slot])))
                    }
                }
            })
            .collect();
        let states = self
            .accumulators
            .iter()
            .map(|acc| acc.state(slot))
            .collect();
        (keys, states)
    }

    /// Every group, in slot order, as the row representation.
    pub(crate) fn into_groups(self) -> Vec<(Vec<GroupKey>, Vec<AggregateState>)> {
        (0..self.len).map(|slot| self.group(slot)).collect()
    }

    /// The states of the group at `slot`, written over `out` in place.
    fn states_into(&self, slot: usize, out: &mut Vec<AggregateState>) {
        out.clear();
        out.extend(self.accumulators.iter().map(|acc| acc.state(slot)));
    }

    /// The group key of `slot`, written in a compact typed exchange frame.
    /// Schema-known integer widths need no per-row type or length words; only
    /// non-final text keys carry a length because the row boundary is known.
    fn encode_keys_into(&self, slot: usize, out: &mut Vec<u8>) {
        let fixed = self.keys.iter().all(|key| {
            matches!(key, KeyColumn::Integer { nulls, .. } if !nulls[slot])
        });
        if fixed {
            out.extend_from_slice(FIXED_KEY_MAGIC);
            out.push(self.keys.len() as u8);
            for key in &self.keys {
                if let KeyColumn::Integer { data_type, .. } = key {
                    out.push(match data_type { DataType::Int32 | DataType::Date32 => COMPACT_KEY_INT32, DataType::Boolean => COMPACT_KEY_BOOL, _ => COMPACT_KEY_INT64 });
                }
            }
            for key in &self.keys {
                if let KeyColumn::Integer { values, data_type, .. } = key {
                    match data_type {
                        DataType::Int32 | DataType::Date32 => {
                            out.extend_from_slice(&(values[slot] as i32).to_le_bytes())
                        }
                        DataType::Boolean => out.push(u8::from(values[slot] != 0)),
                        _ => out.extend_from_slice(&values[slot].to_le_bytes()),
                    }
                }
            }
            return;
        }
        out.extend_from_slice(COMPACT_KEY_MAGIC);
        out.push(self.keys.len() as u8);
        for (position, key) in self.keys.iter().enumerate() {
            match key {
                KeyColumn::Integer {
                    values,
                    nulls,
                    data_type,
                } => {
                    out.push(match data_type {
                        DataType::Int32 | DataType::Date32 => COMPACT_KEY_INT32,
                        DataType::Boolean => COMPACT_KEY_BOOL,
                        _ => COMPACT_KEY_INT64,
                    });
                    if nulls[slot] {
                        out.push(0);
                    } else {
                        out.push(1);
                        match data_type {
                            DataType::Int32 | DataType::Date32 => {
                                out.extend_from_slice(&(values[slot] as i32).to_le_bytes());
                            }
                            DataType::Boolean => {
                                out.push(u8::from(values[slot] != 0));
                            }
                            _ => {
                                out.extend_from_slice(&values[slot].to_le_bytes());
                            }
                        }
                    }
                }
                KeyColumn::Text {
                    words,
                    nulls,
                    arena,
                    ..
                } => {
                    out.push(COMPACT_KEY_TEXT);
                    if nulls[slot] {
                        out.push(0);
                    } else {
                        let bytes = arena.get(words[slot]).as_bytes();
                        out.push(1);
                        if position + 1 == self.keys.len() {
                            out.extend_from_slice(bytes);
                        } else {
                            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                            out.extend_from_slice(bytes);
                        }
                    }
                }
            }
        }
    }

    /// Encode the primitive accumulator columns directly in the compact state
    /// wire format. The generic encoder first reconstructs an
    /// `AggregateState` for every group; that turns a high-cardinality
    /// columnar aggregate into millions of short-lived enums and vectors.
    /// This path is byte-for-byte compatible with that format and is used
    /// only where every state is a fixed-width primitive.
    fn encode_primitive_state_into(&self, slot: usize, out: &mut Vec<u8>) -> bool {
        if self.template.iter().any(|state| {
            !matches!(
                state,
                AggregateState::Count(_)
                    | AggregateState::IntegerSum { .. }
                    | AggregateState::IntegerMin(_)
                    | AggregateState::IntegerMax(_)
                    | AggregateState::Sum { .. }
                    | AggregateState::Avg { .. }
                    | AggregateState::Min(_)
                    | AggregateState::Max(_)
            )
        }) {
            return false;
        }
        out.extend_from_slice(crate::aggregate::compact_state::MAGIC);
        out.extend_from_slice(&(self.accumulators.len() as u32).to_le_bytes());
        for accumulator in &self.accumulators {
            match accumulator {
                AccColumn::Count(counts) => {
                    out.push(crate::aggregate::compact_state::TAG_COUNT);
                    out.extend_from_slice(&counts[slot].to_le_bytes());
                }
                AccColumn::IntegerSum { sums, counts } => {
                    out.push(crate::aggregate::compact_state::TAG_INTEGER_SUM);
                    out.extend_from_slice(&sums[slot].to_le_bytes());
                    out.extend_from_slice(&counts[slot].to_le_bytes());
                }
                AccColumn::IntegerMin { values, present } => {
                    out.push(crate::aggregate::compact_state::TAG_INTEGER_MIN);
                    out.push(u8::from(present[slot]));
                    if present[slot] {
                        out.extend_from_slice(&values[slot].to_le_bytes());
                    }
                }
                AccColumn::IntegerMax { values, present } => {
                    out.push(crate::aggregate::compact_state::TAG_INTEGER_MAX);
                    out.push(u8::from(present[slot]));
                    if present[slot] {
                        out.extend_from_slice(&values[slot].to_le_bytes());
                    }
                }
                AccColumn::Float { sums, counts, avg } => {
                    out.push(if *avg {
                        crate::aggregate::compact_state::TAG_AVG
                    } else {
                        crate::aggregate::compact_state::TAG_SUM
                    });
                    out.extend_from_slice(&sums[slot].to_le_bytes());
                    out.extend_from_slice(&counts[slot].to_le_bytes());
                }
                AccColumn::FloatMin { values, present } => {
                    out.push(crate::aggregate::compact_state::TAG_MIN);
                    out.push(u8::from(present[slot]));
                    if present[slot] {
                        out.extend_from_slice(&values[slot].to_le_bytes());
                    }
                }
                AccColumn::FloatMax { values, present } => {
                    out.push(crate::aggregate::compact_state::TAG_MAX);
                    out.push(u8::from(present[slot]));
                    if present[slot] {
                        out.extend_from_slice(&values[slot].to_le_bytes());
                    }
                }
                AccColumn::TextMin(_) | AccColumn::TextMax(_) => return false,
            }
        }
        true
    }

    /// Encoded group keys and compact states, straight from the columns:
    /// the partial stage's output without a row representation.
    pub(crate) fn encode(&self) -> Result<(BinaryBuilder, BinaryBuilder)> {
        let key_bytes = self
            .keys
            .iter()
            .map(|key| match key {
                KeyColumn::Integer { .. } => 17,
                KeyColumn::Text { arena, .. } => 9 + arena.bytes() as usize / self.len.max(1),
            })
            .sum::<usize>()
            + 8;
        let mut key_values = BinaryBuilder::with_capacity(self.len, self.len * key_bytes);
        let mut state_values = BinaryBuilder::with_capacity(self.len, self.len * 24);
        let mut key_scratch = Vec::new();
        let mut states = Vec::with_capacity(self.accumulators.len());
        let mut state_scratch = Vec::new();
        for slot in 0..self.len {
            if slot % 1024 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            key_scratch.clear();
            self.encode_keys_into(slot, &mut key_scratch);
            key_values.append_value(&key_scratch);
            state_scratch.clear();
            if !self.encode_primitive_state_into(slot, &mut state_scratch) {
                self.states_into(slot, &mut states);
                compact_state::encode_into(&states, &mut state_scratch)?;
            }
            state_values.append_value(&state_scratch);
        }
        Ok((key_values, state_values))
    }

    /// Encode the groups in `slots` the way `encode` does, each into the
    /// sink `partition_of` names for its encoded key bytes.
    pub(crate) fn encode_partitioned(
        &self,
        slots: std::ops::Range<usize>,
        partition_of: &dyn Fn(&[u8]) -> usize,
        sinks: &mut [(BinaryBuilder, BinaryBuilder)],
    ) -> Result<()> {
        let mut key_scratch = Vec::new();
        let mut states = Vec::with_capacity(self.accumulators.len());
        let mut state_scratch = Vec::new();
        for slot in slots {
            if slot % 1024 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            key_scratch.clear();
            self.encode_keys_into(slot, &mut key_scratch);
            state_scratch.clear();
            if !self.encode_primitive_state_into(slot, &mut state_scratch) {
                self.states_into(slot, &mut states);
                compact_state::encode_into(&states, &mut state_scratch)?;
            }
            let (key_values, state_values) = &mut sinks[partition_of(&key_scratch)];
            key_values.append_value(&key_scratch);
            state_values.append_value(&state_scratch);
        }
        Ok(())
    }

    /// The keys as Arrow arrays in the exchange's logical types.
    pub fn key_arrays(&self) -> Vec<ArrayRef> {
        self.keys
            .iter()
            .map(|key| key_array(key, 0..self.len))
            .collect()
    }

    /// Each accumulator column finalized as an Arrow array of `output_type`.
    pub fn output_arrays(&self, output_types: &[DataType]) -> Result<Vec<ArrayRef>> {
        self.accumulators
            .iter()
            .zip(output_types)
            .map(|(acc, output)| output_array(acc, output, 0..self.len))
            .collect()
    }

    /// The finalised key and output arrays of the groups in `slots`, the
    /// table untouched: a result emitted in bounded pieces while the
    /// table stays whole.
    pub fn final_arrays(
        &self,
        slots: std::ops::Range<usize>,
        output_types: &[DataType],
    ) -> Result<(Vec<ArrayRef>, Vec<ArrayRef>)> {
        if slots.end > self.len || slots.start > slots.end {
            return Err(exec_err("final aggregate slot range is out of bounds"));
        }
        let keys = self
            .keys
            .iter()
            .map(|key| key_array(key, slots.clone()))
            .collect();
        let outputs = self
            .accumulators
            .iter()
            .zip(output_types)
            .map(|(acc, output)| output_array(acc, output, slots.clone()))
            .collect::<Result<Vec<_>>>()?;
        Ok((keys, outputs))
    }

    /// The finalised key and output arrays, consuming the table: the index
    /// goes first, then each column is dropped as soon as its array is
    /// built, so the peak is one column's worth over the arrays rather than
    /// the whole table beside them.
    pub fn into_final_arrays(
        self,
        output_types: &[DataType],
    ) -> Result<(Vec<ArrayRef>, Vec<ArrayRef>)> {
        let Self {
            keys,
            accumulators,
            index,
            packed,
            hashes,
            slots,
            len,
            ..
        } = self;
        drop((index, packed, hashes, slots));
        let keys = keys
            .into_iter()
            .map(|key| {
                let array = key_array(&key, 0..len);
                drop(key);
                array
            })
            .collect();
        let outputs = accumulators
            .into_iter()
            .zip(output_types)
            .map(|(acc, output)| {
                let array = output_array(&acc, output, 0..len)?;
                drop(acc);
                Ok(array)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((keys, outputs))
    }
}

/// One key column's `slots` as the Arrow array of its exchange type.
fn key_array(key: &KeyColumn, slots: std::ops::Range<usize>) -> ArrayRef {
    match key {
        KeyColumn::Integer {
            values,
            nulls,
            data_type,
        } => {
            let values = &values[slots.clone()];
            let nulls = &nulls[slots];
            match data_type {
                DataType::Int32 | DataType::Date32 => {
                    let array = Int32Array::from_iter(
                        values
                            .iter()
                            .zip(nulls)
                            .map(|(v, n)| (!n).then_some(*v as i32)),
                    );
                    if data_type == &DataType::Date32 {
                        arrow::compute::cast(&array, &DataType::Date32).expect("Int32 to Date32")
                    } else {
                        Arc::new(array)
                    }
                }
                DataType::Boolean => Arc::new(BooleanArray::from_iter(
                    values
                        .iter()
                        .zip(nulls)
                        .map(|(v, n)| (!n).then_some(*v != 0)),
                )),
                _ => Arc::new(Int64Array::from_iter(
                    values.iter().zip(nulls).map(|(v, n)| (!n).then_some(*v)),
                )),
            }
        }
        KeyColumn::Text {
            words,
            nulls,
            arena,
            large,
        } => {
            let iter = words[slots.clone()]
                .iter()
                .zip(&nulls[slots])
                .map(|(w, n)| (!n).then(|| arena.get(*w)));
            if *large {
                Arc::new(arrow::array::LargeStringArray::from_iter(iter))
            } else {
                Arc::new(StringArray::from_iter(iter))
            }
        }
    }
}

/// One accumulator column's `slots` finalised as an Arrow array of `output`.
fn output_array(
    acc: &AccColumn,
    output: &DataType,
    slots: std::ops::Range<usize>,
) -> Result<ArrayRef> {
    Ok(match (acc, output) {
        (AccColumn::Count(counts), DataType::UInt64) => {
            Arc::new(UInt64Array::from(counts[slots].to_vec()))
        }
        (AccColumn::Count(counts), DataType::Int64) => Arc::new(Int64Array::from_iter(
            counts[slots].iter().map(|c| Some(*c as i64)),
        )),
        (AccColumn::IntegerSum { sums, counts }, DataType::Int64) => {
            Arc::new(Int64Array::from_iter(
                sums[slots.clone()]
                    .iter()
                    .zip(&counts[slots])
                    .map(|(s, c)| {
                        (*c > 0)
                            .then(|| {
                                i64::try_from(*s).map_err(|_| exec_err("integer SUM overflow"))
                            })
                            .transpose()
                    })
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        (AccColumn::IntegerSum { sums, counts }, DataType::Int32) => {
            Arc::new(Int32Array::from_iter(
                sums[slots.clone()]
                    .iter()
                    .zip(&counts[slots])
                    .map(|(s, c)| {
                        (*c > 0)
                            .then(|| {
                                i32::try_from(*s)
                                    .map_err(|_| exec_err("integer aggregate overflow"))
                            })
                            .transpose()
                    })
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        (
            AccColumn::IntegerMin { values, present } | AccColumn::IntegerMax { values, present },
            DataType::Int64,
        ) => Arc::new(Int64Array::from_iter(
            values[slots.clone()]
                .iter()
                .zip(&present[slots])
                .map(|(v, p)| p.then_some(*v)),
        )),
        (
            AccColumn::IntegerMin { values, present } | AccColumn::IntegerMax { values, present },
            DataType::Int32,
        ) => Arc::new(Int32Array::from_iter(
            values[slots.clone()]
                .iter()
                .zip(&present[slots])
                .map(|(v, p)| p.then_some(*v as i32)),
        )),
        (
            AccColumn::IntegerMin { values, present } | AccColumn::IntegerMax { values, present },
            DataType::Date32,
        ) => arrow::compute::cast(
            &Int32Array::from_iter(
                values[slots.clone()]
                    .iter()
                    .zip(&present[slots])
                    .map(|(v, p)| p.then_some(*v as i32)),
            ),
            &DataType::Date32,
        )?,
        (AccColumn::Float { sums, counts, avg }, DataType::Float64) => {
            Arc::new(Float64Array::from_iter(
                sums[slots.clone()]
                    .iter()
                    .zip(&counts[slots])
                    .map(|(s, c)| (*c > 0).then(|| if *avg { s / *c as f64 } else { *s })),
            ))
        }
        (
            AccColumn::FloatMin { values, present } | AccColumn::FloatMax { values, present },
            DataType::Float64,
        ) => Arc::new(Float64Array::from_iter(
            values[slots.clone()]
                .iter()
                .zip(&present[slots])
                .map(|(v, p)| p.then_some(*v)),
        )),
        (AccColumn::TextMin(values) | AccColumn::TextMax(values), DataType::Utf8) => Arc::new(
            StringArray::from_iter(values[slots].iter().map(|v| v.as_deref())),
        ),
        (AccColumn::TextMin(values) | AccColumn::TextMax(values), DataType::LargeUtf8) => Arc::new(
            arrow::array::LargeStringArray::from_iter(values[slots].iter().map(|v| v.as_deref())),
        ),
        (_, other) => {
            return Err(exec_err(format!(
                "columnar aggregate cannot produce {other}"
            )));
        }
    })
}

/// The exchange's logical key types for a batch schema's group columns.
pub fn exchanged_key_types(
    schema: &arrow::datatypes::SchemaRef,
    group_by: &[String],
) -> Result<Vec<DataType>> {
    group_by
        .iter()
        .map(|name| {
            schema
                .field_with_name(name)
                .map(|field| exchanged_group_key_type(field.data_type()))
                .map_err(KaveonError::from)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{DictionaryArray, Int32Array};

    fn template() -> Vec<AggregateState> {
        vec![
            AggregateState::Count(0),
            AggregateState::IntegerSum { sum: 0, count: 0 },
            AggregateState::Avg { sum: 0.0, count: 0 },
            AggregateState::Utf8Max(None),
        ]
    }

    #[test]
    fn null_keys_stay_one_group_across_batches_and_table_growth() {
        // Null keys hash as one word whatever bits the null slot holds, and
        // the rehash on growth reproduces the insert-time hash for nulls of
        // both key kinds — otherwise a resized table would lose them.
        let mut groups =
            ColumnarGroups::new(&[DataType::Int64, DataType::Utf8], &template()).unwrap();
        let rows = 100_000;
        let ints: ArrayRef = Arc::new(Int64Array::from_iter(
            (0..rows).map(|i| if i % 10 == 0 { None } else { Some(i as i64) }),
        ));
        let texts: ArrayRef = Arc::new(StringArray::from_iter((0..rows).map(|i| {
            if i % 7 == 0 {
                None
            } else {
                Some(format!("t{}", i % 1000))
            }
        })));
        let values: ArrayRef = Arc::new(Int64Array::from_iter_values((0..rows).map(|i| i as i64)));
        let (created, _) = groups
            .push_batch(
                &[ints.clone(), texts.clone()],
                &[None, Some(&values), Some(&values), Some(&texts)],
                rows,
            )
            .unwrap();
        assert!(groups.capacity() > 1 << 16);
        let (again, _) = groups
            .push_batch(
                &[ints, texts.clone()],
                &[None, Some(&values), Some(&values), Some(&texts)],
                rows,
            )
            .unwrap();
        assert_eq!(again, 0);
        assert_eq!(groups.len(), created);
        let both_null = (0..groups.len())
            .filter(|slot| {
                let (keys, states) = groups.group(*slot);
                keys == vec![GroupKey::Null, GroupKey::Null]
                    && states[0] == AggregateState::Count(2 * (rows as u64 / 70 + 1))
            })
            .count();
        assert_eq!(both_null, 1);
    }

    #[test]
    fn groups_by_integer_text_and_dictionary_keys_across_batches() {
        let mut groups = ColumnarGroups::new(
            &[
                DataType::Int64,
                DataType::Utf8,
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            ],
            &template(),
        )
        .unwrap();
        let dictionary = |keys: Vec<Option<i32>>, values: Vec<&str>| -> ArrayRef {
            Arc::new(DictionaryArray::<Int32Type>::new(
                Int32Array::from(keys),
                Arc::new(StringArray::from(values)),
            ))
        };
        let first_keys: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(vec![Some(1), Some(1), None, Some(2)])),
            Arc::new(StringArray::from(vec![
                Some("a"),
                Some("a"),
                Some("b"),
                None,
            ])),
            dictionary(vec![Some(0), Some(1), Some(0), None], vec!["x", "y"]),
        ];
        let values: ArrayRef = Arc::new(Int64Array::from(vec![Some(10), None, Some(3), Some(4)]));
        let texts: ArrayRef = Arc::new(StringArray::from(vec![
            Some("m"),
            Some("z"),
            None,
            Some("q"),
        ]));
        let (created, _) = groups
            .push_batch(
                &first_keys,
                &[None, Some(&values), Some(&values), Some(&texts)],
                4,
            )
            .unwrap();
        assert_eq!(created, 4);
        // Same values under different dictionary codes; two rows join group 0.
        let second_keys: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(vec![Some(1), Some(2), Some(1)])),
            Arc::new(StringArray::from(vec![Some("a"), None, Some("a")])),
            dictionary(vec![Some(1), None, Some(1)], vec!["y", "x"]),
        ];
        let values2: ArrayRef = Arc::new(Int64Array::from(vec![Some(5), Some(6), Some(7)]));
        let texts2: ArrayRef = Arc::new(StringArray::from(vec![Some("b"), Some("r"), Some("zz")]));
        let (created, _) = groups
            .push_batch(
                &second_keys,
                &[None, Some(&values2), Some(&values2), Some(&texts2)],
                3,
            )
            .unwrap();
        assert_eq!(created, 0);
        assert_eq!(groups.len(), 4);
        let (keys, states) = groups.group(0);
        assert_eq!(
            keys,
            vec![
                GroupKey::Int64(1),
                GroupKey::Utf8(Arc::from("a")),
                GroupKey::Utf8(Arc::from("x"))
            ]
        );
        assert_eq!(
            states,
            vec![
                AggregateState::Count(3),
                AggregateState::IntegerSum { sum: 22, count: 3 },
                AggregateState::Avg {
                    sum: 22.0,
                    count: 3
                },
                AggregateState::Utf8Max(Some("zz".into())),
            ]
        );
        let (keys, states) = groups.group(2);
        assert_eq!(
            keys,
            vec![
                GroupKey::Null,
                GroupKey::Utf8(Arc::from("b")),
                GroupKey::Utf8(Arc::from("x"))
            ]
        );
        assert_eq!(states[0], AggregateState::Count(1));
        assert_eq!(states[3], AggregateState::Utf8Max(None));
        let (keys, _) = groups.group(3);
        assert_eq!(
            keys,
            vec![GroupKey::Int64(2), GroupKey::Null, GroupKey::Null]
        );
        // Arrays reflect the same groups.
        let arrays = groups.key_arrays();
        assert_eq!(arrays[0].as_primitive::<Int64Type>().value(0), 1);
        assert!(arrays[1].is_null(3));
        let outputs = groups
            .output_arrays(&[
                DataType::UInt64,
                DataType::Int64,
                DataType::Float64,
                DataType::Utf8,
            ])
            .unwrap();
        assert_eq!(outputs[1].as_primitive::<Int64Type>().value(0), 22);
        assert!((outputs[2].as_primitive::<Float64Type>().value(0) - 22.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn merges_partial_groups_by_value() {
        let mut groups =
            ColumnarGroups::new(&[DataType::Utf8, DataType::Int32], &template()).unwrap();
        let key = |text: Option<&str>, n: Option<i32>| {
            vec![
                text.map_or(AggregateValue::Null, |t| AggregateValue::Utf8(t.into())),
                n.map_or(AggregateValue::Null, AggregateValue::Int32),
            ]
        };
        let states = |count: u64, sum: i128, text: Option<&str>| {
            vec![
                AggregateState::Count(count),
                AggregateState::IntegerSum { sum, count },
                AggregateState::Avg {
                    sum: sum as f64,
                    count,
                },
                AggregateState::Utf8Max(text.map(str::to_owned)),
            ]
        };
        assert!(
            groups
                .merge_group(&key(Some("a"), Some(1)), &states(2, 10, Some("k")))
                .unwrap()
        );
        assert!(
            !groups
                .merge_group(&key(Some("a"), Some(1)), &states(1, 5, Some("z")))
                .unwrap()
        );
        assert!(
            groups
                .merge_group(&key(None, Some(1)), &states(1, 1, None))
                .unwrap()
        );
        assert!(
            groups
                .merge_group(&key(Some("a"), None), &states(1, 1, None))
                .unwrap()
        );
        assert_eq!(groups.len(), 3);
        let (keys, states) = groups.group(0);
        assert_eq!(
            keys,
            vec![GroupKey::Utf8(Arc::from("a")), GroupKey::Int32(1)]
        );
        assert_eq!(states[0], AggregateState::Count(3));
        assert_eq!(states[1], AggregateState::IntegerSum { sum: 15, count: 3 });
        assert_eq!(states[3], AggregateState::Utf8Max(Some("z".into())));
    }

    #[test]
    fn utf8_view_keys_fold_with_the_logical_utf8_result() {
        let mut groups =
            ColumnarGroups::new(&[DataType::Utf8View], &[AggregateState::Count(0)]).unwrap();
        let values: ArrayRef = Arc::new(StringViewArray::from(vec![
            Some("alpha"),
            Some("alpha"),
            None,
            Some("beta"),
        ]));
        let (created, _) = groups.push_batch(&[values], &[None], 4).unwrap();
        assert_eq!(created, 3);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups.group(0).0, vec![GroupKey::Utf8(Arc::from("alpha"))]);
        assert_eq!(groups.group(0).1, vec![AggregateState::Count(2)]);
        assert_eq!(groups.group(1).0, vec![GroupKey::Null]);
        assert_eq!(groups.group(1).1, vec![AggregateState::Count(1)]);
        assert_eq!(groups.group(2).0, vec![GroupKey::Utf8(Arc::from("beta"))]);
        assert_eq!(groups.key_arrays()[0].data_type(), &DataType::Utf8);
    }

    #[test]
    fn merges_encoded_batches_of_every_accumulator_kind_like_the_row_merge() {
        // Every in-place accumulator folds straight from its compact bytes:
        // the result matches the row merge state for state, including the
        // absent extrema and the text ones.
        use crate::aggregate::{
            GroupedAggregateState, grouped_aggregate_states_to_typed_batch,
            merge_grouped_aggregate_states,
        };
        let types = [DataType::Int64, DataType::Utf8];
        let states = |n: i64, text: Option<&str>| {
            vec![
                AggregateState::Count(n.unsigned_abs()),
                AggregateState::IntegerSum {
                    sum: n as i128,
                    count: 1,
                },
                AggregateState::IntegerMin(Some(n)),
                AggregateState::IntegerMax(text.map(|_| n)),
                AggregateState::Sum {
                    sum: n as f64 / 2.0,
                    count: 1,
                },
                AggregateState::Avg {
                    sum: n as f64,
                    count: 2,
                },
                AggregateState::Min(text.map(|_| -(n as f64))),
                AggregateState::Max(Some(n as f64)),
                AggregateState::Utf8Min(text.map(str::to_owned)),
                AggregateState::Utf8Max(text.map(str::to_owned)),
            ]
        };
        let key = |k: i64, t: Option<&str>| {
            vec![
                AggregateValue::Int64(k),
                t.map_or(AggregateValue::Null, |t| AggregateValue::Utf8(t.into())),
            ]
        };
        let rows = vec![
            GroupedAggregateState {
                group_keys: key(1, Some("a")),
                states: states(5, Some("m")),
            },
            GroupedAggregateState {
                group_keys: key(1, Some("a")),
                states: states(-3, Some("b")),
            },
            GroupedAggregateState {
                group_keys: key(1, Some("a")),
                states: states(9, None),
            },
            GroupedAggregateState {
                group_keys: key(2, None),
                states: states(7, None),
            },
            GroupedAggregateState {
                group_keys: key(2, None),
                states: states(-7, Some("zz")),
            },
        ];
        let mut groups = ColumnarGroups::new(&types, &rows[0].states).unwrap();
        for batch in rows.chunks(2) {
            let batch = grouped_aggregate_states_to_typed_batch(batch, &types).unwrap();
            let keys = batch
                .column(0)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            let states = batch
                .column(1)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            groups
                .merge_encoded_batch(keys, states, batch.num_rows())
                .unwrap();
        }
        assert_eq!(groups.len(), 2);
        let mut actual = (0..2)
            .map(|slot| {
                let (keys, states) = groups.group(slot);
                let keys = keys
                    .into_iter()
                    .map(AggregateValue::from)
                    .collect::<Vec<_>>();
                (format!("{keys:?}"), format!("{states:?}"))
            })
            .collect::<Vec<_>>();
        actual.sort();
        let mut expected = merge_grouped_aggregate_states(rows)
            .unwrap()
            .into_iter()
            .map(|g| (format!("{:?}", g.group_keys), format!("{:?}", g.states)))
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(actual, expected);
    }

    #[test]
    fn encoded_batch_merge_rejects_states_of_another_layout_or_malformed_bytes() {
        use crate::aggregate::{GroupedAggregateState, grouped_aggregate_states_to_typed_batch};
        let types = [DataType::Int64];
        let batch = |state: AggregateState| {
            grouped_aggregate_states_to_typed_batch(
                &[GroupedAggregateState {
                    group_keys: vec![AggregateValue::Int64(1)],
                    states: vec![state],
                }],
                &types,
            )
            .unwrap()
        };
        let merge = |groups: &mut ColumnarGroups, keys: &BinaryArray, states: &BinaryArray| {
            groups.merge_encoded_batch(keys, states, keys.len())
        };
        let mut groups = ColumnarGroups::new(&types, &[AggregateState::Count(0)]).unwrap();
        let good = batch(AggregateState::Count(2));
        let keys = good
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let states = good
            .column(1)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert_eq!(merge(&mut groups, keys, states).unwrap(), (1, 0));

        // A state whose tag is not this column's.
        let other = batch(AggregateState::IntegerSum { sum: 2, count: 1 });
        let wrong = other
            .column(1)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(merge(&mut groups, keys, wrong).is_err());

        // Two states where the layout has one, and the count that says so.
        let two = grouped_aggregate_states_to_typed_batch(
            &[GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(1)],
                states: vec![AggregateState::Count(1), AggregateState::Count(1)],
            }],
            &types,
        )
        .unwrap();
        let wrong = two
            .column(1)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(merge(&mut groups, keys, wrong).is_err());

        // Truncated state bytes.
        let bytes = states.value(0);
        for end in 0..bytes.len() {
            let truncated = BinaryArray::from(vec![&bytes[..end]]);
            assert!(merge(&mut groups, keys, &truncated).is_err(), "{end}");
        }

        // A key with trailing bytes.
        let mut key = keys.value(0).to_vec();
        key.push(0);
        let bad_key = BinaryArray::from(vec![key.as_slice()]);
        assert!(merge(&mut groups, &bad_key, states).is_err());

        // None of those touched the group; the good row still merges into it.
        assert_eq!(merge(&mut groups, keys, states).unwrap(), (0, 0));
        assert_eq!(groups.group(0).1, vec![AggregateState::Count(4)]);

        // Trailing state bytes fail the row once its states are read.
        let mut trailing = bytes.to_vec();
        trailing.push(0);
        let trailing = BinaryArray::from(vec![trailing.as_slice()]);
        assert!(merge(&mut groups, keys, &trailing).is_err());
    }

    #[test]
    fn growth_bytes_covers_the_largest_index_growth_a_batch_can_cause() {
        let mut groups =
            ColumnarGroups::new(&[DataType::Int64], &[AggregateState::Count(0)]).unwrap();
        assert_eq!(groups.growth_bytes(1 << 20), 0);
        let rows = 70_000;
        let ints: ArrayRef = Arc::new(Int64Array::from_iter_values(0..rows as i64));
        groups.push_batch(&[ints], &[None], rows).unwrap();
        let capacity = groups.capacity();
        assert!(capacity >= 1 << 16);
        assert!(groups.len() <= capacity);
        // Fits: nothing to reserve.
        assert_eq!(groups.growth_bytes(capacity - groups.len()), 0);
        // One large-table growth: the current table is copied.
        assert_eq!(
            groups.growth_bytes(capacity - groups.len() + 1),
            (capacity as u64) * 3 * groups.slot_bytes()
        );
        // Further growth within one batch reserves the largest table copied.
        assert_eq!(
            groups.growth_bytes(capacity * 4 + 1 - groups.len()),
            (capacity as u64) * 12 * groups.slot_bytes()
        );
    }
}
