//! Mergeable summaries a table's statistics carry: a HyperLogLog sketch for
//! distinct counts and a KLL sketch for quantiles. Both are deterministic,
//! bounded in memory, and serialise to compact bytes.
//!
//! The HyperLogLog register layout is the one the DLM's sketch cuboids use
//! (`api/dlm/hll.py`): the top `p` bits of a 64-bit hash select the
//! register and the register keeps the 1-based position of the first set
//! bit among the remaining `64 - p` bits (`64 - p + 1` when none is set).
//! The hash is a port of PostgreSQL's `hash_bytes_extended` with seed 0 —
//! what `hashtextextended(CAST(v AS text), 0)` computes — over the value's
//! canonical text ([`hash_text`]), so a sketch built here and one the DLM
//! extracted in SQL over the same column at the same precision hold the
//! same registers and merge by element-wise maximum.

use crate::statistics::{StatValue, decimal_text};
use arrow::array::{Array, ArrayRef, ArrowPrimitiveType, AsArray, PrimitiveArray};
use arrow::datatypes::{
    DataType, Date32Type, Decimal128Type, Float32Type, Float64Type, Int8Type, Int16Type, Int32Type,
    Int64Type, TimeUnit, TimestampMicrosecondType, TimestampMillisecondType,
    TimestampNanosecondType, TimestampSecondType, UInt8Type, UInt16Type, UInt32Type, UInt64Type,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The lowest register precision a sketch accepts (2 048 registers).
pub const HLL_MIN_PRECISION: u8 = 11;
/// The highest register precision a sketch accepts (16 384 registers).
pub const HLL_MAX_PRECISION: u8 = 14;
/// The precision table statistics use: 4 096 registers, a standard error
/// of 1.04 / √4096 ≈ 1.6 %.
pub const HLL_DEFAULT_PRECISION: u8 = 12;
/// A sparse sketch becomes dense once its entries would occupy more than the
/// packed dense registers do (6 bits per register).
const SPARSE_ENTRY_BYTES: usize = 3;
const HLL_MAGIC: u8 = 0x4b;
const HLL_VERSION: u8 = 1;
const HLL_MODE_SPARSE: u8 = 0;
const HLL_MODE_DENSE: u8 = 1;

/// A HyperLogLog sketch of the distinct values a column holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HllSketch {
    precision: u8,
    registers: Registers,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Registers {
    /// Register index → value, for the registers that are set.
    Sparse(BTreeMap<u16, u8>),
    /// One byte per register.
    Dense(Vec<u8>),
}

impl HllSketch {
    /// An empty sketch at `precision` bits (2^precision registers).
    pub fn new(precision: u8) -> crate::Result<Self> {
        if !(HLL_MIN_PRECISION..=HLL_MAX_PRECISION).contains(&precision) {
            return Err(crate::KaveonError::Execution(format!(
                "HyperLogLog precision must be between {HLL_MIN_PRECISION} and {HLL_MAX_PRECISION}, not {precision}"
            )));
        }
        Ok(Self {
            precision,
            registers: Registers::Sparse(BTreeMap::new()),
        })
    }

    /// The sketch at the statistics precision.
    pub fn default_precision() -> Self {
        Self::new(HLL_DEFAULT_PRECISION).expect("the default precision is valid")
    }

    pub const fn precision(&self) -> u8 {
        self.precision
    }

    /// The number of registers.
    pub fn register_count(&self) -> usize {
        1usize << self.precision
    }

    /// The relative standard error of the estimate at this precision,
    /// 1.04 / √m over m registers: 1.6 % at the statistics precision.
    pub fn standard_error(&self) -> f64 {
        1.04 / (self.register_count() as f64).sqrt()
    }

    /// Bytes this sketch holds in memory, for accounting.
    pub fn memory_bytes(&self) -> usize {
        match &self.registers {
            Registers::Sparse(entries) => entries.len() * 32 + 64,
            Registers::Dense(values) => values.len() + 64,
        }
    }

    /// Fold a value's canonical text in.
    pub fn insert_text(&mut self, text: &str) {
        self.insert_hash(pg_hash_bytes_extended(text.as_bytes(), 0));
    }

    /// Fold a 64-bit hash in.
    pub fn insert_hash(&mut self, hash: u64) {
        let index = (hash >> (64 - u32::from(self.precision))) as u16;
        let remaining = u32::from(self.precision);
        let rest = hash << remaining;
        let rho = if rest == 0 {
            (64 - self.precision) + 1
        } else {
            (rest.leading_zeros() + 1) as u8
        };
        self.set_register(index, rho);
    }

    fn set_register(&mut self, index: u16, rho: u8) {
        match &mut self.registers {
            Registers::Sparse(entries) => {
                let entry = entries.entry(index).or_insert(0);
                if rho > *entry {
                    *entry = rho;
                }
                if entries.len() * SPARSE_ENTRY_BYTES > self.register_count() * 6 / 8 {
                    self.densify();
                }
            }
            Registers::Dense(values) => {
                let slot = &mut values[usize::from(index)];
                if rho > *slot {
                    *slot = rho;
                }
            }
        }
    }

    fn densify(&mut self) {
        if let Registers::Sparse(entries) = &self.registers {
            let mut values = vec![0u8; self.register_count()];
            for (index, rho) in entries {
                values[usize::from(*index)] = *rho;
            }
            self.registers = Registers::Dense(values);
        }
    }

    /// Union `other` into this sketch: the element-wise register maximum.
    pub fn merge(&mut self, other: &HllSketch) -> crate::Result<()> {
        if other.precision != self.precision {
            return Err(crate::KaveonError::Execution(format!(
                "cannot merge HyperLogLog sketches of precision {} and {}",
                self.precision, other.precision
            )));
        }
        match &other.registers {
            Registers::Sparse(entries) => {
                for (index, rho) in entries {
                    self.set_register(*index, *rho);
                }
            }
            Registers::Dense(values) => {
                self.densify();
                let Registers::Dense(mine) = &mut self.registers else {
                    unreachable!("densified above");
                };
                for (slot, value) in mine.iter_mut().zip(values) {
                    if *value > *slot {
                        *slot = *value;
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether no value was folded in.
    pub fn is_empty(&self) -> bool {
        match &self.registers {
            Registers::Sparse(entries) => entries.is_empty(),
            Registers::Dense(values) => values.iter().all(|value| *value == 0),
        }
    }

    /// The estimated distinct count. The estimator is Ertl's improved raw
    /// estimator ("New cardinality estimation algorithms for HyperLogLog
    /// sketches", 2017): unbiased over the whole range without empirical
    /// bias tables, exact linear counting at the low end included.
    pub fn estimate(&self) -> u64 {
        let m = self.register_count();
        let q = 64 - u32::from(self.precision);
        let mut counts = vec![0u64; q as usize + 2];
        match &self.registers {
            Registers::Sparse(entries) => {
                counts[0] = (m - entries.len()) as u64;
                for rho in entries.values() {
                    counts[usize::from(*rho)] += 1;
                }
            }
            Registers::Dense(values) => {
                for rho in values {
                    counts[usize::from(*rho)] += 1;
                }
            }
        }
        let m_f = m as f64;
        let mut z = m_f * tau(1.0 - counts[q as usize + 1] as f64 / m_f);
        for k in (1..=q as usize).rev() {
            z += counts[k] as f64;
            z *= 0.5;
        }
        z += m_f * sigma(counts[0] as f64 / m_f);
        let alpha_inf = 1.0 / (2.0 * std::f64::consts::LN_2);
        let estimate = alpha_inf * m_f * m_f / z;
        if !estimate.is_finite() {
            return 0;
        }
        estimate.round() as u64
    }

    /// The compact encoding: a header, then either the sorted sparse entries
    /// (`u16` index, `u8` value) or the dense registers packed six bits each.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(HLL_MAGIC);
        out.push(HLL_VERSION);
        out.push(self.precision);
        match &self.registers {
            Registers::Sparse(entries) => {
                out.push(HLL_MODE_SPARSE);
                out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                for (index, rho) in entries {
                    out.extend_from_slice(&index.to_le_bytes());
                    out.push(*rho);
                }
            }
            Registers::Dense(values) => {
                out.push(HLL_MODE_DENSE);
                out.extend(pack6(values));
            }
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let malformed = |what: &str| {
            crate::KaveonError::Execution(format!("HyperLogLog sketch bytes are malformed: {what}"))
        };
        if bytes.len() < 4 || bytes[0] != HLL_MAGIC || bytes[1] != HLL_VERSION {
            return Err(malformed("header"));
        }
        let precision = bytes[2];
        let mut sketch = Self::new(precision)?;
        let max_rho = 64 - precision + 1;
        match bytes[3] {
            HLL_MODE_SPARSE => {
                let count = u32::from_le_bytes(
                    bytes
                        .get(4..8)
                        .ok_or_else(|| malformed("sparse count"))?
                        .try_into()
                        .unwrap(),
                ) as usize;
                let body = bytes.get(8..).ok_or_else(|| malformed("sparse body"))?;
                if body.len() != count * SPARSE_ENTRY_BYTES {
                    return Err(malformed("sparse length"));
                }
                for entry in body.as_chunks::<SPARSE_ENTRY_BYTES>().0 {
                    let index = u16::from_le_bytes([entry[0], entry[1]]);
                    let rho = entry[2];
                    if usize::from(index) >= sketch.register_count() || rho == 0 || rho > max_rho {
                        return Err(malformed("sparse entry"));
                    }
                    sketch.set_register(index, rho);
                }
            }
            HLL_MODE_DENSE => {
                let values = unpack6(&bytes[4..], sketch.register_count())
                    .ok_or_else(|| malformed("dense length"))?;
                if values.iter().any(|rho| *rho > max_rho) {
                    return Err(malformed("dense register"));
                }
                sketch.registers = Registers::Dense(values);
            }
            _ => return Err(malformed("mode")),
        }
        Ok(sketch)
    }
}

impl Serialize for HllSketch {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64_encode(&self.to_bytes()))
    }
}

impl<'de> Deserialize<'de> for HllSketch {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes =
            base64_decode(&text).ok_or_else(|| serde::de::Error::custom("sketch is not base64"))?;
        HllSketch::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// σ(x) = x + Σ_{k≥1} x^(2^k) · 2^(k−1); +∞ at x = 1.
fn sigma(x: f64) -> f64 {
    if x == 1.0 {
        return f64::INFINITY;
    }
    let mut x = x;
    let mut y = 1.0;
    let mut z = x;
    loop {
        x *= x;
        let next = z + x * y;
        if next == z {
            return z;
        }
        z = next;
        y *= 2.0;
    }
}

/// τ(x) = (1/3)(1 − x − Σ_{k≥1} (1 − x^(2^−k))² · 2^−k); 0 at x ∈ {0, 1}.
fn tau(x: f64) -> f64 {
    if x == 0.0 || x == 1.0 {
        return 0.0;
    }
    let mut x = x;
    let mut y = 1.0;
    let mut z = 1.0 - x;
    loop {
        x = x.sqrt();
        let previous = z;
        y *= 0.5;
        z -= (1.0 - x).powi(2) * y;
        if previous == z {
            return z / 3.0;
        }
    }
}

fn pack6(values: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 6 / 8 + 1);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for value in values {
        acc = (acc << 6) | u32::from(*value & 0x3f);
        bits += 6;
        while bits >= 8 {
            out.push((acc >> (bits - 8)) as u8);
            bits -= 8;
            acc &= (1 << bits) - 1;
        }
    }
    if bits > 0 {
        out.push((acc << (8 - bits)) as u8);
    }
    out
}

fn unpack6(bytes: &[u8], count: usize) -> Option<Vec<u8>> {
    if bytes.len() != (count * 6).div_ceil(8) {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for byte in bytes {
        acc = (acc << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 6 && out.len() < count {
            out.push(((acc >> (bits - 6)) & 0x3f) as u8);
            bits -= 6;
            acc &= (1 << bits) - 1;
        }
    }
    (out.len() == count).then_some(out)
}

/// PostgreSQL's `hash_bytes_extended` (Bob Jenkins' lookup3 as PostgreSQL
/// applies it, little-endian): what `hashtextextended(text, seed)` returns
/// for the text's bytes. The DLM extracts its HyperLogLog registers with it.
pub fn pg_hash_bytes_extended(key: &[u8], seed: u64) -> u64 {
    #[inline(always)]
    fn rot(x: u32, k: u32) -> u32 {
        x.rotate_left(k)
    }
    #[inline(always)]
    fn mix(a: &mut u32, b: &mut u32, c: &mut u32) {
        *a = a.wrapping_sub(*c);
        *a ^= rot(*c, 4);
        *c = c.wrapping_add(*b);
        *b = b.wrapping_sub(*a);
        *b ^= rot(*a, 6);
        *a = a.wrapping_add(*c);
        *c = c.wrapping_sub(*b);
        *c ^= rot(*b, 8);
        *b = b.wrapping_add(*a);
        *a = a.wrapping_sub(*c);
        *a ^= rot(*c, 16);
        *c = c.wrapping_add(*b);
        *b = b.wrapping_sub(*a);
        *b ^= rot(*a, 19);
        *a = a.wrapping_add(*c);
        *c = c.wrapping_sub(*b);
        *c ^= rot(*b, 4);
        *b = b.wrapping_add(*a);
    }
    #[inline(always)]
    fn fin(a: &mut u32, b: &mut u32, c: &mut u32) {
        *c ^= *b;
        *c = c.wrapping_sub(rot(*b, 14));
        *a ^= *c;
        *a = a.wrapping_sub(rot(*c, 11));
        *b ^= *a;
        *b = b.wrapping_sub(rot(*a, 25));
        *c ^= *b;
        *c = c.wrapping_sub(rot(*b, 16));
        *a ^= *c;
        *a = a.wrapping_sub(rot(*c, 4));
        *b ^= *a;
        *b = b.wrapping_sub(rot(*a, 14));
        *c ^= *b;
        *c = c.wrapping_sub(rot(*b, 24));
    }
    let word = |bytes: &[u8]| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let len = key.len() as u32;
    let mut a = 0x9e37_79b9u32.wrapping_add(len).wrapping_add(3_923_095);
    let mut b = a;
    let mut c = a;
    if seed != 0 {
        a = a.wrapping_add((seed >> 32) as u32);
        b = b.wrapping_add(seed as u32);
        mix(&mut a, &mut b, &mut c);
    }
    let mut k = key;
    while k.len() >= 12 {
        a = a.wrapping_add(word(&k[0..4]));
        b = b.wrapping_add(word(&k[4..8]));
        c = c.wrapping_add(word(&k[8..12]));
        mix(&mut a, &mut b, &mut c);
        k = &k[12..];
    }
    let byte = |i: usize| u32::from(k[i]);
    match k.len() {
        11 => {
            c = c
                .wrapping_add(byte(10) << 24)
                .wrapping_add(byte(9) << 16)
                .wrapping_add(byte(8) << 8);
            b = b.wrapping_add(word(&k[4..8]));
            a = a.wrapping_add(word(&k[0..4]));
        }
        10 => {
            c = c.wrapping_add(byte(9) << 16).wrapping_add(byte(8) << 8);
            b = b.wrapping_add(word(&k[4..8]));
            a = a.wrapping_add(word(&k[0..4]));
        }
        9 => {
            c = c.wrapping_add(byte(8) << 8);
            b = b.wrapping_add(word(&k[4..8]));
            a = a.wrapping_add(word(&k[0..4]));
        }
        8 => {
            b = b.wrapping_add(word(&k[4..8]));
            a = a.wrapping_add(word(&k[0..4]));
        }
        7 => {
            b = b
                .wrapping_add(byte(6) << 16)
                .wrapping_add(byte(5) << 8)
                .wrapping_add(byte(4));
            a = a.wrapping_add(word(&k[0..4]));
        }
        6 => {
            b = b.wrapping_add(byte(5) << 8).wrapping_add(byte(4));
            a = a.wrapping_add(word(&k[0..4]));
        }
        5 => {
            b = b.wrapping_add(byte(4));
            a = a.wrapping_add(word(&k[0..4]));
        }
        4 => {
            a = a.wrapping_add(word(&k[0..4]));
        }
        3 => {
            a = a
                .wrapping_add(byte(2) << 16)
                .wrapping_add(byte(1) << 8)
                .wrapping_add(byte(0));
        }
        2 => {
            a = a.wrapping_add(byte(1) << 8).wrapping_add(byte(0));
        }
        1 => {
            a = a.wrapping_add(byte(0));
        }
        _ => {}
    }
    fin(&mut a, &mut b, &mut c);
    (u64::from(b) << 32) | u64::from(c)
}

/// The canonical text of a value for hashing: what `CAST(v AS text)` renders
/// in PostgreSQL for integers, booleans, dates and exact decimals, so the
/// sketches agree with the DLM's on those types. Doubles render with Rust's
/// shortest round-trip form and timestamps as ISO 8601; the sketch is
/// self-consistent on those but not byte-identical to PostgreSQL's text.
pub fn hash_text(text: &str) -> u64 {
    pg_hash_bytes_extended(text.as_bytes(), 0)
}

/// A KLL sketch (Karnin, Lang and Liberty, 2016) of a numeric column's
/// distribution: `k` controls the space and the rank error (k = 200 keeps
/// the rank error near 1.3 % with high probability). Values are `f64`;
/// integers, dates and timestamps are folded in as their numeric
/// representation. Compaction offsets come from a deterministic generator
/// seeded by the sketch, so the same input yields the same sketch.
#[derive(Clone, Debug, PartialEq)]
pub struct KllSketch {
    k: u16,
    n: u64,
    min: f64,
    max: f64,
    /// `levels[0]` holds weight-1 items, `levels[h]` weight 2^h items.
    levels: Vec<Vec<f64>>,
    rng: u64,
}

pub const KLL_DEFAULT_K: u16 = 200;
const KLL_MIN_K: u16 = 8;
const KLL_MAX_K: u16 = 65_535;
const KLL_MAGIC: u8 = 0x4c;
const KLL_VERSION: u8 = 1;

impl KllSketch {
    pub fn new(k: u16) -> crate::Result<Self> {
        if !(KLL_MIN_K..=KLL_MAX_K).contains(&k) {
            return Err(crate::KaveonError::Execution(format!(
                "KLL k must be between {KLL_MIN_K} and {KLL_MAX_K}, not {k}"
            )));
        }
        Ok(Self {
            k,
            n: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            levels: vec![Vec::new()],
            rng: 0x9e37_79b9_7f4a_7c15 ^ u64::from(k),
        })
    }

    pub fn default_k() -> Self {
        Self::new(KLL_DEFAULT_K).expect("the default k is valid")
    }

    pub const fn k(&self) -> u16 {
        self.k
    }

    /// The normalized rank error bound at this `k` for one quantile:
    /// 2.446 / k^0.9433, the empirical bound of the KLL compaction scheme
    /// this sketch implements (level capacities k·(2/3)^depth, random
    /// compaction offsets), holding with about 99 % confidence — 1.65 % at
    /// the default k. The value at fraction p is one whose true rank is
    /// within this of p.
    pub fn rank_error(&self) -> f64 {
        2.446 / f64::from(self.k).powf(0.9433)
    }

    /// The number of values folded in.
    pub const fn count(&self) -> u64 {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn min(&self) -> Option<f64> {
        (self.n > 0).then_some(self.min)
    }

    pub fn max(&self) -> Option<f64> {
        (self.n > 0).then_some(self.max)
    }

    pub fn memory_bytes(&self) -> usize {
        self.levels
            .iter()
            .map(|level| level.capacity() * 8 + 24)
            .sum::<usize>()
            + 64
    }

    /// Fold a value in; NaN is ignored.
    pub fn update(&mut self, value: f64) {
        if value.is_nan() {
            return;
        }
        self.n += 1;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        self.levels[0].push(value);
        if self.total_items() > self.capacity() {
            self.compress();
        }
    }

    fn total_items(&self) -> usize {
        self.levels.iter().map(Vec::len).sum()
    }

    fn level_capacity(&self, level: usize, num_levels: usize) -> usize {
        let depth = (num_levels - 1 - level) as i32;
        let capacity = f64::from(self.k) * (2.0f64 / 3.0).powi(depth);
        (capacity.ceil() as usize).max(2)
    }

    fn capacity(&self) -> usize {
        let num_levels = self.levels.len();
        (0..num_levels)
            .map(|level| self.level_capacity(level, num_levels))
            .sum()
    }

    fn next_bit(&mut self) -> bool {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 63) == 1
    }

    /// Compact the lowest level at its capacity: sort it, move every second
    /// item (from a random offset) one level up with doubled weight and
    /// discard the rest.
    fn compress(&mut self) {
        while self.total_items() > self.capacity() {
            let num_levels = self.levels.len();
            let Some(level) = (0..num_levels)
                .find(|level| self.levels[*level].len() >= self.level_capacity(*level, num_levels))
            else {
                return;
            };
            if level + 1 == self.levels.len() {
                self.levels.push(Vec::new());
            }
            let offset = usize::from(self.next_bit());
            let mut items = std::mem::take(&mut self.levels[level]);
            items.sort_by(f64::total_cmp);
            let promoted = items
                .into_iter()
                .enumerate()
                .filter(|(index, _)| index % 2 == offset)
                .map(|(_, value)| value);
            self.levels[level + 1].extend(promoted);
        }
    }

    /// Union `other` in: the sketches' levels concatenate and compaction
    /// restores the bound.
    pub fn merge(&mut self, other: &KllSketch) -> crate::Result<()> {
        if other.k != self.k {
            return Err(crate::KaveonError::Execution(format!(
                "cannot merge KLL sketches of k {} and {}",
                self.k, other.k
            )));
        }
        if other.n == 0 {
            return Ok(());
        }
        while self.levels.len() < other.levels.len() {
            self.levels.push(Vec::new());
        }
        for (level, items) in other.levels.iter().enumerate() {
            self.levels[level].extend_from_slice(items);
        }
        self.n += other.n;
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
        self.compress();
        Ok(())
    }

    /// The (value, weight) pairs sorted by value.
    fn weighted(&self) -> Vec<(f64, u64)> {
        let mut items = Vec::with_capacity(self.total_items());
        for (level, values) in self.levels.iter().enumerate() {
            let weight = 1u64 << level;
            items.extend(values.iter().map(|value| (*value, weight)));
        }
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        items
    }

    /// The estimated fraction of values at or below `value` (0..=1).
    pub fn rank(&self, value: f64) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        let below = self
            .weighted()
            .into_iter()
            .filter(|(item, _)| *item <= value)
            .map(|(_, weight)| weight)
            .sum::<u64>();
        below as f64 / self.n as f64
    }

    /// The estimated value at `fraction` (0..=1) of the distribution; the
    /// exact minimum and maximum at the ends.
    pub fn quantile(&self, fraction: f64) -> Option<f64> {
        if self.n == 0 {
            return None;
        }
        if fraction <= 0.0 {
            return Some(self.min);
        }
        if fraction >= 1.0 {
            return Some(self.max);
        }
        let target = (fraction * self.n as f64).ceil().max(1.0) as u64;
        let mut cumulative = 0;
        for (value, weight) in self.weighted() {
            cumulative += weight;
            if cumulative >= target {
                return Some(value.clamp(self.min, self.max));
            }
        }
        Some(self.max)
    }

    /// The values at the given fractions.
    pub fn quantiles(&self, fractions: &[f64]) -> Vec<Option<f64>> {
        fractions
            .iter()
            .map(|fraction| self.quantile(*fraction))
            .collect()
    }

    /// The estimated fraction of values in `[low, high]`, with an open or
    /// closed end as asked: what a range predicate's selectivity needs.
    pub fn fraction_between(&self, low: Option<(f64, bool)>, high: Option<(f64, bool)>) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        let items = self.weighted();
        let mut inside = 0u64;
        for (value, weight) in items {
            let above_low = match low {
                Some((bound, inclusive)) => {
                    if inclusive {
                        value >= bound
                    } else {
                        value > bound
                    }
                }
                None => true,
            };
            let below_high = match high {
                Some((bound, inclusive)) => {
                    if inclusive {
                        value <= bound
                    } else {
                        value < bound
                    }
                }
                None => true,
            };
            if above_low && below_high {
                inside += weight;
            }
        }
        inside as f64 / self.n as f64
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(KLL_MAGIC);
        out.push(KLL_VERSION);
        out.extend_from_slice(&self.k.to_le_bytes());
        out.extend_from_slice(&self.n.to_le_bytes());
        out.extend_from_slice(&self.min.to_le_bytes());
        out.extend_from_slice(&self.max.to_le_bytes());
        out.extend_from_slice(&self.rng.to_le_bytes());
        out.push(self.levels.len() as u8);
        for level in &self.levels {
            out.extend_from_slice(&(level.len() as u32).to_le_bytes());
            for value in level {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let malformed = |what: &str| {
            crate::KaveonError::Execution(format!("KLL sketch bytes are malformed: {what}"))
        };
        let mut cursor = Cursor { bytes, at: 0 };
        if cursor.u8().ok_or_else(|| malformed("magic"))? != KLL_MAGIC
            || cursor.u8().ok_or_else(|| malformed("version"))? != KLL_VERSION
        {
            return Err(malformed("header"));
        }
        let k = cursor.u16().ok_or_else(|| malformed("k"))?;
        let mut sketch = Self::new(k)?;
        sketch.n = cursor.u64().ok_or_else(|| malformed("count"))?;
        sketch.min = cursor.f64().ok_or_else(|| malformed("min"))?;
        sketch.max = cursor.f64().ok_or_else(|| malformed("max"))?;
        sketch.rng = cursor.u64().ok_or_else(|| malformed("generator"))?;
        let num_levels = usize::from(cursor.u8().ok_or_else(|| malformed("levels"))?);
        if num_levels == 0 || num_levels > 64 {
            return Err(malformed("level count"));
        }
        let mut levels = Vec::with_capacity(num_levels);
        let mut items = 0u64;
        for _ in 0..num_levels {
            let count = cursor.u32().ok_or_else(|| malformed("level length"))? as usize;
            let mut level = Vec::with_capacity(count.min(1 << 16));
            for _ in 0..count {
                let value = cursor.f64().ok_or_else(|| malformed("value"))?;
                if value.is_nan() {
                    return Err(malformed("NaN value"));
                }
                level.push(value);
            }
            items += (count as u64) << (levels.len() as u64).min(63);
            levels.push(level);
        }
        if cursor.at != bytes.len() || (sketch.n == 0) != (items == 0) {
            return Err(malformed("trailing bytes or count"));
        }
        sketch.levels = levels;
        Ok(sketch)
    }
}

impl Serialize for KllSketch {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64_encode(&self.to_bytes()))
    }
}

impl<'de> Deserialize<'de> for KllSketch {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes =
            base64_decode(&text).ok_or_else(|| serde::de::Error::custom("sketch is not base64"))?;
        KllSketch::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let slice = self.bytes.get(self.at..self.at + n)?;
        self.at += n;
        Some(slice)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn f64(&mut self) -> Option<f64> {
        self.u64().map(f64::from_bits)
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(BASE64[(n >> 18) as usize & 63] as char);
        out.push(BASE64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            BASE64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || chunk[..4 - pad].contains(&b'=') {
            return None;
        }
        let mut n = 0u32;
        for c in &chunk[..4 - pad] {
            n = (n << 6) | value(*c)?;
        }
        n <<= 6 * pad as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// Whether a column of this type can be folded into a distinct-count
/// sketch: booleans, integers, floats, text, dates, timestamps, decimals,
/// and dictionaries over them.
pub fn sketchable(data_type: &DataType) -> bool {
    matches!(
        logical_type(data_type),
        DataType::Boolean
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float32
            | DataType::Float64
            | DataType::Utf8
            | DataType::LargeUtf8
            | DataType::Date32
            | DataType::Timestamp(_, _)
            | DataType::Decimal128(_, _)
    )
}

/// Whether a sketchable column's values sit on a number line for a
/// quantile sketch: everything sketchable but booleans and text.
pub fn quantile_sketchable(data_type: &DataType) -> bool {
    sketchable(data_type)
        && !matches!(
            logical_type(data_type),
            DataType::Boolean | DataType::Utf8 | DataType::LargeUtf8
        )
}

fn logical_type(data_type: &DataType) -> &DataType {
    match data_type {
        DataType::Dictionary(_, values) => values.as_ref(),
        other => other,
    }
}

/// A dictionary array as its values, any other array as itself.
fn plain(array: &ArrayRef) -> crate::Result<ArrayRef> {
    Ok(match array.data_type() {
        DataType::Dictionary(_, values) => arrow::compute::cast(array, values)?,
        _ => Arc::clone(array),
    })
}

/// Fold every non-null value of `array` into the distinct-count sketch,
/// hashed over the value's canonical text — the text `ANALYZE` hashes, so
/// a sketch built while a statement runs merges with a stored one.
pub fn fold_distinct(array: &ArrayRef, sketch: &mut HllSketch) -> crate::Result<()> {
    let array = plain(array)?;
    if array.null_count() == array.len() {
        return Ok(());
    }
    let mut text = String::new();
    macro_rules! fold_primitive {
        ($values:expr, $fmt:expr) => {{
            for value in $values.iter().flatten() {
                text.clear();
                $fmt(&mut text, value);
                sketch.insert_hash(pg_hash_bytes_extended(text.as_bytes(), 0));
            }
        }};
    }
    match array.data_type() {
        DataType::Boolean => {
            for value in array.as_boolean().iter().flatten() {
                sketch.insert_hash(pg_hash_bytes_extended(
                    if value { b"true" } else { b"false" },
                    0,
                ));
            }
        }
        DataType::Int8 => fold_primitive!(array.as_primitive::<Int8Type>(), write_display),
        DataType::Int16 => fold_primitive!(array.as_primitive::<Int16Type>(), write_display),
        DataType::Int32 => fold_primitive!(array.as_primitive::<Int32Type>(), write_display),
        DataType::Int64 => fold_primitive!(array.as_primitive::<Int64Type>(), write_display),
        DataType::UInt8 => fold_primitive!(array.as_primitive::<UInt8Type>(), write_display),
        DataType::UInt16 => fold_primitive!(array.as_primitive::<UInt16Type>(), write_display),
        DataType::UInt32 => fold_primitive!(array.as_primitive::<UInt32Type>(), write_display),
        DataType::UInt64 => fold_primitive!(array.as_primitive::<UInt64Type>(), write_display),
        DataType::Float32 => fold_primitive!(
            array.as_primitive::<Float32Type>(),
            |text: &mut String, value: f32| write_display(text, f64::from(value))
        ),
        DataType::Float64 => fold_primitive!(array.as_primitive::<Float64Type>(), write_display),
        DataType::Date32 => fold_primitive!(
            array.as_primitive::<Date32Type>(),
            |text: &mut String, value: i32| text.push_str(&StatValue::Date(value).to_hash_text())
        ),
        DataType::Timestamp(unit, zone) => {
            let (unit, utc) = (*unit, zone.is_some());
            for value in timestamp_values(&array, unit).iter().flatten() {
                text.clear();
                text.push_str(&StatValue::Timestamp { value, unit, utc }.to_hash_text());
                sketch.insert_hash(pg_hash_bytes_extended(text.as_bytes(), 0));
            }
        }
        DataType::Decimal128(_, scale) => {
            let scale = *scale;
            fold_primitive!(
                array.as_primitive::<Decimal128Type>(),
                |text: &mut String, value: i128| text.push_str(&decimal_text(value, scale))
            )
        }
        DataType::Utf8 => {
            for value in array.as_string::<i32>().iter().flatten() {
                sketch.insert_hash(pg_hash_bytes_extended(value.as_bytes(), 0));
            }
        }
        DataType::LargeUtf8 => {
            for value in array.as_string::<i64>().iter().flatten() {
                sketch.insert_hash(pg_hash_bytes_extended(value.as_bytes(), 0));
            }
        }
        other => return Err(unsketchable(other)),
    }
    Ok(())
}

/// Fold one row of `array` into the distinct-count sketch, as
/// [`fold_distinct`] would; a null row folds nothing. `text` is scratch
/// the caller keeps between rows.
pub fn fold_distinct_row(
    array: &ArrayRef,
    row: usize,
    sketch: &mut HllSketch,
    text: &mut String,
) -> crate::Result<()> {
    if array.is_null(row) {
        return Ok(());
    }
    if let DataType::Dictionary(key, _) = array.data_type() {
        if key.as_ref() != &DataType::Int32 {
            return Err(unsketchable(array.data_type()));
        }
        let dictionary = array.as_dictionary::<Int32Type>();
        let index = dictionary.keys().value(row) as usize;
        return fold_distinct_row(dictionary.values(), index, sketch, text);
    }
    text.clear();
    let bytes: &[u8] = match array.data_type() {
        DataType::Boolean => {
            if array.as_boolean().value(row) {
                b"true"
            } else {
                b"false"
            }
        }
        DataType::Int8 => display_row(array.as_primitive::<Int8Type>(), row, text),
        DataType::Int16 => display_row(array.as_primitive::<Int16Type>(), row, text),
        DataType::Int32 => display_row(array.as_primitive::<Int32Type>(), row, text),
        DataType::Int64 => display_row(array.as_primitive::<Int64Type>(), row, text),
        DataType::UInt8 => display_row(array.as_primitive::<UInt8Type>(), row, text),
        DataType::UInt16 => display_row(array.as_primitive::<UInt16Type>(), row, text),
        DataType::UInt32 => display_row(array.as_primitive::<UInt32Type>(), row, text),
        DataType::UInt64 => display_row(array.as_primitive::<UInt64Type>(), row, text),
        DataType::Float32 => {
            write_display(
                text,
                f64::from(array.as_primitive::<Float32Type>().value(row)),
            );
            text.as_bytes()
        }
        DataType::Float64 => display_row(array.as_primitive::<Float64Type>(), row, text),
        DataType::Date32 => {
            text.push_str(
                &StatValue::Date(array.as_primitive::<Date32Type>().value(row)).to_hash_text(),
            );
            text.as_bytes()
        }
        DataType::Timestamp(unit, zone) => {
            let value = timestamp_values(array, *unit).value(row);
            text.push_str(
                &StatValue::Timestamp {
                    value,
                    unit: *unit,
                    utc: zone.is_some(),
                }
                .to_hash_text(),
            );
            text.as_bytes()
        }
        DataType::Decimal128(_, scale) => {
            text.push_str(&decimal_text(
                array.as_primitive::<Decimal128Type>().value(row),
                *scale,
            ));
            text.as_bytes()
        }
        DataType::Utf8 => array.as_string::<i32>().value(row).as_bytes(),
        DataType::LargeUtf8 => array.as_string::<i64>().value(row).as_bytes(),
        other => return Err(unsketchable(other)),
    };
    sketch.insert_hash(pg_hash_bytes_extended(bytes, 0));
    Ok(())
}

/// Fold every non-null value of `array` into the quantile sketch as its
/// number: integers and floats as they are, decimals scaled, dates as
/// days, timestamps in their unit.
pub fn fold_quantiles(array: &ArrayRef, sketch: &mut KllSketch) -> crate::Result<()> {
    let array = plain(array)?;
    if array.null_count() == array.len() {
        return Ok(());
    }
    macro_rules! fold_primitive {
        ($values:expr, $to_f64:expr) => {{
            for value in $values.iter().flatten() {
                sketch.update($to_f64(value));
            }
        }};
    }
    match array.data_type() {
        DataType::Int8 => fold_primitive!(array.as_primitive::<Int8Type>(), f64::from),
        DataType::Int16 => fold_primitive!(array.as_primitive::<Int16Type>(), f64::from),
        DataType::Int32 => fold_primitive!(array.as_primitive::<Int32Type>(), f64::from),
        DataType::Int64 => fold_primitive!(array.as_primitive::<Int64Type>(), |v: i64| v as f64),
        DataType::UInt8 => fold_primitive!(array.as_primitive::<UInt8Type>(), f64::from),
        DataType::UInt16 => fold_primitive!(array.as_primitive::<UInt16Type>(), f64::from),
        DataType::UInt32 => fold_primitive!(array.as_primitive::<UInt32Type>(), f64::from),
        DataType::UInt64 => fold_primitive!(array.as_primitive::<UInt64Type>(), |v: u64| v as f64),
        DataType::Float32 => fold_primitive!(array.as_primitive::<Float32Type>(), f64::from),
        DataType::Float64 => fold_primitive!(array.as_primitive::<Float64Type>(), |v: f64| v),
        DataType::Date32 => fold_primitive!(array.as_primitive::<Date32Type>(), f64::from),
        DataType::Timestamp(unit, _) => {
            for value in timestamp_values(&array, *unit).iter().flatten() {
                sketch.update(value as f64);
            }
        }
        DataType::Decimal128(_, scale) => {
            let divisor = 10f64.powi(i32::from(*scale));
            fold_primitive!(array.as_primitive::<Decimal128Type>(), |v: i128| v as f64
                / divisor)
        }
        other => return Err(unsketchable(other)),
    }
    Ok(())
}

/// Fold one row of `array` into the quantile sketch, as [`fold_quantiles`]
/// would; a null row folds nothing.
pub fn fold_quantiles_row(
    array: &ArrayRef,
    row: usize,
    sketch: &mut KllSketch,
) -> crate::Result<()> {
    if array.is_null(row) {
        return Ok(());
    }
    if let DataType::Dictionary(key, _) = array.data_type() {
        if key.as_ref() != &DataType::Int32 {
            return Err(unsketchable(array.data_type()));
        }
        let dictionary = array.as_dictionary::<Int32Type>();
        let index = dictionary.keys().value(row) as usize;
        return fold_quantiles_row(dictionary.values(), index, sketch);
    }
    let value = match array.data_type() {
        DataType::Int8 => f64::from(array.as_primitive::<Int8Type>().value(row)),
        DataType::Int16 => f64::from(array.as_primitive::<Int16Type>().value(row)),
        DataType::Int32 => f64::from(array.as_primitive::<Int32Type>().value(row)),
        DataType::Int64 => array.as_primitive::<Int64Type>().value(row) as f64,
        DataType::UInt8 => f64::from(array.as_primitive::<UInt8Type>().value(row)),
        DataType::UInt16 => f64::from(array.as_primitive::<UInt16Type>().value(row)),
        DataType::UInt32 => f64::from(array.as_primitive::<UInt32Type>().value(row)),
        DataType::UInt64 => array.as_primitive::<UInt64Type>().value(row) as f64,
        DataType::Float32 => f64::from(array.as_primitive::<Float32Type>().value(row)),
        DataType::Float64 => array.as_primitive::<Float64Type>().value(row),
        DataType::Date32 => f64::from(array.as_primitive::<Date32Type>().value(row)),
        DataType::Timestamp(unit, _) => timestamp_values(array, *unit).value(row) as f64,
        DataType::Decimal128(_, scale) => {
            array.as_primitive::<Decimal128Type>().value(row) as f64 / 10f64.powi(i32::from(*scale))
        }
        other => return Err(unsketchable(other)),
    };
    sketch.update(value);
    Ok(())
}

/// A timestamp array's raw values, whatever its unit.
pub fn timestamp_values(array: &ArrayRef, unit: TimeUnit) -> PrimitiveArray<Int64Type> {
    match unit {
        TimeUnit::Second => array
            .as_primitive::<TimestampSecondType>()
            .reinterpret_cast::<Int64Type>(),
        TimeUnit::Millisecond => array
            .as_primitive::<TimestampMillisecondType>()
            .reinterpret_cast::<Int64Type>(),
        TimeUnit::Microsecond => array
            .as_primitive::<TimestampMicrosecondType>()
            .reinterpret_cast::<Int64Type>(),
        TimeUnit::Nanosecond => array
            .as_primitive::<TimestampNanosecondType>()
            .reinterpret_cast::<Int64Type>(),
    }
}

fn write_display<T: std::fmt::Display>(text: &mut String, value: T) {
    use std::fmt::Write;
    let _ = write!(text, "{value}");
}

fn display_row<'a, T: ArrowPrimitiveType>(
    values: &PrimitiveArray<T>,
    row: usize,
    text: &'a mut String,
) -> &'a [u8]
where
    T::Native: std::fmt::Display,
{
    write_display(text, values.value(row));
    text.as_bytes()
}

fn unsketchable(data_type: &DataType) -> crate::KaveonError {
    crate::KaveonError::Execution(format!("column type {data_type} cannot be sketched"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hll_estimates_one_million_distinct_values_within_two_percent_at_p12() {
        let mut sketch = HllSketch::new(12).unwrap();
        for value in 0..1_000_000u64 {
            sketch.insert_text(&value.to_string());
        }
        let estimate = sketch.estimate() as f64;
        let error = (estimate - 1_000_000.0).abs() / 1_000_000.0;
        assert!(error < 0.02, "estimate {estimate} is off by {error}");
        // Duplicates do not change the estimate.
        for value in 0..1_000_000u64 {
            sketch.insert_text(&value.to_string());
        }
        assert_eq!(sketch.estimate() as f64, estimate);
    }

    #[test]
    fn hll_is_exact_at_low_cardinalities_and_accurate_at_every_precision() {
        for precision in HLL_MIN_PRECISION..=HLL_MAX_PRECISION {
            let mut sketch = HllSketch::new(precision).unwrap();
            assert_eq!(sketch.estimate(), 0);
            for value in 0..100u64 {
                sketch.insert_text(&format!("v{value}"));
            }
            let low = sketch.estimate() as f64;
            assert!((low - 100.0).abs() <= 5.0, "precision {precision}: {low}");
            for value in 100..50_000u64 {
                sketch.insert_text(&format!("v{value}"));
            }
            let estimate = sketch.estimate() as f64;
            let tolerance = 3.0 * 1.04 / (f64::from(1u32 << precision)).sqrt();
            assert!(
                ((estimate - 50_000.0) / 50_000.0).abs() < tolerance,
                "precision {precision}: {estimate}"
            );
        }
    }

    #[test]
    fn hll_at_precision_12_is_within_two_percent_over_a_million_values() {
        let mut whole = HllSketch::new(12).unwrap();
        let mut halves = [HllSketch::new(12).unwrap(), HllSketch::new(12).unwrap()];
        // Integer keys as an integer column hashes them.
        for value in 0..1_000_000u64 {
            let text = value.to_string();
            whole.insert_text(&text);
            halves[(value % 2) as usize].insert_text(&text);
        }
        let estimate = whole.estimate() as f64;
        assert!(
            ((estimate - 1_000_000.0) / 1_000_000.0).abs() < 0.02,
            "{estimate}"
        );
        // Merging the halves is the sketch over the whole.
        let mut merged = halves[0].clone();
        merged.merge(&halves[1]).unwrap();
        assert_eq!(merged, whole);
        // Values seen again change nothing.
        let mut again = whole.clone();
        again.insert_text("7");
        assert_eq!(again, whole);
    }

    #[test]
    fn hll_merge_equals_the_sketch_over_the_union() {
        let mut left = HllSketch::new(12).unwrap();
        let mut right = HllSketch::new(12).unwrap();
        let mut whole = HllSketch::new(12).unwrap();
        for value in 0..200_000u64 {
            let text = value.to_string();
            whole.insert_text(&text);
            if value % 3 == 0 || value % 5 == 0 {
                left.insert_text(&text);
            }
            if value % 3 != 0 {
                right.insert_text(&text);
            }
        }
        let mut merged = left.clone();
        merged.merge(&right).unwrap();
        assert_eq!(merged, whole);
        // Sparse into dense and dense into sparse both merge.
        let mut small = HllSketch::new(12).unwrap();
        small.insert_text("only");
        let mut dense_first = whole.clone();
        dense_first.merge(&small).unwrap();
        let mut sparse_first = small.clone();
        sparse_first.merge(&whole).unwrap();
        assert_eq!(dense_first, sparse_first);
        assert!(HllSketch::new(11).unwrap().merge(&small).is_err());
    }

    #[test]
    fn hll_bytes_round_trip_sparse_and_dense_and_reject_garbage() {
        let mut sparse = HllSketch::new(13).unwrap();
        for value in 0..50u64 {
            sparse.insert_text(&value.to_string());
        }
        assert!(matches!(sparse.registers, Registers::Sparse(_)));
        let bytes = sparse.to_bytes();
        assert!(bytes.len() < 200);
        assert_eq!(HllSketch::from_bytes(&bytes).unwrap(), sparse);

        let mut dense = HllSketch::new(12).unwrap();
        for value in 0..100_000u64 {
            dense.insert_text(&value.to_string());
        }
        assert!(matches!(dense.registers, Registers::Dense(_)));
        let bytes = dense.to_bytes();
        assert_eq!(bytes.len(), 4 + 4096 * 6 / 8);
        assert_eq!(HllSketch::from_bytes(&bytes).unwrap(), dense);

        let json = serde_json::to_string(&dense).unwrap();
        assert_eq!(serde_json::from_str::<HllSketch>(&json).unwrap(), dense);
        assert!(HllSketch::from_bytes(&[1, 2, 3]).is_err());
        assert!(HllSketch::from_bytes(&[HLL_MAGIC, HLL_VERSION, 12, 9]).is_err());
        let mut truncated = dense.to_bytes();
        truncated.pop();
        assert!(HllSketch::from_bytes(&truncated).is_err());
    }

    #[test]
    fn hll_register_layout_matches_the_dlm_extraction() {
        // The DLM takes the top p bits as the register and the 1-based
        // position of the first set bit in the remaining 64 - p bits as
        // rho, 64 - p + 1 when none is set.
        let mut sketch = HllSketch::new(11).unwrap();
        let hash = (0x5A5u64 << 53) | (1u64 << 40);
        sketch.insert_hash(hash);
        let Registers::Sparse(entries) = &sketch.registers else {
            panic!("sparse");
        };
        assert_eq!(entries.get(&0x5A5), Some(&13));
        let mut zero_rest = HllSketch::new(11).unwrap();
        zero_rest.insert_hash(0x7u64 << 53);
        let Registers::Sparse(entries) = &zero_rest.registers else {
            panic!("sparse");
        };
        assert_eq!(entries.get(&7), Some(&54));
    }

    #[test]
    fn pg_hash_is_the_lookup3_variant_with_postgresql_initialisation() {
        // Determinism and the length-dependent tail paths.
        for len in 0..40 {
            let key: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            assert_eq!(
                pg_hash_bytes_extended(&key, 0),
                pg_hash_bytes_extended(&key, 0)
            );
            assert_ne!(
                pg_hash_bytes_extended(&key, 0),
                pg_hash_bytes_extended(&key, 1)
            );
        }
        // Distinct short keys hash apart.
        let mut seen = std::collections::HashSet::new();
        for value in 0..10_000u32 {
            assert!(seen.insert(hash_text(&value.to_string())));
        }
        // The empty key is the initial state finalised.
        let a = 0x9e37_79b9u32.wrapping_add(3_923_095);
        let mut b = a;
        let mut c = a;
        let mut a = a;
        c ^= b;
        c = c.wrapping_sub(b.rotate_left(14));
        a ^= c;
        a = a.wrapping_sub(c.rotate_left(11));
        b ^= a;
        b = b.wrapping_sub(a.rotate_left(25));
        c ^= b;
        c = c.wrapping_sub(b.rotate_left(16));
        a ^= c;
        a = a.wrapping_sub(c.rotate_left(4));
        b ^= a;
        b = b.wrapping_sub(a.rotate_left(14));
        c ^= b;
        c = c.wrapping_sub(b.rotate_left(24));
        assert_eq!(
            pg_hash_bytes_extended(b"", 0),
            (u64::from(b) << 32) | u64::from(c)
        );
    }

    #[test]
    fn kll_rank_error_stays_within_bound_over_a_million_values() {
        let mut sketch = KllSketch::default_k();
        // A skewed stream in shuffled order.
        let mut state = 12345u64;
        for _ in 0..1_000_000u64 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let uniform = (state >> 11) as f64 / (1u64 << 53) as f64;
            sketch.update(uniform * uniform * 1000.0);
        }
        assert_eq!(sketch.count(), 1_000_000);
        assert!(sketch.memory_bytes() < 64 * 1024);
        for fraction in [0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99] {
            let exact = fraction * fraction * 1000.0;
            let estimated = sketch.quantile(fraction).unwrap();
            let rank_of_estimate = (estimated / 1000.0).sqrt();
            assert!(
                (rank_of_estimate - fraction).abs() < 0.02,
                "q{fraction}: {estimated} (exact {exact}), rank {rank_of_estimate}"
            );
            let rank = sketch.rank(exact);
            assert!((rank - fraction).abs() < 0.02, "rank({exact}) = {rank}");
        }
        assert_eq!(sketch.quantile(0.0), Some(sketch.min().unwrap()));
        assert_eq!(sketch.quantile(1.0), Some(sketch.max().unwrap()));
        let between = sketch.fraction_between(Some((250.0, true)), Some((1000.0, true)));
        assert!((between - 0.5).abs() < 0.02, "{between}");
    }

    #[test]
    fn kll_merge_and_bytes_preserve_the_distribution() {
        let mut left = KllSketch::default_k();
        let mut right = KllSketch::default_k();
        for value in 0..100_000 {
            if value % 2 == 0 {
                left.update(f64::from(value));
            } else {
                right.update(f64::from(value));
            }
        }
        let mut merged = left.clone();
        merged.merge(&right).unwrap();
        assert_eq!(merged.count(), 100_000);
        let median = merged.quantile(0.5).unwrap();
        assert!((median - 50_000.0).abs() < 2_000.0, "{median}");
        let bytes = merged.to_bytes();
        let decoded = KllSketch::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, merged);
        let json = serde_json::to_string(&merged).unwrap();
        assert_eq!(serde_json::from_str::<KllSketch>(&json).unwrap(), merged);
        assert!(KllSketch::from_bytes(&bytes[..bytes.len() - 1]).is_err());
        let mut other_k = KllSketch::new(100).unwrap();
        other_k.update(1.0);
        assert!(KllSketch::new(200).unwrap().merge(&other_k).is_err());
        let empty = KllSketch::default_k();
        assert_eq!(empty.quantile(0.5), None);
        assert_eq!(KllSketch::from_bytes(&empty.to_bytes()).unwrap(), empty);
    }

    #[test]
    fn base64_round_trips() {
        for len in 0..20 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&bytes)).unwrap(), bytes);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_decode("TWE"), None);
        assert_eq!(base64_decode("TW=E"), None);
    }

    #[test]
    fn array_folds_hash_the_canonical_text_row_by_row_and_state_their_error() {
        use arrow::array::{
            Date32Array, Decimal128Array, Float64Array, Int64Array, StringArray,
            TimestampMillisecondArray,
        };
        let columns: Vec<(ArrayRef, Vec<String>)> = vec![
            (
                Arc::new(Int64Array::from(vec![Some(1), None, Some(-7)])),
                vec!["1".into(), "-7".into()],
            ),
            (
                Arc::new(Float64Array::from(vec![Some(0.5), Some(2.0)])),
                vec!["0.5".into(), "2".into()],
            ),
            (
                Arc::new(StringArray::from(vec![Some("a"), None, Some("b")])),
                vec!["a".into(), "b".into()],
            ),
            (
                Arc::new(Date32Array::from(vec![Some(0), Some(20_635)])),
                vec![
                    StatValue::Date(0).to_hash_text(),
                    StatValue::Date(20_635).to_hash_text(),
                ],
            ),
            (
                Arc::new(TimestampMillisecondArray::from(vec![Some(1_000)]).with_timezone("UTC")),
                vec![
                    StatValue::Timestamp {
                        value: 1_000,
                        unit: TimeUnit::Millisecond,
                        utc: true,
                    }
                    .to_hash_text(),
                ],
            ),
            (
                Arc::new(
                    Decimal128Array::from(vec![Some(12_345)])
                        .with_precision_and_scale(10, 2)
                        .unwrap(),
                ),
                vec!["123.45".into()],
            ),
        ];
        for (array, texts) in columns {
            let mut expected = HllSketch::default_precision();
            for text in &texts {
                expected.insert_text(text);
            }
            let mut whole = HllSketch::default_precision();
            fold_distinct(&array, &mut whole).unwrap();
            assert_eq!(whole, expected, "{}", array.data_type());
            let mut rows = HllSketch::default_precision();
            let mut scratch = String::new();
            for row in 0..array.len() {
                fold_distinct_row(&array, row, &mut rows, &mut scratch).unwrap();
            }
            assert_eq!(rows, expected, "{}", array.data_type());
            // A dictionary over the values folds as the values.
            let dictionary = arrow::compute::cast(
                &array,
                &DataType::Dictionary(
                    Box::new(DataType::Int32),
                    Box::new(array.data_type().clone()),
                ),
            );
            if let Ok(dictionary) = dictionary {
                let mut coded = HllSketch::default_precision();
                fold_distinct(&dictionary, &mut coded).unwrap();
                assert_eq!(coded, expected);
                let mut coded_rows = HllSketch::default_precision();
                for row in 0..dictionary.len() {
                    fold_distinct_row(&dictionary, row, &mut coded_rows, &mut scratch).unwrap();
                }
                assert_eq!(coded_rows, expected);
            }
        }
        let numbers: ArrayRef = Arc::new(Int64Array::from(vec![Some(3), None, Some(1), Some(2)]));
        let mut whole = KllSketch::default_k();
        fold_quantiles(&numbers, &mut whole).unwrap();
        let mut rows = KllSketch::default_k();
        for row in 0..numbers.len() {
            fold_quantiles_row(&numbers, row, &mut rows).unwrap();
        }
        assert_eq!(whole.count(), 3);
        assert_eq!(whole, rows);
        assert_eq!(whole.quantile(0.5), Some(2.0));
        let text: ArrayRef = Arc::new(StringArray::from(vec!["a"]));
        assert!(fold_quantiles(&text, &mut whole).is_err());
        assert!(sketchable(&DataType::Utf8) && !quantile_sketchable(&DataType::Utf8));
        assert!(quantile_sketchable(&DataType::Decimal128(10, 2)));
        assert!(!sketchable(&DataType::Binary));

        assert!((HllSketch::default_precision().standard_error() - 0.01625).abs() < 1e-6);
        assert!((HllSketch::new(14).unwrap().standard_error() - 0.008125).abs() < 1e-6);
        assert!((KllSketch::default_k().rank_error() - 0.0165).abs() < 5e-4);
    }
}
