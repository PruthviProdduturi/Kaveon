# Distributed cancellation worker fan-out

## Scope

Coordinator cancellation now sends the query cancellation request to every
active worker concurrently. A slow or unavailable worker therefore cannot
delay cancellation propagation to the other workers that may still own
exchange or spill state for the query.

## Evidence

The `cancellation_reaches_all_workers_concurrently` server regression starts
two local worker endpoints whose handlers wait until both requests arrive.
The test calls the coordinator fan-out helper with a two-second bound and
asserts that both workers received the request. A serial implementation would
deadlock the first request until the test timeout because the second worker
could never be contacted.

## Boundary

This qualifies cancellation request fan-out and does not claim worker-loss
recovery, retry policy, or final-merge memory-pressure behavior.
