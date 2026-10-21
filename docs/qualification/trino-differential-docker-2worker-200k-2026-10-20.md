# Trino differential qualification — Docker two-worker topology, 200k rows

Status: **passed for distributed correctness**

The qualification harness now creates a disposable Docker network and uses
container DNS for coordinator discovery. The current image was compared with
Trino on identical generated Parquet files:

```powershell
python engine/qualification/same_files.py `
  --docker-image kaveon-engine:qualification-local `
  --rows 200000 `
  --customers 20000 `
  --suite extended `
  --warmups 1 `
  --repetitions 1 `
  --workers 2 `
  --output tmp/same-files-docker-200k-2worker
```

All **12/12** cases returned exact matching rows. Every non-metadata case ran
with `execution_mode: distributed` and 2–4 stages; the unfiltered metadata
count used the documented coordinator fast path. The report marks the timing
comparison fair for this bounded workload, but this result is not a general
performance claim or cloud qualification.

