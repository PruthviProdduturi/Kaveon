# Fabric/Azure SQL passthrough routing qualification — 2026-10-29

The API's catalog resolver now has an explicit routing boundary: only active
native Engine catalog sources resolve to Kaveon Engine execution. Fabric SQL
and Azure SQL registrations do not resolve as Engine catalogs and therefore
remain on their configured ODBC passthrough path. The resolver never treats a
logical catalog name from the browser as an Engine authority by itself.

Evidence:

- `api/routers/test_sql_engine.py::test_native_catalog_resolution_is_distinct_from_fabric_passthrough`
- `api/routers/test_sql_engine.py::test_fabric_catalog_without_native_adapter_stays_passthrough`
- `python -m pytest api/routers/test_sql_engine.py -q`: 43 passed

This qualifies routing and safety boundaries only. It does not claim that
Kaveon Engine executes Fabric/Azure SQL queries or supports cross-source joins.
