//! The columnar hash aggregate: keys as typed vectors, accumulators as flat
//! columns, one hash table of slot ids. A batch is processed in three
//! passes — key words per row, slot per row, then one tight loop per
//! aggregate over the batch — so the per-row cost is a probe and a few
//! stores, not an enum dispatch and a pointer chase per accumulator.
//!
//! Covers every key type the exchange can carry (integers, dates, booleans,
//! text, dictionary-encoded text) and the accumulators that update in place:
//! COUNT, integer and floating SUM/AVG/MIN/MAX, text MIN/MAX. Distinct,
//! exact-decimal and decimal states stay on the row path.
use std::sync::Arc;

use ahash::RandomState;
use arrow::array::{
    Array, ArrayRef, AsArray, BinaryBuilder, BooleanArray, Float64Array, Int32Array,
    Int32DictionaryArray, Int64Array, StringArray, UInt64Array,
};
use arrow::datatypes::{DataType, Date32Type, Float64Type, Int32Type, Int64Type};
use hashbrown::HashTable;
use kaveon_core::{KaveonError, Result};

use crate::aggregate::compact_state;
use crate::aggregate::{
    AggExpr, AggFunc, AggregateState, AggregateValue, GroupKey, VALUE_BOOL, VALUE_INT32,
    VALUE_INT64, VALUE_NULL, VALUE_UTF8, exchanged_group_key_type, exec_err,
};

/// The most key columns a row packs into its hash; wider GROUP BYs take the
/// row path.
pub const MAX_KEYS: usize = 8;

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
    index: HashTable<u32>,
}

impl Arena {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            offsets: vec![0],
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
    fn intern(&mut self, hasher: &RandomState, text: &str) -> Result<(u32, bool)> {
        let hash = hasher.hash_one(text.as_bytes());
        let bytes = &self.bytes;
        let offsets = &self.offsets;
        let found = self.index.find(hash, |&id| {
            let (start, end) = (
                offsets[id as usize] as usize,
                offsets[id as usize + 1] as usize,
            );
            &bytes[start..end] == text.as_bytes()
        });
        if let Some(&id) = found {
            return Ok((id, false));
        }
        let id = u32::try_from(self.offsets.len() - 1)
            .map_err(|_| exec_err("too many distinct strings for one task"))?;
        self.bytes.extend_from_slice(text.as_bytes());
        let end =
            u32::try_from(self.bytes.len()).map_err(|_| exec_err("string arena exceeds 4 GiB"))?;
        self.offsets.push(end);
        let bytes = &self.bytes;
        let offsets = &self.offsets;
        self.index.insert_unique(hash, id, |&other| {
            let (start, end) = (
                offsets[other as usize] as usize,
                offsets[other as usize + 1] as usize,
            );
            hasher.hash_one(&bytes[start..end])
        });
        Ok((id, true))
    }
    fn bytes(&self) -> u64 {
        (self.bytes.capacity() + self.offsets.capacity() * 4 + self.index.capacity() * 8) as u64
    }
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
        Some(DataType::Utf8 | DataType::LargeUtf8) => {
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

/// Words per row for one key column of a batch: the key's bits, or an
/// arena id for text. `u64::MAX` with the null flag set marks null.
fn key_words(
    column: &mut KeyColumn,
    hasher: &RandomState,
    array: &ArrayRef,
    words: &mut Vec<u64>,
    nulls: &mut Vec<bool>,
    new_bytes: &mut u64,
) -> Result<()> {
    let rows = array.len();
    words.clear();
    nulls.clear();
    match column {
        KeyColumn::Integer { .. } => match array.data_type() {
            // A null slot's value bits are arbitrary; every null hashes as
            // the null word.
            DataType::Int64 | DataType::Int32 | DataType::Date32 | DataType::Boolean
                if array.null_count() != 0 =>
            {
                words.extend((0..rows).map(|row| {
                    if array.is_null(row) {
                        u64::MAX
                    } else {
                        match array.data_type() {
                            DataType::Int64 => array.as_primitive::<Int64Type>().value(row) as u64,
                            DataType::Boolean => array.as_boolean().value(row) as u64,
                            DataType::Date32 => {
                                array.as_primitive::<Date32Type>().value(row) as i64 as u64
                            }
                            _ => array.as_primitive::<Int32Type>().value(row) as i64 as u64,
                        }
                    }
                }));
                nulls.extend((0..rows).map(|row| array.is_null(row)));
            }
            DataType::Int64 => {
                let values = array.as_primitive::<Int64Type>();
                words.extend(values.values().iter().map(|v| *v as u64));
                nulls.extend((0..rows).map(|row| values.is_null(row)));
            }
            DataType::Int32 => {
                let values = array.as_primitive::<Int32Type>();
                words.extend(values.values().iter().map(|v| *v as i64 as u64));
                nulls.extend((0..rows).map(|row| values.is_null(row)));
            }
            DataType::Date32 => {
                let values = array.as_primitive::<Date32Type>();
                words.extend(values.values().iter().map(|v| *v as i64 as u64));
                nulls.extend((0..rows).map(|row| values.is_null(row)));
            }
            DataType::Boolean => {
                let values = array.as_boolean();
                words.extend((0..rows).map(|row| values.value(row) as u64));
                nulls.extend((0..rows).map(|row| values.is_null(row)));
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
                            words.push(u64::MAX);
                            nulls.push(true);
                        } else {
                            words.push(intern(values.value(row))?);
                            nulls.push(false);
                        }
                    }
                } else {
                    let values = array.as_string::<i64>();
                    for row in 0..rows {
                        if values.is_null(row) {
                            words.push(u64::MAX);
                            nulls.push(true);
                        } else {
                            words.push(intern(values.value(row))?);
                            nulls.push(false);
                        }
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
                        words.push(u64::MAX);
                        nulls.push(true);
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
                                    words.push(u64::MAX);
                                    nulls.push(true);
                                    continue;
                                }
                            }
                        }
                    };
                    words.push(word);
                    nulls.push(false);
                }
                *new_bytes += arena.bytes().saturating_sub(before);
            }
            other => return Err(exec_err(format!("group key column type changed: {other}"))),
        },
    }
    Ok(())
}

