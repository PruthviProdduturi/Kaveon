# Track 1 — current release Docker distributed smoke

## Result

**PASS.** A freshly rebuilt release image from commit `41444c6b` passed the
extended same-file two-worker distributed smoke:

```text
docker build -t kaveon-engine:qualification-current -f engine/Dockerfile engine
python engine/qualification/same_files.py \
  --docker-image kaveon-engine:qualification-current \
  --suite extended --rows 500000 --customers 50000 \
  --warmups 1 --repetitions 2 --workers 2 \
  --output tmp/qualification-docker-2worker-current

12/12 cases passed; zero result mismatches; distributed stages observed.
```

The report records Kaveon image digest
`sha256:973b367b663b02f1b5c50d0f6812a89dde4da8cfa33be24ee974d5645924dce9`
and the pinned Trino 483 reference digest. It includes exact result hashes,
source-file checksums, runtime limits, and per-stage/task telemetry. The
comparison is correctness evidence, not a broad performance-superiority claim;
the workload is 500,000 rows with two warmups/repetitions and one local
parallel lane.

Only the disposable local Trino reference container was started. No cloud
resources were provisioned and incremental cloud spend was **$0**.
