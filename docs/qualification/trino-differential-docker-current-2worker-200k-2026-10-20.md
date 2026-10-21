# Trino differential qualification — current Docker image, two workers

Status: **passed for distributed correctness**

The Engine image was rebuilt from the current source before the run:

```powershell
docker build -t kaveon-engine:qualification-current -f engine/Dockerfile engine
```

The rebuilt image was then compared with Trino on identical 200k-row,
20k-customer Parquet fixtures:

```powershell
python engine/qualification/same_files.py `
  --docker-image kaveon-engine:qualification-current `
  --rows 200000 `
  --customers 20000 `
  --suite extended `
  --warmups 1 `
  --repetitions 1 `
  --workers 2 `
  --output tmp/same-files-docker-current-200k-2worker
```

All **12/12** cases returned exact matching rows. Every non-metadata query ran
with distributed stages; the unfiltered count used the documented coordinator
metadata path. This is a bounded local container qualification, not a cloud
endurance or general performance-superiority claim.