/// Groups keyed by `keys` with `accumulators`, plus the slot index.
pub struct ColumnarGroups {
    keys: Vec<KeyColumn>,
    accumulators: Vec<AccColumn>,
    template: Vec<AggregateState>,
    index: HashTable<u32>,
    hasher: RandomState,
    /// Scratch per batch: words and nulls per key column, hashes, slots.
    words: Vec<Vec<u64>>,
    nulls: Vec<Vec<bool>>,
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
            index: HashTable::new(),
            hasher: RandomState::new(),
            words: vec![Vec::new(); key_types.len()],
            nulls: vec![Vec::new(); key_types.len()],
            hashes: Vec::new(),
            slots: Vec::new(),
            len: 0,
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// The accumulator template every group started from.
    pub fn template(&self) -> &[AggregateState] {
        &self.template
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bytes one more group costs: its key words, its accumulators, the
    /// index entry with hashbrown's overhead.
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
            + 16
    }

    /// Bytes a doubling of the index and the columns needs at once if
    /// `incoming` more groups can arrive, or 0 when they fit: the old
    /// buffers stay alive until the copy is done. Small tables are covered
    /// by the per-group figure.
    pub fn growth_bytes(&self, incoming: usize) -> u64 {
        let capacity = self.index.capacity();
        if capacity < 1 << 16 || self.len + incoming <= capacity {
            return 0;
        }
        (capacity as u64).saturating_mul(self.slot_bytes())
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
        for (position, array) in key_columns.iter().enumerate() {
            let (words, nulls) = (&mut self.words[position], &mut self.nulls[position]);
            key_words(
                &mut self.keys[position],
                &self.hasher,
                array,
                words,
                nulls,
                &mut new_bytes,
            )?;
        }
        // Hash every row from its words and null flags.
        self.hashes.clear();
        self.hashes.reserve(rows);
        let key_count = self.keys.len();
        let mut packed = [0u64; MAX_KEYS + 1];
        for row in 0..rows {
            let mut null_bits = 0u64;
            for (position, word) in packed.iter_mut().enumerate().take(key_count) {
                *word = self.words[position][row];
                if self.nulls[position][row] {
                    null_bits |= 1 << position;
                }
            }
            packed[key_count] = null_bits;
            self.hashes
                .push(self.hasher.hash_one(&packed[..=key_count]));
        }
        // Resolve a slot per row.
        self.slots.clear();
        self.slots.reserve(rows);
        let mut created = 0usize;
        for row in 0..rows {
            let hash = self.hashes[row];
            let words = &self.words;
            let nulls = &self.nulls;
            let keys = &self.keys;
            let found = self.index.find(hash, |&slot| {
                let slot = slot as usize;
                (0..key_count).all(|position| {
                    let null = nulls[position][row];
                    match &keys[position] {
                        KeyColumn::Integer {
                            values,
                            nulls: stored,
                            ..
                        } => {
                            stored[slot] == null
                                && (null || values[slot] as u64 == words[position][row])
                        }
                        KeyColumn::Text {
                            words: stored_words,
                            nulls: stored,
                            ..
                        } => {
                            stored[slot] == null
                                && (null || stored_words[slot] as u64 == words[position][row])
                        }
                    }
                })
            });
            let slot = match found {
                Some(&slot) => slot,
                None => {
                    let slot = u32::try_from(self.len)
                        .map_err(|_| exec_err("too many groups for one task"))?;
                    for position in 0..key_count {
                        let null = self.nulls[position][row];
                        let word = self.words[position][row];
                        match &mut self.keys[position] {
                            KeyColumn::Integer {
                                values,
                                nulls: stored,
                                ..
                            } => {
                                values.push(if null { 0 } else { word as i64 });
                                stored.push(null);
                            }
                            KeyColumn::Text {
                                words: stored_words,
                                nulls: stored,
                                ..
                            } => {
                                stored_words.push(if null { 0 } else { word as u32 });
                                stored.push(null);
                            }
                        }
                    }
                    for accumulator in &mut self.accumulators {
                        accumulator.push_identity();
                    }
                    let hashes_of = |keys: &[KeyColumn], hasher: &RandomState, slot: u32| -> u64 {
                        let slot = slot as usize;
                        let mut packed = [0u64; MAX_KEYS + 1];
                        let mut null_bits = 0u64;
                        for (position, key) in keys.iter().enumerate() {
                            let (word, null) = match key {
                                KeyColumn::Integer { values, nulls, .. } => {
                                    (values[slot] as u64, nulls[slot])
                                }
                                KeyColumn::Text { words, nulls, .. } => {
                                    (words[slot] as u64, nulls[slot])
                                }
                            };
                            packed[position] = if null { u64::MAX } else { word };
                            if null {
                                null_bits |= 1 << position;
                            }
                        }
                        packed[keys.len()] = null_bits;
                        hasher.hash_one(&packed[..=keys.len()])
                    };
                    let keys = &self.keys;
                    let hasher = &self.hasher;
                    self.index
                        .insert_unique(hash, slot, |&other| hashes_of(keys, hasher, other));
                    self.len += 1;
                    created += 1;
                    slot
                }
            };
            self.slots.push(slot);
        }
        // One tight loop per aggregate over the batch.
        for (accumulator, column) in self.accumulators.iter_mut().zip(value_columns) {
            accumulator.update(*column, &self.slots)?;
        }
        Ok((created, new_bytes))
    }

