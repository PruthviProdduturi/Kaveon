# Server suite qualification — 2026-11-02

## Result

The current server test suite was run from `engine/` with the long-running
differential module excluded:

```text
cargo test -p kaveon-server --all-targets --no-default-features -- \
  --nocapture --skip differential_tests

test result: ok. 468 passed; 0 failed; 0 ignored; 0 measured;
193 filtered out; finished in 31.11s
```

The run includes the server, API, transaction, storage, scheduler, exchange,
recovery, cancellation, result-cache, and spill tests. The injected-failure
and worker-loss recovery cases passed.

## Boundary

This is a local workspace result. The `differential_tests` module is excluded
because it is a long-running multi-encoding/distributed qualification gate;
this document does not claim that gate passed. No AKS, object-store fault, or
external-client qualification is implied.
