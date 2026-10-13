"use client";

/**
 * The Engine-catalog path through the new-dataset form.
 *
 * A lake-backed installation has no external database connection and never
 * will: it reads the lake through registered catalogs, and a dataset over one
 * of their tables is bound by the Engine's table id. The API then resolves the
 * catalog, schema and table names, the columns, the metrics and the date
 * column from the Engine's own definition, so this surface collects exactly
 * one fact — which table — and reports what that binding will produce.
 *
 * Catalog → schema → table, each a listing of roughly half a second against
 * the Engine. Every step says that it is running rather than showing a
 * control that looks merely empty, and row counts arrive separately from the
 * schema's inventory so the table list is usable before they land.
 */

import React, { useEffect, useMemo, useState } from "react";

import {
  EngineCatalog,
  EngineMeasurement,
  EngineSchema,
  EngineTable,
  derivedDateColumn,
  fetchCatalogSchemas,
  fetchSchemaInventory,
  fetchSchemaTables,
  formatBytes,
  formatRows,
  temporalColumns,
} from "../../../utils/engineCatalog";
import s from "./engineSource.module.css";

export interface EngineBinding {
  catalog: EngineCatalog;
  schema: EngineSchema;
  table: EngineTable;
  measurement: EngineMeasurement | null;
}

function problem(error: unknown): string {
  return error instanceof Error ? error.message : "The catalog could not be read.";
}

function Working({ children }: { children: React.ReactNode }) {
  return (
    <p className={s.working}>
      <span className={s.spinner} aria-hidden="true" />
      {children}
    </p>
  );
}

function Problem({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <p className={s.problem}>
      {message}
      <button type="button" className={s.retry} onClick={onRetry}>
        Try again
      </button>
    </p>
  );
}

/** The table options' trailing count, once the schema's inventory has landed. */
function tableLabel(table: EngineTable, measurement: EngineMeasurement | undefined): string {
  if (!measurement) return table.name;
  if (measurement.state === "unreadable") return `${table.name} — unreadable`;
  const rows = formatRows(measurement.rows);
  return rows ? `${table.name} — ${rows} rows` : table.name;
}

