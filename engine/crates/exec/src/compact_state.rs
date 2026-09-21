//! Versioned accumulator payload inside the typed grouped Arrow envelope.
use super::*;

const MAGIC: &[u8; 4] = b"KAS\x01";

/// The state tags, one per accumulator kind the payload carries.
pub(crate) const TAG_SUM: u8 = 1;
pub(crate) const TAG_COUNT: u8 = 2;
pub(crate) const TAG_MIN: u8 = 3;
pub(crate) const TAG_MAX: u8 = 4;
pub(crate) const TAG_AVG: u8 = 5;
const TAG_COUNT_DISTINCT: u8 = 6;
const TAG_SUM_DISTINCT: u8 = 7;
const TAG_AVG_DISTINCT: u8 = 8;
const TAG_DECIMAL_SUM: u8 = 9;
pub(crate) const TAG_INTEGER_SUM: u8 = 10;
pub(crate) const TAG_INTEGER_MIN: u8 = 11;
pub(crate) const TAG_INTEGER_MAX: u8 = 12;
const TAG_INTEGER_SUM_DISTINCT: u8 = 13;
const TAG_EXACT: u8 = 14;
pub(crate) const TAG_UTF8_MIN: u8 = 15;
pub(crate) const TAG_UTF8_MAX: u8 = 16;
/// A HyperLogLog sketch: its compact bytes as one payload.
pub(crate) const TAG_APPROX_DISTINCT: u8 = 17;
/// A KLL sketch and what it answers, as one payload: the sketch's compact
/// bytes behind their length, the list flag, and the fractions (a count,
/// then each as a little-endian f64).
pub(crate) const TAG_APPROX_PERCENTILE: u8 = 18;
/// A HyperLogLog sketch answered as itself (`APPROX_COUNT_DISTINCT_STATE`):
/// its compact bytes as one payload.
pub(crate) const TAG_DISTINCT_SKETCH: u8 = 19;
/// A column profile (`COLUMN_STATISTICS`): its JSON as one payload.
pub(crate) const TAG_COLUMN_PROFILE: u8 = 20;

/// The payload of a sketch state, the same under both state encodings.
pub(crate) fn sketch_payload(state: &AggregateState) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match state {
        AggregateState::ApproxDistinct(sketch) | AggregateState::DistinctSketch(sketch) => {
            out.extend(sketch.to_bytes())
        }
        AggregateState::ColumnReadProfile(profile) => out.extend(profile.to_json_bytes()?),
        AggregateState::ApproxPercentile {
            sketch,
            percentiles,
        } => {
            payload(&mut out, &sketch.to_bytes())?;
            out.push(u8::from(percentiles.list));
            length(&mut out, percentiles.fractions.len())?;
            for fraction in &percentiles.fractions {
                out.extend(fraction.to_le_bytes());
            }
        }
        _ => return Err(exec_err("sketch payload requested from a non-sketch state")),
    }
    Ok(out)
}

/// A sketch state from its tag and payload.
pub(crate) fn sketch_state(tag: u8, bytes: &[u8]) -> Result<AggregateState> {
    match tag {
        TAG_APPROX_DISTINCT => Ok(AggregateState::ApproxDistinct(HllSketch::from_bytes(
            bytes,
        )?)),
        TAG_DISTINCT_SKETCH => Ok(AggregateState::DistinctSketch(HllSketch::from_bytes(
            bytes,
        )?)),
        TAG_COLUMN_PROFILE => Ok(AggregateState::ColumnReadProfile(Box::new(
            kaveon_storage::ColumnReadProfile::from_json_bytes(bytes)?,
        ))),
        TAG_APPROX_PERCENTILE => {
            let mut input = Input(bytes);
            let sketch = KllSketch::from_bytes(input.payload()?)?;
            let list = input.flag()?;
            let count = input.u32()? as usize;
            if count > input.0.len() / 8 {
                return Err(exec_err("APPROX_PERCENTILE fraction count exceeds payload"));
            }
            let mut fractions = Vec::with_capacity(count);
            for _ in 0..count {
                fractions.push(f64::from_le_bytes(input.fixed()?));
            }
            if !input.0.is_empty() {
                return Err(exec_err("trailing APPROX_PERCENTILE state bytes"));
            }
            let percentiles = Percentiles { fractions, list };
            percentiles.validate()?;
            Ok(AggregateState::ApproxPercentile {
                sketch,
                percentiles,
            })
        }
        _ => Err(exec_err("unknown sketch state tag")),
    }
}

