//! The final aggregate over one thread's share of partial rows.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::array::{Array, Int64Array, UInt64Array};
    use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
    use arrow::record_batch::RecordBatch;
    use kaveon_core::{BatchOperator, KaveonError, QueryMemoryPool, Result};

    use crate::aggregate::{
        AggregateState, AggregateValue, GroupedAggregateState,
        grouped_aggregate_states_to_typed_batch,
    };
    use crate::incremental_aggregate::{IncrementalAggregateMerger, MergedGroups};
    use crate::local_parallel::{ParallelPartials, ThreadOperator};
    use crate::spill::SpillManager;

    /// The q33 shape: `GROUP BY WatchID, ClientIP` with `COUNT(*)`,
    /// `SUM(IsRefresh)` and `AVG(ResolutionWidth)` — Int64 + Int32 keys,
    /// near unique (one row in sixteen repeats the key fifteen rows
    /// before it), three columnar states.
    const REPEAT_EVERY: usize = 16;
    const KEY_TYPES: [DataType; 2] = [DataType::Int64, DataType::Int32];
    const OUTPUT_TYPES: [DataType; 3] = [DataType::UInt64, DataType::Int64, DataType::Float64];

    fn q33_partial_batches(rows: usize, batch_rows: usize) -> Vec<RecordBatch> {
        let source_row = |i: usize| {
            if i % REPEAT_EVERY == REPEAT_EVERY - 1 {
                i - (REPEAT_EVERY - 1)
            } else {
                i
            }
        };
        (0..rows.div_ceil(batch_rows))
            .map(|batch| {
                let groups = (batch * batch_rows..((batch + 1) * batch_rows).min(rows))
                    .map(source_row)
                    .map(|i| GroupedAggregateState {
                        group_keys: vec![
                            AggregateValue::Int64(
                                (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) as i64
                            ),
                            AggregateValue::Int32((i as u32).wrapping_mul(0x85EB_CA6B) as i32),
                        ],
                        states: vec![
                            AggregateState::Count(1),
                            AggregateState::IntegerSum {
                                sum: i128::from(i.is_multiple_of(3)),
                                count: 1,
                            },
                            AggregateState::Avg {
                                sum: 1000.0 + (i % 500) as f64,
                                count: 1,
                            },
                        ],
                    })
                    .collect::<Vec<_>>();
                grouped_aggregate_states_to_typed_batch(&groups, &KEY_TYPES).unwrap()
            })
            .collect()
    }

    struct Batches {
        schema: SchemaRef,
        batches: std::collections::VecDeque<RecordBatch>,
    }
    impl BatchOperator for Batches {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }
    fn batches(batches: &[RecordBatch]) -> Box<dyn BatchOperator> {
        Box::new(Batches {
            schema: batches[0].schema(),
            batches: batches.iter().cloned().collect(),
        })
    }

    /// The finalised batch of the merged groups, the way the fragment
    /// executor builds it: keys as the exchange typed them, outputs from
    /// the accumulator columns.
    fn finalized(merged: MergedGroups) -> Result<RecordBatch> {
        let MergedGroups::Columnar(groups) = merged else {
            return Err(KaveonError::Execution(
                "expected the columnar layout".into(),
            ));
        };
        let (keys, outputs) = groups.into_final_arrays(&OUTPUT_TYPES)?;
        let fields = keys
            .iter()
            .chain(&outputs)
            .enumerate()
            .map(|(i, column)| Field::new(format!("c{i}"), column.data_type().clone(), true))
            .collect::<Vec<_>>();
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            keys.into_iter().chain(outputs).collect(),
        )?)
    }

    fn final_schema() -> SchemaRef {
        let groups = crate::columnar_aggregate::ColumnarGroups::new(
            &KEY_TYPES,
            &[
                AggregateState::Count(0),
                AggregateState::IntegerSum { sum: 0, count: 0 },
                AggregateState::Avg { sum: 0.0, count: 0 },
            ],
        )
        .unwrap();
        finalized(MergedGroups::Columnar(Box::new(groups)))
            .unwrap()
            .schema()
    }

    /// The in-memory merge over a source, finalised as one batch: what
    /// each thread of the replayable final runs today.
    fn in_memory_final(
        mut source: Box<dyn BatchOperator>,
        pool: &QueryMemoryPool,
    ) -> Result<Box<dyn BatchOperator>> {
        let account = pool.operator("final-aggregate")?;
        let mut merger = IncrementalAggregateMerger::new(Some(account.clone()));
        while let Some(batch) = source.next_batch()? {
            merger.push_batch(&batch)?;
        }
        let (merged, reservations) = merger.finish_groups()?;
        let batch = finalized(merged)?;
        drop(reservations);
        let _held = account.reserve(batch.get_array_memory_size() as u64)?;
        Ok(Box::new(Batches {
            schema: batch.schema(),
            batches: std::collections::VecDeque::from([batch]),
        }))
    }

    struct Totals {
        rows: usize,
        count: u64,
        sum: i64,
    }
    fn totals(operator: &mut dyn BatchOperator) -> Result<Totals> {
        let mut totals = Totals {
            rows: 0,
            count: 0,
            sum: 0,
        };
        while let Some(batch) = operator.next_batch()? {
            totals.rows += batch.num_rows();
            totals.count += batch
                .column(2)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .values()
                .iter()
                .sum::<u64>();
            totals.sum += batch
                .column(3)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values()
                .iter()
                .sum::<i64>();
        }
        Ok(totals)
    }

    fn spill_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kaveon-final-merge-{name}"))
    }

    /// The final stage on the q33 shape under a budget that refuses the
    /// in-memory merge near the end of its input, with a spill registered:
    /// the path the AKS final stage takes on ClickBench q19/q33/q34/q35.
    /// Ignored by default; run it as
    /// `cargo test --release -p kaveon-exec final_merge_under_pressure -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark: prints the final merge figures, run explicitly in release"]
    fn final_merge_under_pressure() {
        const ROWS: usize = 6_000_000;
        const BATCH_ROWS: usize = 262_144;
        const THREADS: usize = 3;
        let build_started = std::time::Instant::now();
        let batches_all = q33_partial_batches(ROWS, BATCH_ROWS);
        let encoded_bytes = batches_all
            .iter()
            .map(RecordBatch::get_array_memory_size)
            .sum::<usize>();
        println!(
            "built {ROWS} partial rows ({} MiB encoded) in {:.2?}",
            encoded_bytes >> 20,
            build_started.elapsed()
        );
        let expected_groups = ROWS - ROWS / REPEAT_EVERY;
        let expected_sum = (0..ROWS).filter(|i| i.is_multiple_of(3)).count() as i64;

        // The budget: the merge on three threads holds about 80 % of the
        // groups before a doubling is refused.
        let budget = 640u64 << 20;
        for round in 1..=3 {
            let pool = QueryMemoryPool::new("final-before", budget).unwrap();
            let spill = SpillManager::new(spill_root("before"), 8 << 30).unwrap();
            pool.shared_resource("kaveon.exec.hash-spill.v1", || Ok((spill.clone(), 16usize)))
                .unwrap();
            let started = std::time::Instant::now();
            let operator: ThreadOperator =
                Arc::new(move |source, pool, _| in_memory_final(source, pool));
            let mut attempt = ParallelPartials::partitioned(
                batches(&batches_all),
                final_schema(),
                vec!["group_keys".into()],
                operator,
                pool.clone(),
                THREADS,
            )
            .unwrap();
            let refused = match totals(&mut attempt) {
                Ok(_) => panic!("the budget is meant to refuse the in-memory attempt"),
                Err(KaveonError::MemoryLimit(message)) => message,
                Err(error) => panic!("{error}"),
            };
            drop(attempt);
            let attempt_took = started.elapsed();
            assert_eq!(pool.snapshot().current_bytes, 0);
            // The replay: the reopened input through the partitioned disk
            // path, one partition merged at a time on the calling thread.
            let replay_started = std::time::Instant::now();
            let partitions = crate::partitioned::partition_sources(
                batches(&batches_all),
                &["group_keys".into()],
                16,
                &pool.operator("final-partition").unwrap(),
                &spill,
            )
            .unwrap();
            let partitioned_in = replay_started.elapsed();
            let mut totals_all = Totals {
                rows: 0,
                count: 0,
                sum: 0,
            };
            for partition in partitions {
                let mut merged = in_memory_final(partition, &pool).unwrap();
                let part = totals(&mut *merged).unwrap();
                totals_all.rows += part.rows;
                totals_all.count += part.count;
                totals_all.sum += part.sum;
            }
            let total = started.elapsed();
            assert_eq!(totals_all.rows, expected_groups);
            assert_eq!(totals_all.count, ROWS as u64);
            assert_eq!(totals_all.sum, expected_sum);
            let snapshot = spill.snapshot();
            let memory = pool.snapshot();
            println!(
                "before, round {round}: {total:.2?} wall (attempt {attempt_took:.2?} refused: \
                 {refused}; replay partition {partitioned_in:.2?}, merge {:.2?}); spill {} MiB \
                 written in {} runs, {} compactions over {} MiB, write {:.2?} read {:.2?}; \
                 memory peak {} MiB, {} reservation calls",
                total - attempt_took - partitioned_in,
                snapshot.bytes_written >> 20,
                snapshot.runs_written,
                snapshot.compactions,
                snapshot.compaction_input_bytes >> 20,
                std::time::Duration::from_micros(snapshot.write_us),
                std::time::Duration::from_micros(snapshot.read_us),
                memory.peak_bytes >> 20,
                memory.reservation_calls,
            );
            assert_eq!(pool.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().current_bytes, 0);
        }
    }
}
