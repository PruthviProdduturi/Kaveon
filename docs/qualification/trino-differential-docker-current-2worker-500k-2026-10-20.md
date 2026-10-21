# Trino differential qualification — current Docker image, two workers, 500k rows

Status: **passed for distributed correctness**

The image built from the current Engine source was compared with Trino on
identical 500k-row, 50k-customer Parquet fixtures:

```powershell
python engine/qualification/same_files.py `
  --docker-image kaveon-engine:qualification-current `
  --rows 500000 `
  --customers 50000 `
  --suite extended `
  --warmups 1 `
  --repetitions 1 `
  --workers 2 `
  --output tmp/same-files-docker-current-500k-2worker
```

All **12/12** cases returned exact matching rows. Every non-metadata query ran
with distributed stages; the unfiltered count used the documented coordinator
metadata path. This is a bounded local container qualification and does not
claim cloud endurance or general performance superiority.