/// Append one group's states to `out`. The caller has validated the layout
/// for the whole set of groups and checks cancellation per stride, so this
/// is the per-group hot path with no allocation of its own.
pub(crate) fn encode_into(states: &[AggregateState], out: &mut Vec<u8>) -> Result<()> {
    out.extend_from_slice(MAGIC);
    length(out, states.len())?;
    for state in states {
        match state {
            AggregateState::Sum { sum, count } | AggregateState::Avg { sum, count } => {
                out.push(if matches!(state, AggregateState::Sum { .. }) {
                    TAG_SUM
                } else {
                    TAG_AVG
                });
                out.extend(sum.to_le_bytes());
                out.extend(count.to_le_bytes());
            }
            AggregateState::Count(count) => {
                out.push(TAG_COUNT);
                out.extend(count.to_le_bytes());
            }
            AggregateState::Min(value) | AggregateState::Max(value) => {
                out.push(if matches!(state, AggregateState::Min(_)) {
                    TAG_MIN
                } else {
                    TAG_MAX
                });
                out.push(u8::from(value.is_some()));
                if let Some(value) = value {
                    out.extend(value.to_le_bytes());
                }
            }
            AggregateState::Utf8Min(value) | AggregateState::Utf8Max(value) => {
                out.push(if matches!(state, AggregateState::Utf8Min(_)) {
                    TAG_UTF8_MIN
                } else {
                    TAG_UTF8_MAX
                });
                out.push(u8::from(value.is_some()));
                if let Some(value) = value {
                    payload(
                        out,
                        &encode_aggregate_value(&AggregateValue::Utf8(value.clone()))?,
                    )?;
                }
            }
            AggregateState::CountDistinct(values)
            | AggregateState::SumDistinct(values)
            | AggregateState::AvgDistinct(values)
            | AggregateState::IntegerSumDistinct(values) => {
                out.push(match state {
                    AggregateState::CountDistinct(_) => TAG_COUNT_DISTINCT,
                    AggregateState::SumDistinct(_) => TAG_SUM_DISTINCT,
                    AggregateState::AvgDistinct(_) => TAG_AVG_DISTINCT,
                    _ => TAG_INTEGER_SUM_DISTINCT,
                });
                payload(out, &encode_distinct_values(values)?)?;
            }
            AggregateState::DecimalSum { sum, count, scale } => {
                out.push(TAG_DECIMAL_SUM);
                out.extend(sum.to_le_bytes());
                out.extend(count.to_le_bytes());
                out.push(*scale as u8);
            }
            AggregateState::IntegerSum { sum, count } => {
                out.push(TAG_INTEGER_SUM);
                out.extend(sum.to_le_bytes());
                out.extend(count.to_le_bytes());
            }
            AggregateState::IntegerMin(value) | AggregateState::IntegerMax(value) => {
                out.push(if matches!(state, AggregateState::IntegerMin(_)) {
                    TAG_INTEGER_MIN
                } else {
                    TAG_INTEGER_MAX
                });
                out.push(u8::from(value.is_some()));
                if let Some(value) = value {
                    out.extend(value.to_le_bytes());
                }
            }
            AggregateState::ApproxDistinct(_)
            | AggregateState::ApproxPercentile { .. }
            | AggregateState::DistinctSketch(_)
            | AggregateState::ColumnReadProfile(_) => {
                out.push(match state {
                    AggregateState::ApproxDistinct(_) => TAG_APPROX_DISTINCT,
                    AggregateState::ApproxPercentile { .. } => TAG_APPROX_PERCENTILE,
                    AggregateState::DistinctSketch(_) => TAG_DISTINCT_SKETCH,
                    _ => TAG_COLUMN_PROFILE,
                });
                payload(out, &sketch_payload(state)?)?;
            }
            AggregateState::Exact {
                function,
                scale,
                value,
                distinct,
            } => {
                out.push(TAG_EXACT);
                out.push(match function {
                    AggFunc::Sum => 0,
                    AggFunc::Min => 1,
                    AggFunc::Max => 2,
                    _ => return Err(exec_err("invalid exact function")),
                });
                out.push(u8::from(scale.is_some()));
                if let Some(scale) = scale {
                    out.push(*scale as u8);
                }
                out.push(u8::from(value.is_some()));
                if let Some(value) = value {
                    out.extend(value.to_le_bytes());
                }
                out.push(u8::from(distinct.is_some()));
                if let Some(values) = distinct {
                    payload(out, &encode_distinct_values(values)?)?;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn decode(bytes: &[u8]) -> Result<Vec<AggregateState>> {
    let mut states = Vec::new();
    decode_into(bytes, &mut states)?;
    Ok(states)
}

/// Decode one group's states into `states` (cleared first). The final
/// merge calls this once per incoming row with a reused vector, so a row
/// costs no allocation unless it opens a new group.
pub(crate) fn decode_into(bytes: &[u8], states: &mut Vec<AggregateState>) -> Result<()> {
    states.clear();
    let States {
        count,
        bytes: mut input,
    } = begin(bytes)?;
    states.reserve(count);
    for _ in 0..count {
        let tag = input.byte()?;
        let state = match tag {
            TAG_SUM | TAG_AVG => {
                let sum = f64::from_le_bytes(input.fixed()?);
                let count = u64::from_le_bytes(input.fixed()?);
                if tag == TAG_SUM {
                    AggregateState::Sum { sum, count }
                } else {
                    AggregateState::Avg { sum, count }
                }
            }
            TAG_COUNT => AggregateState::Count(u64::from_le_bytes(input.fixed()?)),
            TAG_MIN | TAG_MAX => {
                let value = if input.flag()? {
                    Some(f64::from_le_bytes(input.fixed()?))
                } else {
                    None
                };
                if tag == TAG_MIN {
                    AggregateState::Min(value)
                } else {
                    AggregateState::Max(value)
                }
            }
            TAG_UTF8_MIN | TAG_UTF8_MAX => {
                let value = if input.flag()? {
                    Some(utf8_extremum(input.payload()?)?.to_owned())
                } else {
                    None
                };
                if tag == TAG_UTF8_MIN {
                    AggregateState::Utf8Min(value)
                } else {
                    AggregateState::Utf8Max(value)
                }
            }
            TAG_COUNT_DISTINCT | TAG_SUM_DISTINCT | TAG_AVG_DISTINCT | TAG_INTEGER_SUM_DISTINCT => {
                let values = decode_distinct_values(input.payload()?)?;
                match tag {
                    TAG_COUNT_DISTINCT => AggregateState::CountDistinct(values),
                    TAG_SUM_DISTINCT => AggregateState::SumDistinct(values),
                    TAG_AVG_DISTINCT => AggregateState::AvgDistinct(values),
                    _ => AggregateState::IntegerSumDistinct(values),
                }
            }
            TAG_DECIMAL_SUM | TAG_INTEGER_SUM => {
                let sum = i128::from_le_bytes(input.fixed()?);
                let count = u64::from_le_bytes(input.fixed()?);
                if tag == TAG_DECIMAL_SUM {
                    let scale = input.byte()? as i8;
                    if !(-38..=38).contains(&scale) {
                        return Err(exec_err("invalid decimal SUM scale"));
                    }
                    AggregateState::DecimalSum { sum, count, scale }
                } else {
                    AggregateState::IntegerSum { sum, count }
                }
            }
            TAG_INTEGER_MIN | TAG_INTEGER_MAX => {
                let value = if input.flag()? {
                    Some(i64::from_le_bytes(input.fixed()?))
                } else {
                    None
                };
                if tag == TAG_INTEGER_MIN {
                    AggregateState::IntegerMin(value)
                } else {
                    AggregateState::IntegerMax(value)
                }
            }
            TAG_APPROX_DISTINCT
            | TAG_APPROX_PERCENTILE
            | TAG_DISTINCT_SKETCH
            | TAG_COLUMN_PROFILE => sketch_state(tag, input.payload()?)?,
            TAG_EXACT => {
                let function = match input.byte()? {
                    0 => AggFunc::Sum,
                    1 => AggFunc::Min,
                    2 => AggFunc::Max,
                    _ => return Err(exec_err("invalid exact function")),
                };
                let scale = if input.flag()? {
                    Some(input.byte()? as i8)
                } else {
                    None
                };
                let value = if input.flag()? {
                    Some(i128::from_le_bytes(input.fixed()?))
                } else {
                    None
                };
                let distinct = if input.flag()? {
                    Some(decode_distinct_values(input.payload()?)?)
                } else {
                    None
                };
                if distinct.is_some() && (value.is_some() || function != AggFunc::Sum) {
                    return Err(exec_err("invalid exact distinct state"));
                }
                let mut state = AggregateState::Exact {
                    function,
                    scale,
                    value,
                    distinct: distinct.as_ref().map(|_| HashSet::new()),
                };
                if let Some(values) = distinct {
                    for value in values {
                        state.update_exact(value)?;
                    }
                }
                state
            }
            _ => return Err(exec_err("unknown compact aggregate state tag")),
        };
        states.push(state);
    }
    States {
        count,
        bytes: input,
    }
    .finish()?;
    state_layout(states)?;
    Ok(())
}

/// One group's states opened for reading: how many follow, and the cursor
/// over them. The columnar final reads the accumulators straight from the
/// cursor, one tag and payload each, without a state enum per row.
pub(crate) struct States<'a> {
    pub(crate) count: usize,
    pub(crate) bytes: Input<'a>,
}

impl States<'_> {
    /// Every state has been read: nothing may follow.
    pub(crate) fn finish(self) -> Result<()> {
        if !self.bytes.0.is_empty() {
            return Err(exec_err("trailing compact aggregate state bytes"));
        }
        Ok(())
    }
}

/// Check the version and read the state count of one group's payload.
pub(crate) fn begin(bytes: &[u8]) -> Result<States<'_>> {
    let mut input = Input(bytes);
    if input.take(4)? != MAGIC {
        return Err(exec_err("unsupported compact aggregate state version"));
    }
    let count = input.u32()? as usize;
    if count > input.0.len() / 2 {
        return Err(exec_err("compact aggregate count exceeds payload"));
    }
    Ok(States {
        count,
        bytes: input,
    })
}

/// The text of a UTF-8 extremum payload: the typed value encoding, which
/// must carry a string.
pub(crate) fn utf8_extremum(payload: &[u8]) -> Result<&str> {
    match payload.split_first() {
        Some((&VALUE_UTF8, text)) => {
            std::str::from_utf8(text).map_err(|_| exec_err("distinct string is not valid UTF-8"))
        }
        _ => Err(exec_err("invalid UTF-8 extremum payload")),
    }
}

fn length(out: &mut Vec<u8>, size: usize) -> Result<()> {
    out.extend(
        u32::try_from(size)
            .map_err(|_| exec_err("compact aggregate payload too large"))?
            .to_le_bytes(),
    );
    Ok(())
}
fn payload(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    length(out, bytes.len())?;
    out.extend(bytes);
    Ok(())
}
/// A cursor over compact state bytes.
pub(crate) struct Input<'a>(&'a [u8]);
impl<'a> Input<'a> {
    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        if len > self.0.len() {
            return Err(exec_err("truncated compact aggregate payload"));
        }
        let (value, tail) = self.0.split_at(len);
        self.0 = tail;
        Ok(value)
    }
    pub(crate) fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_| exec_err("invalid compact field"))
    }
    pub(crate) fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn flag(&mut self) -> Result<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(exec_err("invalid compact optional flag")),
        }
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }
    pub(crate) fn payload(&mut self) -> Result<&'a [u8]> {
        let len = self.u32()? as usize;
        self.take(len)
    }
}

