# Connector Capability Matrix

This matrix distinguishes UI registration, API execution, and DLM profiling.
“Current” means code exists in the shipping Studio/API path; it does not imply
that every deployment has drivers, credentials, or network access configured.

| Source | Studio picker | API pool/query | Authentication | DLM notes |
|---|---|---|---|---|
| Fabric SQL analytics endpoint | Current | Current (`pyodbc`) | `DefaultAzureCredential` token | PostgreSQL-specific statistics and HLL profiling are unavailable |
| Fabric SQL warehouse | Current | Current (`pyodbc`) | `DefaultAzureCredential` token | Same limitation |
| Azure SQL | Current | Current (`pyodbc`) | `DefaultAzureCredential` token | Same limitation |
| PostgreSQL | Current | Current (`psycopg2`) | Password or configured Azure token path | Full implemented profiler path |
| StarRocks | Current | Current (`pymysql`, MySQL protocol) | Username/password | Manifest/precomputation may work; PostgreSQL statistics/HLL path is unavailable |
| MySQL/MariaDB | API only | Current (`pymysql`) | Username/password | Not exposed in Studio's source-type picker |
| Trino | Registration only | **Target** | None implemented | No executable driver |

## Important boundaries

- A platform query executes against one selected source. Cross-source federation
  and distributed joins are not implemented.
- `POST /api/v1/data-sources/{id}/test` is currently a stub. First-run setup and
  metadata-admin probe endpoints perform actual connection/write checks.
- Connection strings for PostgreSQL, MySQL, and StarRocks may contain credentials
  and are stored plaintext in `data_sources`; API responses suppress the field,
  but vault-backed storage is target work.
- Fabric/Azure SQL requires ODBC Driver 18 in the API runtime.
- Local Parquet is an Engine catalog path, not a Studio data-source connector.

See [data-source guide](../guides/data-sources.md), [configuration](configuration.md),
and [security](../../SECURITY.md).
