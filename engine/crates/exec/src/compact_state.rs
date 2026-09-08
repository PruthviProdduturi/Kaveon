//! Versioned accumulator payload inside the typed grouped Arrow envelope.
use super::*;

pub(super) fn encode(states: &[AggregateState]) -> Result<Vec<u8>> {
    state_layout(states)?;
    let mut out = b"KAS\x01".to_vec();
    length(&mut out, states.len())?;
    for state in states {
        crate::expr_eval::check_expression_cancelled()?;
        match state {
            AggregateState::Sum { sum, count } | AggregateState::Avg { sum, count } => {
                out.push(if matches!(state, AggregateState::Sum { .. }) {
                    1
                } else {
                    5
                });
                out.extend(sum.to_le_bytes());
                out.extend(count.to_le_bytes());
            }
            AggregateState::Count(count) => {
                out.push(2);
                out.extend(count.to_le_bytes());
            }
            AggregateState::Min(value) | AggregateState::Max(value) => {
                out.push(if matches!(state, AggregateState::Min(_)) {
                    3
                } else {
                    4
                });
                out.push(u8::from(value.is_some()));
                if let Some(value) = value {
                    out.extend(value.to_le_bytes());
                }
            }
            AggregateState::CountDistinct(values)
            | AggregateState::SumDistinct(values)
            | AggregateState::AvgDistinct(values)
            | AggregateState::IntegerSumDistinct(values) => {
                out.push(match state {
                    AggregateState::CountDistinct(_) => 6,
                    AggregateState::SumDistinct(_) => 7,
                    AggregateState::AvgDistinct(_) => 8,
                    _ => 13,
                });
                payload(&mut out, &encode_distinct_values(values)?)?;
            }
            AggregateState::DecimalSum { sum, count, scale } => {
                out.push(9);
                out.extend(sum.to_le_bytes());
                out.extend(count.to_le_bytes());
                out.push(*scale as u8);
            }
            AggregateState::IntegerSum { sum, count } => {
                out.push(10);
                out.extend(sum.to_le_bytes());
                out.extend(count.to_le_bytes());
            }
            AggregateState::IntegerMin(value) | AggregateState::IntegerMax(value) => {
                out.push(if matches!(state, AggregateState::IntegerMin(_)) {
                    11
                } else {
                    12
                });
                out.push(u8::from(value.is_some()));
                if let Some(value) = value {
                    out.extend(value.to_le_bytes());
                }
            }
            AggregateState::Exact {
                function,
                scale,
                value,
                distinct,
            } => {
                out.push(14);
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
                    payload(&mut out, &encode_distinct_values(values)?)?;
                }
            }
        }
    }
    Ok(out)
}

pub(super) fn decode(bytes: &[u8]) -> Result<Vec<AggregateState>> {
    let mut input = Input(bytes);
    if input.take(4)? != b"KAS\x01" {
        return Err(exec_err("unsupported compact aggregate state version"));
    }
    let count = input.u32()? as usize;
    if count > input.0.len() / 2 {
        return Err(exec_err("compact aggregate count exceeds payload"));
    }
    let mut states = Vec::with_capacity(count);
    for _ in 0..count {
        crate::expr_eval::check_expression_cancelled()?;
        let tag = input.byte()?;
        let state = match tag {
            1 | 5 => {
                let sum = f64::from_le_bytes(input.fixed()?);
                let count = u64::from_le_bytes(input.fixed()?);
                if tag == 1 {
                    AggregateState::Sum { sum, count }
                } else {
                    AggregateState::Avg { sum, count }
                }
            }
            2 => AggregateState::Count(u64::from_le_bytes(input.fixed()?)),
            3 | 4 => {
                let value = if input.flag()? {
                    Some(f64::from_le_bytes(input.fixed()?))
                } else {
                    None
                };
                if tag == 3 {
                    AggregateState::Min(value)
                } else {
                    AggregateState::Max(value)
                }
            }
            6 | 7 | 8 | 13 => {
                let values = decode_distinct_values(input.payload()?)?;
                match tag {
                    6 => AggregateState::CountDistinct(values),
                    7 => AggregateState::SumDistinct(values),
                    8 => AggregateState::AvgDistinct(values),
                    _ => AggregateState::IntegerSumDistinct(values),
                }
            }
            9 | 10 => {
                let sum = i128::from_le_bytes(input.fixed()?);
                let count = u64::from_le_bytes(input.fixed()?);
                if tag == 9 {
                    let scale = input.byte()? as i8;
                    if !(-38..=38).contains(&scale) {
                        return Err(exec_err("invalid decimal SUM scale"));
                    }
                    AggregateState::DecimalSum { sum, count, scale }
                } else {
                    AggregateState::IntegerSum { sum, count }
                }
            }
            11 | 12 => {
                let value = if input.flag()? {
                    Some(i64::from_le_bytes(input.fixed()?))
                } else {
                    None
                };
                if tag == 11 {
                    AggregateState::IntegerMin(value)
                } else {
                    AggregateState::IntegerMax(value)
                }
            }
            14 => {
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
    if !input.0.is_empty() {
        return Err(exec_err("trailing compact aggregate state bytes"));
    }
    state_layout(&states)?;
    Ok(states)
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
struct Input<'a>(&'a [u8]);
impl<'a> Input<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        if len > self.0.len() {
            return Err(exec_err("truncated compact aggregate payload"));
        }
        let (value, tail) = self.0.split_at(len);
        self.0 = tail;
        Ok(value)
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_| exec_err("invalid compact field"))
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn flag(&mut self) -> Result<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(exec_err("invalid compact optional flag")),
        }
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }
    fn payload(&mut self) -> Result<&'a [u8]> {
        let len = self.u32()? as usize;
        self.take(len)
    }
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
