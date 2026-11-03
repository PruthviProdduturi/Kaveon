# Server suite qualification — 2026-11-02

Source commit: `2e7c6de6`

## Result

The current server test suite was run from `engine/` with the long-running
differential module excluded:

```text
cargo test -p kaveon-server --all-targets --no-default-features -- \
  --nocapture --skip differential_tests

test result: ok. 470 passed; 0 failed; 0 ignored; 0 measured;
193 filtered out; finished in 31.11s
```

The run includes the server, API, transaction, storage, scheduler, exchange,
recovery, cancellation, result-cache, and spill tests. The injected-failure
and worker-loss recovery cases passed.

The latest full server binary run, including the long-running differential
module, also passed:

```text
cargo test -p kaveon-server --bin kaveon-server

test result: ok. 674 passed; 0 failed; 0 ignored; 0 measured;
finished in 717.09s
```

This supersedes the earlier 671-test and 670-test full-suite results and the 470-test
bounded-suite count as the latest total for the server binary; the bounded run
remains useful as the fast pre-merge gate.

## Boundary

This is a local workspace result. The `differential_tests` module is excluded
because it is a long-running multi-encoding/distributed qualification gate;
this document does not claim that gate passed. No AKS, object-store fault, or
external-client qualification is implied.