#[cfg(test)]
fn encode(states: &[AggregateState]) -> Result<Vec<u8>> {
    state_layout(states)?;
    let mut out = Vec::new();
    encode_into(states, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_round_trip_preserves_all_state_variants_and_is_canonical() {
        let values = HashSet::from([AggregateValue::Int64(3), AggregateValue::Int64(9)]);
        let states = vec![
            AggregateState::Sum {
                sum: 1.25,
                count: 2,
            },
            AggregateState::Count(u64::MAX),
            AggregateState::Min(None),
            AggregateState::Max(Some(-3.5)),
            AggregateState::Avg { sum: 4.5, count: 3 },
            AggregateState::CountDistinct(values.clone()),
            AggregateState::SumDistinct(values.clone()),
            AggregateState::AvgDistinct(values.clone()),
            AggregateState::IntegerSumDistinct(values),
            AggregateState::DecimalSum {
                sum: i128::MAX,
                count: 7,
                scale: 4,
            },
            AggregateState::IntegerSum {
                sum: i128::MIN,
                count: 2,
            },
            AggregateState::IntegerMin(Some(i64::MIN)),
            AggregateState::IntegerMax(None),
            AggregateState::Exact {
                function: AggFunc::Min,
                scale: Some(4),
                value: Some(i128::MIN),
                distinct: None,
            },
            AggregateState::Exact {
                function: AggFunc::Max,
                scale: None,
                value: Some(u64::MAX as i128),
                distinct: None,
            },
            AggregateState::Exact {
                function: AggFunc::Sum,
                scale: Some(4),
                value: None,
                distinct: Some(HashSet::from([AggregateValue::Decimal128(12345, 4)])),
            },
            AggregateState::ApproxDistinct({
                let mut sketch = HllSketch::default_precision();
                for value in 0..10_000 {
                    sketch.insert_text(&value.to_string());
                }
                sketch
            }),
            AggregateState::ApproxPercentile {
                sketch: {
                    let mut sketch = KllSketch::default_k();
                    for value in 0..10_000 {
                        sketch.update(f64::from(value));
                    }
                    sketch
                },
                percentiles: Percentiles {
                    fractions: vec![0.5, 0.99],
                    list: true,
                },
            },
            AggregateState::DistinctSketch({
                let mut sketch = HllSketch::default_precision();
                for value in 0..100 {
                    sketch.insert_text(&value.to_string());
                }
                sketch
            }),
            AggregateState::ColumnReadProfile(Box::new({
                let mut profile = kaveon_storage::ColumnReadProfile::untyped();
                let array: ArrayRef = Arc::new(arrow::array::Int64Array::from(vec![
                    Some(3),
                    None,
                    Some(-7),
                    Some(12),
                ]));
                profile.fold_array(&array).unwrap();
                profile
            })),
            AggregateState::ColumnReadProfile(Box::new(
                kaveon_storage::ColumnReadProfile::untyped(),
            )),
        ];
        let bytes = encode(&states).unwrap();
        assert_eq!(decode(&bytes).unwrap(), states);
        assert_eq!(encode(&decode(&bytes).unwrap()).unwrap(), bytes);
        assert!(encode(&[AggregateState::Count(1)]).unwrap().len() < 20);
        // The separate public Arrow accumulator format stays compatible.
        assert_eq!(
            decode_aggregate_states(&encode_aggregate_states(&states).unwrap()).unwrap(),
            states
        );
    }

    #[test]
    fn compact_decoder_rejects_truncation_counts_tags_flags_and_trailing_bytes() {
        let good = encode(&[AggregateState::Count(7)]).unwrap();
        for end in 0..good.len() {
            assert!(decode(&good[..end]).is_err());
        }
        let mut bad = good.clone();
        bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&bad).is_err());
        let mut bad = good.clone();
        bad[8] = 255;
        assert!(decode(&bad).is_err());
        let mut bad = good.clone();
        bad[3] = 255;
        assert!(decode(&bad).is_err());
        let mut bad = good;
        bad.push(0);
        assert!(decode(&bad).is_err());
        let mut bad = encode(&[AggregateState::Min(None)]).unwrap();
        bad[9] = 3;
        assert!(decode(&bad).is_err());
    }
}