export function EngineTablePicker({
  catalogs,
  unsynchronized = [],
  onChange,
}: {
  catalogs: EngineCatalog[];
  /**
   * Catalog sources that are registered but have no definition in the Engine.
   * Named here rather than dropped: a reader who cannot find their catalog in
   * the list is owed the reason.
   */
  unsynchronized?: string[];
  /** Stable callback: the picker emits on every change of the resolved binding. */
  onChange: (binding: EngineBinding | null) => void;
}) {
  const [catalogId, setCatalogId] = useState("");
  const [schemas, setSchemas] = useState<EngineSchema[]>([]);
  const [schemasLoading, setSchemasLoading] = useState(false);
  const [schemasError, setSchemasError] = useState<string | null>(null);
  const [schemaId, setSchemaId] = useState("");
  const [tables, setTables] = useState<EngineTable[]>([]);
  const [tablesLoading, setTablesLoading] = useState(false);
  const [tablesError, setTablesError] = useState<string | null>(null);
  const [tableId, setTableId] = useState("");
  const [measurements, setMeasurements] = useState<EngineMeasurement[]>([]);
  const [measuring, setMeasuring] = useState(false);
  const [attempt, setAttempt] = useState(0);

  // One registered catalog is not a choice; land the reader on its schemas.
  useEffect(() => {
    if (!catalogId && catalogs.length === 1) setCatalogId(catalogs[0].id);
  }, [catalogs, catalogId]);

  useEffect(() => {
    setSchemas([]);
    setSchemaId("");
    setSchemasError(null);
    // A listing that was abandoned mid-flight never reaches its own cleanup,
    // so the flag it set is cleared here rather than left reading "reading".
    setSchemasLoading(false);
    if (!catalogId) return;
    let cancelled = false;
    setSchemasLoading(true);
    fetchCatalogSchemas(catalogId)
      .then(list => {
        if (cancelled) return;
        setSchemas(list);
        if (list.length === 1) setSchemaId(list[0].id);
      })
      .catch(error => {
        if (!cancelled) setSchemasError(problem(error));
      })
      .finally(() => {
        if (!cancelled) setSchemasLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [catalogId, attempt]);

  useEffect(() => {
    setTables([]);
    setTableId("");
    setTablesError(null);
    setMeasurements([]);
    setTablesLoading(false);
    setMeasuring(false);
    if (!schemaId) return;
    let cancelled = false;
    setTablesLoading(true);
    fetchSchemaTables(schemaId)
      .then(list => {
        if (!cancelled) setTables(list);
      })
      .catch(error => {
        if (!cancelled) setTablesError(problem(error));
      })
      .finally(() => {
        if (!cancelled) setTablesLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [schemaId, attempt]);

  // Row counts are an annotation on a list that already works, so they are
  // read after the definitions rather than with them. The platform fans the
  // measurements out server-side: one request for the whole schema.
  useEffect(() => {
    if (!schemaId || tables.length === 0) return;
    let cancelled = false;
    setMeasuring(true);
    fetchSchemaInventory(schemaId)
      .then(list => {
        if (!cancelled) setMeasurements(list);
      })
      .catch(() => {
        // A table is still bindable without its row count; the summary says so.
      })
      .finally(() => {
        if (!cancelled) setMeasuring(false);
      });
    return () => {
      cancelled = true;
    };
  }, [schemaId, tables]);

  const catalog = useMemo(() => catalogs.find(item => item.id === catalogId) ?? null, [catalogs, catalogId]);
  const schema = useMemo(() => schemas.find(item => item.id === schemaId) ?? null, [schemas, schemaId]);
  const table = useMemo(() => tables.find(item => item.id === tableId) ?? null, [tables, tableId]);
  const measurement = useMemo(
    () => measurements.find(item => item.tableId === tableId) ?? null,
    [measurements, tableId],
  );

  useEffect(() => {
    onChange(catalog && schema && table ? { catalog, schema, table, measurement } : null);
  }, [catalog, schema, table, measurement, onChange]);

  const catalogKind = [catalog?.storageType, catalog?.dataFormat].filter(Boolean).join(" · ");

  return (
    <div>
      <div className={s.field}>
        <label className={s.label} htmlFor="dataset-engine-catalog">
          Catalog
        </label>
        <select
          id="dataset-engine-catalog"
          className="save-query-input chart-builder-select"
          value={catalogId}
          onChange={event => setCatalogId(event.target.value)}
        >
          <option value="" disabled>
            Select catalog
          </option>
          {catalogs.map(item => (
            <option key={item.id} value={item.id}>
              {item.name}
            </option>
          ))}
        </select>
        {catalog && (catalogKind || catalog.description) && (
          <p className={s.hint}>{catalog.description || catalogKind}</p>
        )}
        {unsynchronized.length > 0 && (
          <p className={s.hint}>
            {unsynchronized.length === 1
              ? `${unsynchronized[0]} is registered but the Engine returned no definition for it`
              : `${unsynchronized.length} registered catalog sources returned no Engine definition`}
            , so {unsynchronized.length === 1 ? "it is" : "they are"} not listed here: either not yet
            synchronized, or not granted to your account.
          </p>
        )}
      </div>

      <div className={s.field}>
        <label className={s.label} htmlFor="dataset-engine-schema">
          Schema
        </label>
        <select
          id="dataset-engine-schema"
          className="save-query-input chart-builder-select"
          value={schemaId}
          onChange={event => setSchemaId(event.target.value)}
          disabled={!catalogId || schemasLoading || schemas.length === 0}
        >
          <option value="" disabled>
            {!catalogId
              ? "Select a catalog first"
              : schemasLoading
              ? "Reading schemas…"
              : schemas.length === 0
              ? "This catalog holds no schema"
              : "Select schema"}
          </option>
          {schemas.map(item => (
            <option key={item.id} value={item.id}>
              {item.name}
            </option>
          ))}
        </select>
        {schemasLoading && <Working>Reading the catalog&rsquo;s schemas</Working>}
        {schemasError && <Problem message={schemasError} onRetry={() => setAttempt(n => n + 1)} />}
      </div>

      <div className={s.field}>
        <label className={s.label} htmlFor="dataset-engine-table">
          Table
        </label>
        <select
          id="dataset-engine-table"
          className="save-query-input chart-builder-select"
          value={tableId}
          onChange={event => setTableId(event.target.value)}
          disabled={!schemaId || tablesLoading || tables.length === 0}
        >
          <option value="" disabled>
            {!schemaId
              ? "Select a schema first"
              : tablesLoading
              ? "Reading tables…"
              : tables.length === 0
              ? "This schema holds no active table"
              : "Select table"}
          </option>
          {tables.map(item => (
            <option key={item.id} value={item.id}>
              {tableLabel(item, measurements.find(entry => entry.tableId === item.id))}
            </option>
          ))}
        </select>
        {tablesLoading && <Working>Reading the schema&rsquo;s tables</Working>}
        {!tablesLoading && measuring && <Working>Reading row counts</Working>}
        {tablesError && <Problem message={tablesError} onRetry={() => setAttempt(n => n + 1)} />}
      </div>
    </div>
  );
}

/**
 * What the chosen table will give the dataset, and the one field the reader
 * may override. Everything else is read from the Engine's definition at
 * creation, which is what keeps a dataset from naming one table and binding
 * to another.
 */
export function EngineBindingPanel({
  qualifiedName,
  table,
  measurement,
  bound,
  dateColumn,
  onDateColumn,
}: {
  /** `catalog.schema.table`, or null while nothing is chosen. */
  qualifiedName: string | null;
  table: EngineTable | null;
  measurement?: EngineMeasurement | null;
  /** True when the dataset already exists and its binding cannot be changed here. */
  bound?: boolean;
  dateColumn: string;
  onDateColumn: (value: string) => void;
}) {
  const temporal = useMemo(() => temporalColumns(table), [table]);
  const derived = useMemo(() => derivedDateColumn(table), [table]);
  // A table can declare a time column that is not itself a date or timestamp
  // (an epoch integer, say), and a dataset can already carry one. Either must
  // stay selectable, or saving would silently clear it.
  const options = useMemo(() => {
    const named = new Set(temporal.map(column => column.name));
    const extra: { name: string; type: string }[] = [];
    for (const name of [dateColumn, table?.shape?.time?.column]) {
      if (!name || named.has(name)) continue;
      named.add(name);
      extra.push({ name, type: "declared" });
    }
    return [...temporal, ...extra];
  }, [temporal, dateColumn, table]);
  const columnCount = table?.columns?.length ?? 0;
  const shape = table?.shape;
  const rows = formatRows(measurement?.rows);
  const size = formatBytes(measurement?.bytes);

  if (!qualifiedName) {
    return (
      <div className={s.field}>
        <span className={s.label}>Source table</span>
        <p className={s.hint}>
          Choose a catalog, a schema and a table on the left. The dataset binds to that table by
          the id the Engine keeps for it, and takes its columns, metrics and date column from the
          Engine&rsquo;s definition.
        </p>
      </div>
    );
  }

  return (
    <>
      <div className={s.field}>
        <span className={s.label}>Source table</span>
        <div className={s.summary}>
          <p className={s.summaryName}>{qualifiedName}</p>
          <p className={s.summaryKind}>
            {bound
              ? "Bound to this Engine table. Create a new dataset to read a different one."
              : shape
              ? `Dimensions and measures come from the table's declared shape: ${
                  shape.dimensions?.length ?? 0
                } dimensions, ${shape.measures?.length ?? 0} measures.`
              : "Dimensions and metrics are derived from the column types when the dataset is created."}
          </p>
          {(columnCount > 0 || rows || size || table?.format) && (
            <div className={s.facts}>
              {table?.format && (
                <div>
                  <div className={s.factLabel}>Format</div>
                  <div className={s.factValue}>{table.format}</div>
                </div>
              )}
              {columnCount > 0 && (
                <div>
                  <div className={s.factLabel}>Columns</div>
                  <div className={s.factValue}>{columnCount}</div>
                </div>
              )}
              {rows && (
                <div>
                  <div className={s.factLabel}>Rows</div>
                  <div className={s.factValue}>{rows}</div>
                </div>
              )}
              {size && (
                <div>
                  <div className={s.factLabel}>Size</div>
                  <div className={s.factValue}>{size}</div>
                </div>
              )}
            </div>
          )}
          {table?.location && <p className={s.location}>{table.location}</p>}
          {measurement?.state === "unreadable" && (
            <p className={s.unreadable}>
              The Engine could not read this location: {measurement.error || "no detail was returned"}.
            </p>
          )}
        </div>
      </div>

      <div className={s.field}>
        <label className={s.label} htmlFor="dataset-engine-date-column">
          Date column
        </label>
        <select
          id="dataset-engine-date-column"
          className="save-query-input chart-builder-select"
          value={dateColumn}
          onChange={event => onDateColumn(event.target.value)}
          disabled={!table}
        >
          <option value="">
            {derived ? `From the Engine's definition — ${derived}` : "No date column"}
          </option>
          {options.map(column => (
            <option key={column.name} value={column.name}>
              {column.name} ({column.type})
            </option>
          ))}
        </select>
        <p className={s.hint}>
          {options.length === 0
            ? "This table declares no date or timestamp column."
            : "Left as it stands, the date column is the one the Engine's definition implies."}
        </p>
      </div>
    </>
  );
}