    /// Merge one partial group (decoded key words + states) into the table:
    /// the final stage's path. Keys arrive as the exchange's logical values.
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
        let mut null_bits = 0u64;
        let mut words = [0u64; MAX_KEYS];
        for (position, value) in key.iter().enumerate() {
            let word = match (value, &mut self.keys[position]) {
                (AggregateValue::Null, _) => {
                    null_bits |= 1 << position;
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
            words[position] = word;
        }
        self.merge_words(words, null_bits, states)
    }

    /// Merge one partial group whose key is still in the exchange's
    /// encoding — the final stage's hot path: the bytes are parsed straight
    /// into key words, nothing is materialised per row. Returns whether the
    /// group is new and the text bytes it added.
    pub fn merge_encoded(&mut self, key: &[u8], states: &[AggregateState]) -> Result<(bool, u64)> {
        let key_count = self.keys.len();
        if states.len() != self.accumulators.len() {
            return Err(exec_err(
                "partial group does not match the aggregate layout",
            ));
        }
        let mut offset = 0usize;
        let count = read_u64(key, &mut offset)?;
        if count != key_count as u64 {
            return Err(exec_err(
                "partial group key count does not match the aggregate",
            ));
        }
        let mut null_bits = 0u64;
        let mut words = [0u64; MAX_KEYS];
        let mut new_bytes = 0u64;
        for (position, word) in words.iter_mut().enumerate().take(key_count) {
            let length = usize::try_from(read_u64(key, &mut offset)?)
                .map_err(|_| exec_err("partial group key is too large"))?;
            let end = offset
                .checked_add(length)
                .ok_or_else(|| exec_err("partial group key length overflow"))?;
            let encoded = key
                .get(offset..end)
                .ok_or_else(|| exec_err("truncated partial group key"))?;
            offset = end;
            let (tag, payload) = encoded
                .split_first()
                .ok_or_else(|| exec_err("empty partial group key value"))?;
            let mismatch = || exec_err("partial group key type does not match the aggregate");
            *word = match (&mut self.keys[position], *tag) {
                (_, VALUE_NULL) => {
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
                    let id = arena.intern(&self.hasher, text)?.0 as u64;
                    new_bytes += arena.bytes().saturating_sub(before);
                    id
                }
                _ => return Err(mismatch()),
            };
        }
        if offset != key.len() {
            return Err(exec_err("trailing partial group key bytes"));
        }
        let created = self.merge_words(words, null_bits, states)?;
        Ok((created, new_bytes))
    }

    /// Probe or insert the group for `words`, then merge `states` into it.
    fn merge_words(
        &mut self,
        words: [u64; MAX_KEYS],
        null_bits: u64,
        states: &[AggregateState],
    ) -> Result<bool> {
        let key_count = self.keys.len();
        let mut packed = [0u64; MAX_KEYS + 1];
        packed[..key_count].copy_from_slice(&words[..key_count]);
        packed[key_count] = null_bits;
        let hash = self.hasher.hash_one(&packed[..=key_count]);
        let keys = &self.keys;
        let found = self.index.find(hash, |&slot| {
            let slot = slot as usize;
            (0..key_count).all(|position| {
                let null = null_bits & (1 << position) != 0;
                match &keys[position] {
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
        });
        let (slot, created) = match found {
            Some(&slot) => (slot, false),
            None => {
                let slot = u32::try_from(self.len)
                    .map_err(|_| exec_err("too many groups for one task"))?;
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
                let keys = &self.keys;
                let hasher = &self.hasher;
                self.index.insert_unique(hash, slot, |&other| {
                    let other = other as usize;
                    let mut packed = [0u64; MAX_KEYS + 1];
                    let mut bits = 0u64;
                    for (position, key) in keys.iter().enumerate() {
                        let (word, null) = match key {
                            KeyColumn::Integer { values, nulls, .. } => {
                                (values[other] as u64, nulls[other])
                            }
                            KeyColumn::Text { words, nulls, .. } => {
                                (words[other] as u64, nulls[other])
                            }
                        };
                        packed[position] = if null { u64::MAX } else { word };
                        if null {
                            bits |= 1 << position;
                        }
                    }
                    packed[keys.len()] = bits;
                    hasher.hash_one(&packed[..=keys.len()])
                });
                self.len += 1;
                (slot, true)
            }
        };
        for (accumulator, state) in self.accumulators.iter_mut().zip(states) {
            accumulator.merge(slot as usize, state)?;
        }
        Ok(created)
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

    /// The group key of `slot` in the exchange's key encoding: a count,
    /// then each key as a length-prefixed tagged value — the same bytes the
    /// row encoder writes, produced from the columns.
    fn encode_keys_into(&self, slot: usize, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.keys.len() as u64).to_le_bytes());
        for key in &self.keys {
            let length_at = out.len();
            out.extend_from_slice(&0u64.to_le_bytes());
            match key {
                KeyColumn::Integer {
                    values,
                    nulls,
                    data_type,
                } => {
                    if nulls[slot] {
                        out.push(VALUE_NULL);
                    } else {
                        match data_type {
                            DataType::Int32 | DataType::Date32 => {
                                out.push(VALUE_INT32);
                                out.extend_from_slice(&(values[slot] as i32).to_le_bytes());
                            }
                            DataType::Boolean => {
                                out.push(VALUE_BOOL);
                                out.push(u8::from(values[slot] != 0));
                            }
                            _ => {
                                out.push(VALUE_INT64);
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
                    if nulls[slot] {
                        out.push(VALUE_NULL);
                    } else {
                        out.push(VALUE_UTF8);
                        out.extend_from_slice(arena.get(words[slot]).as_bytes());
                    }
                }
            }
            let length = (out.len() - length_at - 8) as u64;
            out[length_at..length_at + 8].copy_from_slice(&length.to_le_bytes());
        }
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
            self.states_into(slot, &mut states);
            state_scratch.clear();
            compact_state::encode_into(&states, &mut state_scratch)?;
            state_values.append_value(&state_scratch);
        }
        Ok((key_values, state_values))
    }

    /// The keys as Arrow arrays in the exchange's logical types.
    pub fn key_arrays(&self) -> Vec<ArrayRef> {
        self.keys
            .iter()
            .map(|key| -> ArrayRef {
                match key {
                    KeyColumn::Integer {
                        values,
                        nulls,
                        data_type,
                    } => match data_type {
                        DataType::Int32 | DataType::Date32 => {
                            let array = Int32Array::from_iter(
                                values
                                    .iter()
                                    .zip(nulls)
                                    .map(|(v, n)| (!n).then_some(*v as i32)),
                            );
                            if data_type == &DataType::Date32 {
                                arrow::compute::cast(&array, &DataType::Date32)
                                    .expect("Int32 to Date32")
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
                    },
                    KeyColumn::Text {
                        words,
                        nulls,
                        arena,
                        large,
                    } => {
                        let iter = words
                            .iter()
                            .zip(nulls)
                            .map(|(w, n)| (!n).then(|| arena.get(*w)));
                        if *large {
                            Arc::new(arrow::array::LargeStringArray::from_iter(iter))
                        } else {
                            Arc::new(StringArray::from_iter(iter))
                        }
                    }
                }
            })
            .collect()
    }

    /// Each accumulator column finalized as an Arrow array of `output_type`.
    pub fn output_arrays(&self, output_types: &[DataType]) -> Result<Vec<ArrayRef>> {
        self.accumulators
            .iter()
            .zip(output_types)
            .map(|(acc, output)| -> Result<ArrayRef> {
                Ok(match (acc, output) {
                    (AccColumn::Count(counts), DataType::UInt64) => {
                        Arc::new(UInt64Array::from(counts.clone()))
                    }
                    (AccColumn::Count(counts), DataType::Int64) => Arc::new(Int64Array::from_iter(
                        counts.iter().map(|c| Some(*c as i64)),
                    )),
                    (AccColumn::IntegerSum { sums, counts }, DataType::Int64) => {
                        Arc::new(Int64Array::from_iter(
                            sums.iter()
                                .zip(counts)
                                .map(|(s, c)| {
                                    (*c > 0)
                                        .then(|| {
                                            i64::try_from(*s)
                                                .map_err(|_| exec_err("integer SUM overflow"))
                                        })
                                        .transpose()
                                })
                                .collect::<Result<Vec<_>>>()?,
                        ))
                    }
                    (AccColumn::IntegerSum { sums, counts }, DataType::Int32) => {
                        Arc::new(Int32Array::from_iter(
                            sums.iter()
                                .zip(counts)
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
                        AccColumn::IntegerMin { values, present }
                        | AccColumn::IntegerMax { values, present },
                        DataType::Int64,
                    ) => Arc::new(Int64Array::from_iter(
                        values.iter().zip(present).map(|(v, p)| p.then_some(*v)),
                    )),
                    (
                        AccColumn::IntegerMin { values, present }
                        | AccColumn::IntegerMax { values, present },
                        DataType::Int32,
                    ) => Arc::new(Int32Array::from_iter(
                        values
                            .iter()
                            .zip(present)
                            .map(|(v, p)| p.then_some(*v as i32)),
                    )),
                    (AccColumn::Float { sums, counts, avg }, DataType::Float64) => {
                        Arc::new(Float64Array::from_iter(sums.iter().zip(counts).map(
                            |(s, c)| (*c > 0).then(|| if *avg { s / *c as f64 } else { *s }),
                        )))
                    }
                    (
                        AccColumn::FloatMin { values, present }
                        | AccColumn::FloatMax { values, present },
                        DataType::Float64,
                    ) => Arc::new(Float64Array::from_iter(
                        values.iter().zip(present).map(|(v, p)| p.then_some(*v)),
                    )),
                    (AccColumn::TextMin(values) | AccColumn::TextMax(values), DataType::Utf8) => {
                        Arc::new(StringArray::from_iter(values.iter().map(|v| v.as_deref())))
                    }
                    (
                        AccColumn::TextMin(values) | AccColumn::TextMax(values),
                        DataType::LargeUtf8,
                    ) => Arc::new(arrow::array::LargeStringArray::from_iter(
                        values.iter().map(|v| v.as_deref()),
                    )),
                    (_, other) => {
                        return Err(exec_err(format!(
                            "columnar aggregate cannot produce {other}"
                        )));
                    }
                })
            })
            .collect()
    }
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| exec_err("partial group key length overflow"))?;
    let word = bytes
        .get(*offset..end)
        .ok_or_else(|| exec_err("truncated partial group key"))?;
    *offset = end;
    Ok(u64::from_le_bytes(word.try_into().expect("eight bytes")))
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
}
