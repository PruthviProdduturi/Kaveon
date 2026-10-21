# Trino differential qualification — Docker two-worker topology, 500k rows

Status: **passed for distributed correctness**

The current qualification image and Trino were run against identical generated
Parquet files:

```powershell
python engine/qualification/same_files.py `
  --docker-image kaveon-engine:qualification-local `
  --rows 500000 `
  --customers 50000 `
  --suite extended `
  --warmups 1 `
  --repetitions 1 `
  --workers 2 `
  --output tmp/same-files-docker-500k-2worker
```

All **12/12** cases returned exact matching rows. Every non-metadata query ran
with `execution_mode: distributed` and 2–4 stages; the unfiltered count used the
documented coordinator metadata path. The comparison was fair for this bounded
workload, but this artifact is a correctness qualification and does not claim
general performance superiority or cloud behavior.

