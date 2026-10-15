"use client";

import React, { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useAuth } from "../../../auth/useAuth";
import { useRole } from "../../../hooks/useRole";
import { unknownText } from "../../../components/Reading";
import { useRouter, useSearchParams } from "next/navigation";
import { DataSourceSelector } from "../../../components/DataSourceSelector";
import { useTheme } from "../../../contexts/ThemeContext";
import {
  EngineCatalog,
  EngineTable,
  fetchEngineTable,
  fetchRegisteredCatalogs,
} from "../../../utils/engineCatalog";
import { EngineBinding, EngineBindingPanel, EngineTablePicker } from "./EngineSource";
import es from "./engineSource.module.css";

const PRIMARY_DB_NAME = process.env.NEXT_PUBLIC_PRIMARY_DATABASE_NAME || "";

// One identity for "no catalogs", so the picker's props do not change on every
// render of the page while the listing is still in flight.
const NO_CATALOGS: EngineCatalog[] = [];

interface DatabaseConfig {
  database: string;
  display_name: string;
  table_count?: number;
}

interface DataSource {
  id: number;
  name: string;
  type: string;
  connection_string: string;
  database_name: string;
  region: string;
  is_active: boolean;
  is_favorite?: number;
}

interface TableInfo {
  id: string;
  schema: string;
  name: string;
  fullName?: string;
}

interface ColumnInfo {
  name: string;
  dataType: string;
}

interface DimensionMapping {
  id: string;
  schema: string;
  name: string;
  include: boolean;
  factKey?: string | null;
  dimKey?: string | null;
  dimColumns: ColumnInfo[];
  selectedColumns: string[];
}

interface DatasetFilterRow {
  id: string;
  column: string;
  value: string | "";
  options: string[];
  isLoading: boolean;
}

interface DatasetMetricRow {
  id: string;
  name: string;
  column: string;
  agg: string;
  /**
   * The expression as the store holds it, kept only for a metric this form
   * could not take apart into an aggregate and a column — `COUNT(DISTINCT x)`,
   * `SUM(a)/SUM(b)`, a CASE. Such a metric used to seed with an empty column,
   * which the submit filter then dropped from the payload, and the API
   * replaces metrics wholesale: saving any edit deleted it. It is sent back
   * unchanged instead.
   */
  expression?: string;
}

interface DatasetDimensionDTO {
  dimension_table: string;
  join_condition: string;
  display_name?: string | null;
}

interface DatasetFilterDTO {
  column: string;
  op?: string;
  value: string | string[];
}

interface DatasetMetricDTO {
  name: string;
  expression: string;
  metric_type: string;
  format?: string | null;
}

interface DatasetColumnDTO {
  table_name: string;
  column_name: string;
  data_type: string;
  is_dimension: boolean;
  is_metric: boolean;
  semantic_type?: string | null;
}

interface DatasetDetailDTO {
  id: number;
  name: string;
  description?: string | null;
  table_name: string;
  date_column?: string | null;
  schema_name?: string | null;
  database_name?: string | null;
  /** `{kind: "engine", table_id}` for a dataset bound to an Engine catalog table. */
  source?: { kind: string; table_id: string } | null;
  dimensions?: DatasetDimensionDTO[];
  filters?: DatasetFilterDTO[];
  metrics?: DatasetMetricDTO[];
  columns?: DatasetColumnDTO[];
}

type DatasetMode = "flat" | "semantic";

function guessKeysForDimension(
  mapping: DimensionMapping,
  dimColumns: ColumnInfo[],
  factColumns: ColumnInfo[] | null,
): { factKey?: string | null; dimKey?: string | null } {
  if (!factColumns || factColumns.length === 0 || dimColumns.length === 0) {
    return {};
  }

  const isKey = (name: string) => {
    const lower = name.toLowerCase();
    return lower.endsWith("key") || lower.endsWith("id");
  };

  const factKeyCandidates = factColumns.filter((c) => isKey(c.name));
  const dimKeyCandidates = dimColumns.filter((c) => isKey(c.name));

  if (factKeyCandidates.length === 0 || dimKeyCandidates.length === 0) {
    return {};
  }

  // Try to match on identical column names first.
  for (const f of factKeyCandidates) {
    const match = dimKeyCandidates.find((d) => d.name.toLowerCase() === f.name.toLowerCase());
    if (match) {
      return { factKey: f.name, dimKey: match.name };
    }
  }

  // Fall back to the first key-looking columns on each side.
  return {
    factKey: factKeyCandidates[0]?.name ?? null,
    dimKey: dimKeyCandidates[0]?.name ?? null,
  };
}

  function parseJoinConditionKeys(joinCondition: string): {
    factKey?: string | null;
    dimKey?: string | null;
  } {
    if (!joinCondition) return {};
    const regex =
      /\[(?<factSchema>[^\]]+)\]\.\[(?<factTable>[^\]]+)\]\.\[(?<factKey>[^\]]+)\]\s*=\s*\[(?<dimSchema>[^\]]+)\]\.\[(?<dimTable>[^\]]+)\]\.\[(?<dimKey>[^\]]+)\]/;
    const match = joinCondition.match(regex) as
      | (RegExpMatchArray & { groups?: { [key: string]: string } })
      | null;
    if (!match || !match.groups) return {};
    return {
      factKey: match.groups["factKey"] ?? null,
      dimKey: match.groups["dimKey"] ?? null,
    };
  }

function guessDimensionTableForFactKey(
  factKey: string,
  tables: TableInfo[],
): TableInfo | undefined {
  if (!factKey || tables.length === 0) return undefined;

  let base = factKey.toLowerCase();
  if (base.endsWith("key")) {
    base = base.slice(0, -3);
  } else if (base.endsWith("id")) {
    base = base.slice(0, -2);
  }

  const normalizeTableName = (name: string) => {
    let n = name.toLowerCase();
    if (n.startsWith("dim")) {
      n = n.slice(3);
    }
    return n;
  };

  const candidates = tables.map((t) => ({ table: t, norm: normalizeTableName(t.name) }));

  let match = candidates.find((c) => c.norm === base);
  if (match) return match.table;

  match = candidates.find((c) => base.endsWith(c.norm) || c.norm.endsWith(base));
  if (match) return match.table;

  match = candidates.find((c) => base.startsWith(c.norm) || c.norm.startsWith(base));
  if (match) return match.table;

  return undefined;
}

export default function NewDatasetPage() {
  const { isAuthenticated, account } = useAuth();
  // This form POSTs a dataset, or PATCHes one when ?datasetId= is present.
  // Both routes sit above a Viewer: the create floor is Analyst and the
  // update is `can_write`, which is false for every Viewer. So a read-only
  // role is told here rather than at the end of the form.
  const { canCreate, whyNoCreate } = useRole();
  const { primaryColor } = useTheme();
  const router = useRouter();
  const searchParams = useSearchParams();

  const [dataSources, setDataSources] = useState<DataSource[]>([]);
  const [dataSourcesLoaded, setDataSourcesLoaded] = useState(false);
  const [currentDataSourceId, setCurrentDataSourceId] = useState<number | null>(null);
  const [databases, setDatabases] = useState<DatabaseConfig[]>([]);
  const [currentDatabase, setCurrentDatabase] = useState<string | null>(null);
  const [tables, setTables] = useState<TableInfo[]>([]);
  const [isLoadingDatabases, setIsLoadingDatabases] = useState(false);
  const [isLoadingTables, setIsLoadingTables] = useState(false);
  /**
   * A re-read of the schemas and tables that did not answer. Held apart from
   * `loadError`, which hides the whole form: a refresh that failed leaves
   * everything already on screen valid, and the previous list standing.
   */
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [selectedSchema, setSelectedSchema] = useState<string | null>(null);
  const [selectedTableId, setSelectedTableId] = useState<string | null>(null);

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [dateColumn, setDateColumn] = useState<string>("");
  const [mode, setMode] = useState<DatasetMode>("flat");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const [factColumns, setFactColumns] = useState<ColumnInfo[] | null>(null);
  const [isLoadingFactColumns, setIsLoadingFactColumns] = useState(false);
  const [factColumnsError, setFactColumnsError] = useState<string | null>(null);
  const [dimensionTables, setDimensionTables] = useState<TableInfo[]>([]);
  const [dimensionMappings, setDimensionMappings] = useState<DimensionMapping[]>([]);
  const [isLoadingDimensions, setIsLoadingDimensions] = useState(false);
  const [openDimValueId, setOpenDimValueId] = useState<string | null>(null);
  const [filters, setFilters] = useState<DatasetFilterRow[]>([]);
  const [metrics, setMetrics] = useState<DatasetMetricRow[]>([]);
  const [editingDatasetId, setEditingDatasetId] = useState<number | null>(null);
  const [editingDataset, setEditingDataset] = useState<DatasetDetailDTO | null>(null);
  const [hasSeededFromDataset, setHasSeededFromDataset] = useState(false);
  const [dimensionSchema, setDimensionSchema] = useState<string | null>(null);

  // ── Where the dataset reads from ───────────────────────────────────────────
  // Two kinds of source exist and they are not interchangeable. A `database`
  // dataset reads an external connection (Fabric SQL, Azure SQL, PostgreSQL,
  // MySQL) through the Lab's metadata endpoints. An `engine` dataset binds to
  // one table of a registered catalog by the id the Engine keeps for it, and
  // the API resolves its names, columns, metrics and date column from the
  // Engine's definition. A lake-backed installation has only the second kind,
  // so the form cannot assume the first: the path is chosen from what is
  // actually registered, and said plainly when neither kind is.
  const [sourceKind, setSourceKind] = useState<"engine" | "database" | null>(null);
  const [engineCatalogs, setEngineCatalogs] = useState<EngineCatalog[] | null>(null);
  const [engineUnsynchronized, setEngineUnsynchronized] = useState<string[]>([]);
  const [engineCatalogError, setEngineCatalogError] = useState<string | null>(null);
  const [engineBinding, setEngineBinding] = useState<EngineBinding | null>(null);
  const [boundEngineTable, setBoundEngineTable] = useState<EngineTable | null>(null);
  const lastEngineTableId = useRef<string | null>(null);

  // The picker emits on every change of its resolved binding, so the callback
  // it is given has to be stable. A different table also invalidates a date
  // column chosen for the previous one.
  const handleEngineBinding = useCallback((binding: EngineBinding | null) => {
    setEngineBinding(binding);
    const tableId = binding?.table.id ?? null;
    if (lastEngineTableId.current !== null && lastEngineTableId.current !== tableId) {
      setDateColumn("");
    }
    lastEngineTableId.current = tableId;
    // A dataset named after the table it binds is the sane default; anything
    // the reader has typed is left alone.
    if (binding) setName(current => (current.trim() ? current : binding.table.name));
  }, []);

  const schemaOptions = useMemo(() => {
    const unique = new Set<string>();
    for (const t of tables) {
      if (t.schema) unique.add(t.schema);
    }
    return Array.from(unique).sort((a, b) => a.localeCompare(b));
  }, [tables]);

  const filteredTables = useMemo(() => {
    if (!selectedSchema) return tables;
    return tables.filter((t) => t.schema === selectedSchema);
  }, [tables, selectedSchema]);

  const selectedTable = useMemo(() => {
    if (!selectedTableId) return null;
    return tables.find((t) => t.id === selectedTableId) || null;
  }, [tables, selectedTableId]);

  // Detect optional datasetId query for editing mode
  useEffect(() => {
    const idStr = searchParams?.get('datasetId');
    if (!idStr) return;
    const idNum = Number(idStr);
    if (!Number.isFinite(idNum)) return;
    setEditingDatasetId(idNum);
  }, [searchParams]);

  // The catalogs a dataset may be bound to. Derived from the platform's
  // catalog *source* records and resolved to the Engine's definitions by
  // name, rather than read from the Engine's definitions directly: a
  // benchmark campaign registers scratch catalogs straight against the
  // Engine, and those are not a source anybody means to build a dataset on.
  useEffect(() => {
    if (!isAuthenticated) return;
    let cancelled = false;
    setEngineCatalogError(null);
    fetchRegisteredCatalogs()
      .then(({ catalogs, unsynchronized }) => {
        if (cancelled) return;
        setEngineCatalogs(catalogs);
        setEngineUnsynchronized(unsynchronized);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setEngineCatalogs([]);
        setEngineCatalogError(
          error instanceof Error ? error.message : "The registered catalogs could not be read.",
        );
      });
    return () => {
      cancelled = true;
    };
  }, [isAuthenticated]);

  // Which path the form takes. One kind of source registered decides it; both
  // registered is a question for the reader; neither is reported rather than
  // drawn as an empty picker. An existing dataset decides it by its binding.
  useEffect(() => {
    if (sourceKind) return;
    if (editingDatasetId) return;
    if (engineCatalogs === null || !dataSourcesLoaded) return;
    if (engineCatalogs.length > 0 && dataSources.length === 0) setSourceKind("engine");
    else if (dataSources.length > 0 && engineCatalogs.length === 0) setSourceKind("database");
  }, [sourceKind, editingDatasetId, engineCatalogs, dataSourcesLoaded, dataSources.length]);

  useEffect(() => {
    if (!isAuthenticated) return;
    if (!selectedTable) {
      setFactColumns(null);
      setFactColumnsError(null);
      return;
    }

    const loadColumns = async () => {
      try {
        setIsLoadingFactColumns(true);
        setFactColumnsError(null);
        const schemaEncoded = encodeURIComponent(selectedTable.schema);
        const nameEncoded = encodeURIComponent(selectedTable.name);

        const params = new URLSearchParams();
        if (currentDatabase) {
          params.append('database', currentDatabase);
        }
        const queryString = params.toString() ? `?${params.toString()}` : '';

        const res = await msalFetch(`${API_BASE}/api/v1/lab/schema/${schemaEncoded}/${nameEncoded}${queryString}`);
        const data = await res.json();
        if (!res.ok || !data.success) {
          throw new Error(data.error || `Failed to load columns (${res.status})`);
        }
        const cols: ColumnInfo[] = (data.schema?.columns || []).map((c: any) => ({
          name: c.name,
          dataType: c.dataType,
        }));
        setFactColumns(cols);
        setFactColumnsError(null);
      } catch (e) {
        const errorMsg = e instanceof Error ? e.message : "Unknown error loading columns";
        setFactColumns(null);
        setFactColumnsError(errorMsg);
      } finally {
        setIsLoadingFactColumns(false);
      }
    };

    void loadColumns();
  }, [isAuthenticated, selectedTable]);

  useEffect(() => {
    if (!isAuthenticated) return;

    // Skip auto-database-switching when editing - check searchParams directly BEFORE async
    const idStr = searchParams?.get('datasetId');
    if (idStr) {
      console.log('[Datasets] Skipping loadInitial - editing existing dataset (datasetId in URL)');
      return;
    }

    const loadInitial = async () => {
      try {
        setIsLoadingDatabases(true);
        setLoadError(null);

        // Load data sources from the new data_sources table
        const userEmail = account?.email || account?.username || null;
        const res = await msalFetch(`${API_BASE}/api/v1/data-sources/active`, {
          headers: userEmail ? { 'x-user-email': userEmail } : undefined,
        });
        if (!res.ok) {
          throw new Error("Failed to load data sources");
        }
        const data = await res.json();
        const sources: DataSource[] = data.dataSources || [];

        setDataSources(sources);

        // Auto-select favorite data source, or first available (only for new datasets)
        const favorite = sources.find((ds) => ds.is_favorite === 1);
        const preferred = favorite || sources[0];

        if (preferred) {
          setCurrentDataSourceId(preferred.id);
          await switchDatabase(preferred.database_name);
        }
      } catch (e: unknown) {
        const message = e instanceof Error ? e.message : "Unknown error";
        setLoadError(message);
      } finally {
        // Read, successfully or not: the source-kind decision waits on this
        // answer and must not be held up by a failed one.
        setDataSourcesLoaded(true);
        setIsLoadingDatabases(false);
      }
    };

    void loadInitial();
  }, [isAuthenticated, searchParams]);

  // If editing, load the existing dataset definition
  useEffect(() => {
    if (!isAuthenticated) return;
    if (!editingDatasetId) return;

    const loadDataset = async () => {
      try {
        // Acquire access token for API
        const res = await msalFetch(`${API_BASE}/api/v1/datasets/${editingDatasetId}`);
        if (!res.ok) {
          throw new Error(`Failed to load dataset (${res.status})`);
        }
        const data: DatasetDetailDTO = await res.json();

        // Debug: Log what we received from API
        console.log('[Dataset Edit] Loaded dataset:', {
          name: data.name,
          hasDimensions: !!data.dimensions,
          dimensionCount: data.dimensions?.length ?? 0,
          hasColumns: !!data.columns,
          columnCount: data.columns?.length ?? 0,
          hasMetrics: !!data.metrics,
          metricCount: data.metrics?.length ?? 0
        });

        // Set all basic fields immediately - no waiting
        setEditingDataset(data);
        setName(data.name || "");
        setDescription(data.description || "");
        setDateColumn(data.date_column || "");

        // Set mode immediately so UI updates right away
        if (data.dimensions && data.dimensions.length > 0) {
          console.log('[Dataset Edit] Setting mode to SEMANTIC - has', data.dimensions.length, 'dimensions');
          setMode("semantic");

          // Extract the schema from the first dimension and set it
          // This ensures dimension tables are loaded from the correct schema
          const firstDim = data.dimensions[0];
          if (firstDim?.dimension_table) {
            const [schema] = firstDim.dimension_table.split(".", 2);
            if (schema) {
              console.log('[Dataset Edit] Setting dimension schema from saved dimensions:', schema);
              setDimensionSchema(schema);
            }
          }
        } else {
          console.log('[Dataset Edit] Setting mode to FLAT - no dimensions found');
          setMode("flat");
        }

        // Seed dataset-level filters immediately
        const seedFilters: DatasetFilterRow[] = (data.filters || []).map((f, idx) => {
          const rawValue =
            typeof f.value === "string"
              ? f.value
              : Array.isArray(f.value) && f.value.length > 0
              ? String(f.value[0])
              : "";

          return {
            id: `existing-filter-${idx}`,
            column: f.column,
            value: rawValue,
            options: [],
            isLoading: false,
          };
        });
        if (seedFilters.length > 0) {
          setFilters(seedFilters);
        }

        // Seed metrics immediately
        const seedMetrics: DatasetMetricRow[] = (data.metrics || []).map((m, idx) => {
          let agg = (m.metric_type || "sum").toUpperCase();
          const supportedAggs = ["SUM", "AVG", "COUNT", "MIN", "MAX"];
          if (!supportedAggs.includes(agg)) {
            agg = "SUM";
          }

          let column = "";
          if (m.expression) {
            const match = m.expression.match(/^\s*([A-Za-z]+)\s*\((.+)\)\s*$/);
            if (match) {
              const exprAgg = match[1].toUpperCase();
              const inner = match[2].trim();
              if (supportedAggs.includes(exprAgg)) {
                agg = exprAgg;
              }

              let innerCol = inner;
              const parts = innerCol.split(".");
              if (parts.length > 1) {
                innerCol = parts[parts.length - 1];
              }
              const colMatch = innerCol.match(/\[([^\]]+)\]/);
              if (colMatch) {
                column = colMatch[1];
              } else {
                column = innerCol.replace(/[\[\]]/g, "");
              }
            }
          }

          return {
            id: `existing-metric-${idx}`,
            name: m.name,
            column,
            agg,
            expression: column ? undefined : (m.expression || undefined),
          };
        });
        if (seedMetrics.length > 0) {
          setMetrics(seedMetrics);
        }

        // An Engine-backed dataset has no external connection to switch to:
        // its names live in the Engine's catalog, so the Lab's metadata
        // endpoints would be asked about a database that does not exist.
        // Read the bound table's definition instead — it is what the date
        // column is chosen from.
        const engineTableId =
          data.source && data.source.kind === "engine" ? data.source.table_id : null;
        if (engineTableId) {
          setSourceKind("engine");
          void fetchEngineTable(engineTableId)
            .then(setBoundEngineTable)
            .catch(() => setBoundEngineTable(null));
          return;
        }
        setSourceKind("database");

        // Load database/tables in parallel (non-blocking)
        // Dimensions will populate once this completes via the other useEffect
        if (data.database_name) {
          console.log('[Dataset Edit] About to call switchDatabase:', {
            database: data.database_name,
            schema: data.schema_name,
            table: data.table_name
          });
          void switchDatabase(data.database_name, data.schema_name ?? null, data.table_name ?? null);
        }
      } catch (e) {
        console.error("Failed to load dataset for editing", e);
      }
    };

    void loadDataset();
  }, [isAuthenticated, editingDatasetId]);

  // When editing, ensure filter value dropdowns are populated with all
  // distinct values for the selected column, not just the saved value.
  // Fetch filter options for filters that have a column but no options yet
  // This runs once when selectedTable AND currentDatabase are set (e.g., when editing an existing dataset)
  useEffect(() => {
    if (!isAuthenticated) return;
    if (!selectedTable) return;
    if (!currentDatabase) return; // Wait for database to be set

    const pending = filters.filter(
      (f) => f.column && !f.isLoading && f.options.length === 0,
    );
    if (pending.length === 0) return;

    pending.forEach((f) => {
      (async () => {
        try {
          // Mark as loading
          setFilters((prev) =>
            prev.map((row) =>
              row.id === f.id ? { ...row, isLoading: true } : row,
            ),
          );

          const schemaEncoded = encodeURIComponent(selectedTable.schema);
          const tableEncoded = encodeURIComponent(selectedTable.name);
          const colEncoded = encodeURIComponent(f.column);

          // Pass database parameter to query the correct database
          const databaseParam = `?database=${encodeURIComponent(currentDatabase)}`;
          console.log(`[Dataset Filters] Fetching distinct values for ${selectedTable.schema}.${selectedTable.name}.${f.column} from database: ${currentDatabase}`);

          const res = await msalFetch(
            `${API_BASE}/api/v1/lab/distinct/${schemaEncoded}/${tableEncoded}/${colEncoded}${databaseParam}`,
          );
          const data = await res.json();
          if (!res.ok || !data.success) {
            console.error(`[Dataset Filters] Failed to fetch distinct values:`, data.error || 'Unknown error');
            throw new Error(data.error || "Failed to load values");
          }
          console.log(`[Dataset Filters] Loaded ${data.values?.length || 0} distinct values for ${f.column}`);
          const values: string[] = Array.isArray(data.values)
            ? data.values
                .map((v: any) => {
                  if (v == null) return "";
                  if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") {
                    return String(v);
                  }
                  if (typeof v === "object" && "value" in v && v.value != null) {
                    return String((v as any).value);
                  }
                  return "";
                })
                .filter((s: string) => s !== "")
            : [];

          setFilters((prev) =>
            prev.map((row) =>
              row.id === f.id
                ? {
                    ...row,
                    options: values,
                    isLoading: false,
                  }
                : row,
            ),
          );
        } catch (error) {
          console.error("Failed to load filter options:", error);
          setFilters((prev) =>
            prev.map((row) =>
              row.id === f.id
                ? { ...row, options: [], isLoading: false }
                : row,
            ),
          );
        }
      })();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isAuthenticated, selectedTable, currentDatabase]);

  const switchDatabase = async (
    databaseName: string,
    preferredSchema: string | null = null,
    preferredTable: string | null = null,
  ) => {
    console.log('[Datasets] switchDatabase called with:', {
      databaseName,
      preferredSchema,
      preferredTable,
      callerStack: new Error().stack?.split('\n').slice(2, 4).join('\n')
    });
    try {
      setIsLoadingTables(true);
      setLoadError(null);

      const res = await msalFetch(`${API_BASE}/api/v1/lab/switch-database`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ database_name: databaseName }),
      });
      const data = await res.json();
      if (!res.ok || !data.success) {
        throw new Error(data.error || "Failed to switch database");
      }

  setCurrentDatabase(databaseName);
  await loadTables(false, preferredSchema, preferredTable, databaseName);
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      setLoadError(message);
    } finally {
      setIsLoadingTables(false);
    }
  };

  const loadTables = async (
    refresh: boolean = false,
    preferredSchema: string | null = null,
    preferredTable: string | null = null,
    databaseName?: string,
  ) => {
    const dbToUse = databaseName || currentDatabase;

    if (!dbToUse) {
      console.warn('[Datasets] No database selected, skipping table load');
      return;
    }

    const params = new URLSearchParams();
    if (refresh) params.append('refresh', '1');
    params.append('database', dbToUse);

    const queryString = params.toString() ? `?${params.toString()}` : '';
    console.log(`[Datasets] Loading tables from database: ${dbToUse}`, {
      preferredSchema,
      preferredTable,
      refresh,
      calledFrom: new Error().stack?.split('\n')[2]?.trim()
    });

    const res = await msalFetch(`${API_BASE}/api/v1/lab/tables${queryString}`);
    const data = await res.json();
    if (!res.ok || !data.success) {
      throw new Error(data.error || "Failed to load tables");
    }

    const withIds: TableInfo[] = (data.tables || []).map((t: any, idx: number) => ({
      id: t.id || `${t.schema}.${t.name}.${idx}`,
      schema: t.schema,
      name: t.name,
      fullName: t.fullName,
    }));

    console.log(`[Datasets] Loaded ${withIds.length} tables from ${dbToUse}`);
    setTables(withIds);

    // Reset selection when tables change, preferring an explicit target when provided
    if (withIds.length > 0) {
      let target: TableInfo | undefined;

      if (preferredSchema || preferredTable) {
        console.log(`[Datasets] Looking for table:`, { preferredSchema, preferredTable });
        target = withIds.find((t) => {
          const schemaMatches = preferredSchema ? t.schema === preferredSchema : true;
          const tableMatches = preferredTable ? t.name === preferredTable : true;
          return schemaMatches && tableMatches;
        });

        if (target) {
          console.log(`[Datasets] Found matching table:`, { schema: target.schema, name: target.name });
        } else {
          console.warn(`[Datasets] No matching table found! Falling back to first table.`, {
            looking_for: { schema: preferredSchema, table: preferredTable },
            available_tables: withIds.slice(0, 5).map(t => ({ schema: t.schema, name: t.name }))
          });
        }
      } else if (selectedTableId) {
        // If no preferred table specified, but we already have a selection, keep it!
        // This prevents resetting the table when loadTables is called without preferred params
        const currentTable = withIds.find(t => t.id === selectedTableId);
        if (currentTable) {
          console.log(`[Datasets] Keeping current selection (no preferred table specified):`, {
            id: currentTable.id,
            schema: currentTable.schema,
            name: currentTable.name
          });
          return; // Don't change selection
        }
      }

      const first = target ?? withIds[0];
      console.log(`[Datasets] Setting selected table:`, {
        id: first.id,
        schema: first.schema,
        name: first.name,
        wasTarget: !!target,
        fallbackToFirst: !target
      });
      setSelectedSchema(first.schema || null);
      setSelectedTableId(first.id);
    } else {
      setSelectedSchema(null);
      setSelectedTableId(null);
    }
  };

  /**
   * The refresh behind both re-read buttons. `loadTables` throws, and the
   * buttons called it as a bare `void` — so a refused re-read became an
   * unhandled rejection, set no state, and left a control that looked as
   * though it had done nothing at all.
   */
  const refreshTables = async () => {
    setRefreshError(null);
    setIsLoadingTables(true);
    try {
      await loadTables(true);
    } catch (e: unknown) {
      setRefreshError(e instanceof Error
        ? e.message
        : "The schemas and tables could not be re-read. The list below is the last one that answered.");
    } finally {
      setIsLoadingTables(false);
    }
  };

  const loadDimensionCandidates = async (schema?: string) => {
    if (!isAuthenticated) return;
    if (!schema) return;

    try {
      setIsLoadingDimensions(true);

      const params = new URLSearchParams();
      params.append('schema', schema);
      if (currentDatabase) {
        params.append('database', currentDatabase);
      }

      const res = await msalFetch(`${API_BASE}/api/v1/lab/tables?${params.toString()}`);
      const data = await res.json();
      if (!res.ok || !data.success) {
        throw new Error(data.error || "Failed to load dimension tables");
      }

      const dims: TableInfo[] = (data.tables || []).map((t: any, idx: number) => ({
        id: t.id || `${t.schema}.${t.name}.${idx}`,
        schema: t.schema,
        name: t.name,
        fullName: t.fullName,
      }));

      // Filter out the selected table itself from dimension candidates
      const filteredDims = dims.filter(d => d.id !== selectedTableId);
      setDimensionTables(filteredDims);
    } catch {
      setDimensionTables([]);
    } finally {
      setIsLoadingDimensions(false);
    }
  };

  // Auto-select Dims schema if it exists, otherwise use the selected table's schema
  useEffect(() => {
    if (schemaOptions.length === 0) return;
    if (dimensionSchema !== null) return; // Already set
    if (!selectedTable) return;

    const hasDims = schemaOptions.some(s => s.toLowerCase() === 'dims');
    if (hasDims) {
      const dimsSchema = schemaOptions.find(s => s.toLowerCase() === 'dims');
      setDimensionSchema(dimsSchema || null);
    } else {
      // Default to the fact table's schema if Dims doesn't exist
      setDimensionSchema(selectedTable.schema);
    }
  }, [schemaOptions, dimensionSchema, selectedTable]);

  useEffect(() => {
    if (mode !== "semantic") {
      // Clear mappings when leaving semantic mode
      if (dimensionMappings.length > 0) {
        setDimensionMappings([]);
      }
      return;
    }

    // Wait for selectedTable to be set before loading dimension tables
    if (!selectedTable) return;

    // Wait for dimensionSchema to be determined
    if (dimensionSchema === null) return;

    // Only load if dimension tables are not yet populated
    if (dimensionTables.length > 0) return;

    console.log('[Dimension Tables] Loading dimension tables for schema:', dimensionSchema);
    void loadDimensionCandidates(dimensionSchema);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, dimensionSchema, selectedTable, dimensionTables.length]);

  const ensureDimColumnsLoaded = async (mapping: DimensionMapping) => {
    if (mapping.dimColumns.length > 0) return;

    try {
      const schemaEncoded = encodeURIComponent(mapping.schema);
      const nameEncoded = encodeURIComponent(mapping.name);

      const params = new URLSearchParams();
      if (currentDatabase) {
        params.append('database', currentDatabase);
      }
      const queryString = params.toString() ? `?${params.toString()}` : '';

      const res = await msalFetch(`${API_BASE}/api/v1/lab/schema/${schemaEncoded}/${nameEncoded}${queryString}`);
      const data = await res.json();
      if (!res.ok || !data.success) {
        throw new Error(data.error || "Failed to load dimension columns");
      }

      const cols: ColumnInfo[] = (data.schema?.columns || []).map((c: any) => ({
        name: c.name,
        dataType: c.dataType,
      }));

      setDimensionMappings((prev) =>
        prev.map((d) =>
          d.id === mapping.id
            ? (() => {
                const guess = guessKeysForDimension(d, cols, factColumns);
                return {
                  ...d,
                  dimColumns: cols,
                  factKey: d.factKey ?? guess.factKey ?? d.factKey ?? null,
                  dimKey: d.dimKey ?? guess.dimKey ?? d.dimKey ?? null,
                  selectedColumns: d.selectedColumns || [],
                };
              })()
            : d,
        ),
      );
    } catch {
      // Ignore for now; user can retry by toggling include.
    }
  };

  /**
   * Create or change an Engine-backed dataset.
   *
   * The payload names the table by the Engine's id and says nothing else
   * about it: `services/engine_datasets.apply_binding` resolves the catalog,
   * schema and table names, the column list, the metrics and the date column
   * from the Engine's own definition. That is the point of binding by id — a
   * dataset cannot name one table and read another — so nothing here is
   * typed, and a date column the reader chose is the one field that travels.
   *
   * An existing dataset's binding is not changed here. Its stored columns and
   * metrics were derived from the table it is bound to, and rebinding would
   * leave them describing a different one.
   */
  const submitEngineDataset = async () => {
    const tableId = engineBinding?.table.id ?? null;
    if (!editingDatasetId && !tableId) {
      setSubmitError("Choose a catalog, a schema and a table for this dataset to read.");
      return;
    }

    setIsSubmitting(true);
    setSubmitError(null);

    try {
      const payload: Record<string, unknown> = {
        name: name.trim(),
        description: description.trim() || null,
        date_column: dateColumn || null,
      };
      if (!editingDatasetId && tableId) {
        payload.source = { kind: "engine", table_id: tableId };
      }

      const url = editingDatasetId
        ? `${API_BASE}/api/v1/datasets/${editingDatasetId}`
        : `${API_BASE}/api/v1/datasets`;
      const headers: Record<string, string> = { "Content-Type": "application/json" };
      const userEmail = account?.email || account?.username || null;
      if (userEmail) {
        headers["x-user-email"] = userEmail;
      }

      const res = await msalFetch(url, {
        method: editingDatasetId ? "PATCH" : "POST",
        headers,
        body: JSON.stringify(payload),
      });
      const body = await res.json().catch(() => null);
      if (!res.ok) {
        const detail = (body as { detail?: unknown } | null)?.detail;
        const detailMessage =
          typeof detail === "string"
            ? detail
            : detail && typeof detail === "object"
            ? String((detail as { message?: unknown }).message ?? "")
            : "";
        throw new Error(
          detailMessage ||
            (body as { error?: string } | null)?.error ||
            `Failed to save dataset (${res.status})`,
        );
      }

      const created = (body as { id?: string | number } | null)?.id;
      const datasetId =
        typeof created === "string" || typeof created === "number"
          ? String(created)
          : editingDatasetId
          ? String(editingDatasetId)
          : null;
      // `replace`, not `push` — the same rule the database path below states
      // and explains: the creation step is finished and must not stay in
      // history, or Back from the saved dataset returns into the form that
      // created it and submitting again creates a second dataset. This path
      // was pushing.
      void router.replace(datasetId ? `/datasets/${datasetId}` : "/datasets");
    } catch (e: unknown) {
      setSubmitError(e instanceof Error ? e.message : "Failed to save dataset");
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleSubmit = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!isAuthenticated) return;

    // Validate required fields
    if (!name.trim()) {
      setSubmitError("Dataset name is required");
      return;
    }
    if (sourceKind === "engine") {
      await submitEngineDataset();
      return;
    }
    if (!selectedTable) {
      setSubmitError("Please select a table");
      return;
    }
    if (!currentDatabase) {
      setSubmitError("Database is required");
      return;
    }

    setIsSubmitting(true);
    setSubmitError(null);

    try {
      const semanticDims =
        mode === "semantic"
          ? dimensionMappings
              .filter((d) => d.include && d.factKey && d.dimKey && d.schema && d.name)
              .map((d) => {
                const displayName = d.name.startsWith("Dim") ? d.name.substring(3) : d.name;
                const factKey = d.factKey as string;
                const dimKey = d.dimKey as string;
                const factSchema = selectedTable.schema;
                const factTableName = selectedTable.name;
                const joinCondition =
                  `[${factSchema}].[${factTableName}].[${factKey}] = ` +
                  `[${d.schema}].[${d.name}].[${dimKey}]`;
                return {
                  dimension_table: `${d.schema}.${d.name}`,
                  join_condition: joinCondition,
                  display_name: displayName.trim() || d.name,
                };
              })
          : [];

      const filterPayload = filters
        .filter((f) => f.column && f.value)
        .map((f) => ({
          column: f.column,
          op: "=",
          value: f.value,
        }));

      const metricPayload = metrics
        .filter((m) => m.name.trim() && (m.column || m.expression))
        .map((m) => {
          const agg = (m.agg || "SUM").toUpperCase();
          // A metric this form could not parse keeps the expression it came
          // with. Rebuilding it from an empty column would write
          // `SUM([])`, and dropping it would delete it.
          if (!m.column && m.expression) {
            return {
              name: m.name.trim(),
              expression: m.expression,
              metric_type: agg.toLowerCase(),
              format: null as string | null,
            };
          }
          const safeColumn = m.column.replace(/[\[\]]/g, "");
          const expression = `${agg}([${safeColumn}])`;
          return {
            name: m.name.trim(),
            expression,
            metric_type: agg.toLowerCase(),
            format: null as string | null,
          };
        });

      const columnPayload: {
        table_name: string;
        column_name: string;
        data_type: string;
        is_dimension: boolean;
        is_metric: boolean;
        semantic_type: string | null;
      }[] = [];

      // Track dimension columns we've already added.
      // IMPORTANT: Include the fact key in the uniqueness check so that
      // when multiple fact keys point to the same dimension table
      // (e.g. IsMSFTTenantKey and IsStrategicCustomerKey both → IDEASBoolFlag),
      // we create separate columns for each fact key mapping.
      const seenDimColumns = new Set<string>();

      if (mode === "semantic") {
        dimensionMappings
          .filter((d) => d.include && d.schema && d.name)
          .forEach((d) => {
            const tableName = `${d.schema}.${d.name}`;
            const dimCols = d.dimColumns || [];
            const selected = d.selectedColumns || [];
            const factKey = (d.factKey || "").trim();

            selected.forEach((colName) => {
              // Include factKey in the uniqueness check to allow multiple fact columns
              // mapping to the same dimension table to create separate schema columns
              const key = `${tableName}|${colName}|${factKey}`;
              if (seenDimColumns.has(key)) {
                return;
              }
              seenDimColumns.add(key);
              const meta = dimCols.find((c) => c.name === colName);
              let aliasBase = factKey;
              const lower = aliasBase.toLowerCase();
              if (lower.endsWith("key")) {
                aliasBase = aliasBase.slice(0, -3);
              } else if (lower.endsWith("id")) {
                aliasBase = aliasBase.slice(0, -2);
              }
              const semanticAlias = aliasBase && aliasBase !== colName ? aliasBase : null;
              columnPayload.push({
                table_name: tableName,
                column_name: colName,
                data_type: meta?.dataType || "nvarchar", // fallback
                is_dimension: true,
                is_metric: false,
                semantic_type: semanticAlias,
              });
            });
          });
      }

      if (factColumns && selectedTable) {
        const factTableName = `${selectedTable.schema}.${selectedTable.name}`;
        const metricCols = new Set(
          metrics
            .filter((m) => m.column)
            .map((m) => m.column),
        );
        factColumns.forEach((c) => {
          if (metricCols.has(c.name)) {
            columnPayload.push({
              table_name: factTableName,
              column_name: c.name,
              data_type: c.dataType,
              is_dimension: false,
              is_metric: true,
              semantic_type: null,
            });
          }
        });

        if (dateColumn) {
          const meta = factColumns.find((c) => c.name === dateColumn);
          columnPayload.push({
            table_name: factTableName,
            column_name: dateColumn,
            data_type: meta?.dataType || "datetime",
            is_dimension: false,
            is_metric: false,
            semantic_type: "time",
          });
        }
      }

      const payload: Record<string, unknown> = {
        name: name.trim(),
        description: description.trim() || null,
        table_name: selectedTable.name,
        schema_name: selectedTable.schema,
        database_name: currentDatabase,
        date_column: dateColumn || null,
        // For semantic datasets we send dimension join metadata; for flat datasets this is empty.
        dimensions: semanticDims,
        filters: filterPayload,
        columns: columnPayload,
        metrics: metricPayload,
      };

      // The API applies `exclude_unset`: a collection left out of a PATCH is
      // left as it stands, while one sent as `[]` is an explicit delete —
      // `_replace_columns(..., delete_first=True)`. This form can only speak
      // for a collection it was able to rebuild, and all four of these are
      // rebuilt from the table's columns. When that read did not answer,
      // `factColumns` is null and every one of them is empty because nothing
      // was read, not because the dataset holds nothing — and Update was
      // still enabled, so pressing it deleted the dataset's columns,
      // dimension joins, metrics and filters. They are omitted instead, and
      // the dataset keeps what it has.
      if (editingDatasetId && !factColumns) {
        delete payload.dimensions;
        delete payload.filters;
        delete payload.columns;
        delete payload.metrics;
      }

      const url = editingDatasetId
        ? `${API_BASE}/api/v1/datasets/${editingDatasetId}`
        : `${API_BASE}/api/v1/datasets`;
      const method = editingDatasetId ? "PATCH" : "POST";

      const userEmail = account?.email || account?.username || null;
      const headers: Record<string, string> = { "Content-Type": "application/json" };
      if (userEmail) {
        headers['x-user-email'] = userEmail;
      }

      let res = await msalFetch(url, {
        method,
        headers,
        body: JSON.stringify(payload),
      });

      // Retry once on connection errors (500)
      if (!res.ok && res.status === 500) {
        console.log('Retrying dataset save due to connection error...');
        await new Promise(resolve => setTimeout(resolve, 1000)); // Wait 1 second
        res = await msalFetch(url, {
          method,
          headers,
          body: JSON.stringify(payload),
        });
      }

      const created = await res.json();
      if (!res.ok) {
        const msg = created?.detail || created?.error || `Failed to save dataset (${res.status})`;
        throw new Error(msg);
      }

      // After creation, take user to the dataset detail page where
      // they can configure dimensions/metrics for semantic datasets.
      if (created && typeof created.id === "number") {
        // `replace`, not `push`: the creation step is finished and must not stay
        // in history. With a push, Back from the saved dataset returned INTO the
        // creation flow -- "after a chart is saved, Back should go to the
        // library" -- and on this page that flow would have created a second
        // dataset. Replacing it leaves [wherever they came from, the dataset], so
        // the page's own Back button, which returns where the reader came from,
        // reaches the library it was opened from.
        void router.replace(`/datasets/${created.id}`);
      } else {
        // A string id is as valid as a number one — the engine path above
        // accepts both — and landing on the list instead of the dataset just
        // created was the only consequence of insisting on a number here.
        void router.replace(created?.id != null ? `/datasets/${String(created.id)}` : "/datasets");
      }
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Failed to create dataset";
      setSubmitError(message);
    } finally {
      setIsSubmitting(false);
    }
  };

  const hasSemanticKeyIssues = useMemo(() => {
    if (mode !== "semantic") return false;
    return dimensionMappings.some(
      (dim) =>
        dim.include && (!dim.factKey || !dim.dimKey || !dim.schema || !dim.name),
    );
  }, [mode, dimensionMappings]);

  /**
   * Included dimensions whose fact key could not be parsed out of the stored
   * join condition. There is no control on this page that can set a fact key,
   * so these cannot be completed here — only excluded.
   */
  const unkeyedDimensions = useMemo(
    () => (mode !== "semantic" ? [] : dimensionMappings
      .filter(dim => dim.include && !dim.factKey)
      .map(dim => (dim.schema && dim.name ? `${dim.schema}.${dim.name}` : dim.name || "a dimension"))),
    [mode, dimensionMappings]);

  const factKeyOptions = useMemo(() => {
    if (!factColumns) return [] as ColumnInfo[];
    const keyLike = factColumns.filter((c) => {
      const lower = c.name.toLowerCase();
      return lower.endsWith("key") || lower.endsWith("id");
    });
    return keyLike.length > 0 ? keyLike : factColumns;
  }, [factColumns]);

  const dateCandidateColumns = useMemo(() => {
    if (!factColumns) return [] as ColumnInfo[];
    return factColumns.filter((c) => {
      const type = c.dataType.toLowerCase();
      const name = c.name.toLowerCase();
      return /date|time/.test(type) || /date|time/.test(name);
    });
  }, [factColumns]);

  // Columns suitable for dataset filters - show ALL fact columns
  const filterableColumns = useMemo(() => {
    if (!factColumns) return [] as ColumnInfo[];
    // Return all fact columns without filtering
    return factColumns;
  }, [factColumns]);

  // When editing an existing dataset, seed mappings from its stored dimensions
  useEffect(() => {
    if (!editingDataset) {
      console.log('[Dimension Seed] No editing dataset');
      return;
    }
    if (mode !== "semantic") {
      console.log('[Dimension Seed] Mode not semantic:', mode);
      return;
    }
    if (!factColumns) {
      console.log('[Dimension Seed] No fact columns yet');
      return;
    }
    if (hasSeededFromDataset) {
      console.log('[Dimension Seed] Already seeded');
      return;
    }

    const dims = editingDataset.dimensions ?? [];
    console.log('[Dimension Seed] Starting seed with', dims.length, 'dimensions');

    if (dims.length === 0) {
      setHasSeededFromDataset(true);
      return;
    }

    const initialMappings: DimensionMapping[] = dims.map((dim, idx) => {
      const [schema, name] = dim.dimension_table.split(".", 2);
      const keys = parseJoinConditionKeys(dim.join_condition);
      const dimTableFull = dim.dimension_table;
      const selectedCols = Array.from(
        new Set(
          (editingDataset.columns || [])
            .filter((col) => col.table_name === dimTableFull && col.is_dimension)
            .map((col) => col.column_name),
        ),
      );

      console.log(`[Dimension Seed] Dimension ${idx + 1}:`, {
        table: dimTableFull,
        factKey: keys.factKey,
        dimKey: keys.dimKey,
        selectedCols: selectedCols.length
      });

      return {
        id: `existing-${idx}`,
        schema: schema ?? "",
        name: name ?? "",
        include: true,
        factKey: keys.factKey ?? null,
        dimKey: keys.dimKey ?? null,
        dimColumns: [],
        selectedColumns: selectedCols,
      };
    });

    console.log('[Dimension Seed] Setting', initialMappings.length, 'dimension mappings');
    setDimensionMappings(initialMappings);
    setHasSeededFromDataset(true);

    // For existing semantic datasets, eagerly load dimension columns
    // so the saved dimension keys can be shown immediately.
    initialMappings.forEach((m) => {
      if (m.schema && m.name) {
        console.log('[Dimension Seed] Loading columns for', m.schema, m.name);
        void ensureDimColumnsLoaded(m);
      }
    });
  }, [editingDataset, mode, factColumns, hasSeededFromDataset]);

  useEffect(() => {
    if (mode !== "semantic") return;
    if (factKeyOptions.length === 0) {
      setDimensionMappings([]);
      return;
    }

    setDimensionMappings((prev) => {
      const existing = [...prev];

      // For new datasets (no existing mappings), create a default
      // mapping row for each key-like fact column.
      if (existing.length === 0) {
        return factKeyOptions.map((col, idx) => {
          const guessedDim = guessDimensionTableForFactKey(col.name, dimensionTables);
          const defaultDim = guessedDim ?? dimensionTables[0];
          return {
            id: `${col.name}.${idx}`,
            schema: defaultDim?.schema ?? "",
            name: defaultDim?.name ?? "",
            include: false,
            factKey: col.name,
            dimKey: null,
            dimColumns: [],
            selectedColumns: [],
          };
        });
      }

      // Update existing mappings if they have empty schema/name (dimension tables loaded after mappings were created)
      const updated = existing.map((mapping) => {
        if (!mapping.schema && !mapping.name && mapping.factKey && dimensionTables.length > 0) {
          const guessedDim = guessDimensionTableForFactKey(mapping.factKey, dimensionTables);
          const defaultDim = guessedDim ?? dimensionTables[0];
          return {
            ...mapping,
            schema: defaultDim?.schema ?? "",
            name: defaultDim?.name ?? "",
          };
        }
        return mapping;
      });

      // Add new mappings for any key-like fact columns that don't yet have a mapping
      factKeyOptions.forEach((col) => {
        const hasMappingForFactKey = updated.some((d) => d.factKey === col.name);
        if (!hasMappingForFactKey) {
          const guessedDim = guessDimensionTableForFactKey(col.name, dimensionTables);
          const defaultDim = guessedDim ?? dimensionTables[0];
          updated.push({
            id: `${col.name}.${updated.length}`,
            schema: defaultDim?.schema ?? "",
            name: defaultDim?.name ?? "",
            include: false,
            factKey: col.name,
            dimKey: null,
            dimColumns: [],
            selectedColumns: [],
          });
        }
      });

      return updated;
    });
  }, [mode, factKeyOptions, dimensionTables]);

  // An Engine-backed dataset needs a name and a bound table and nothing else;
  // an existing one keeps the binding it has.
  const canSubmit =
    sourceKind === "engine"
      ? isAuthenticated && !!name.trim() && !isSubmitting && (!!engineBinding || !!editingDatasetId)
      : isAuthenticated &&
        !!name.trim() &&
        !!selectedTable &&
        !!currentDatabase &&
        !isSubmitting &&
        (mode !== "semantic" || !hasSemanticKeyIssues);

  if (isAuthenticated && !canCreate) {
    return (
      <div className="page-shell page-shell-wide">
        <div className="card page-empty-card">
          <p className="page-empty-title">
            {editingDatasetId ? "You cannot change a dataset" : "You cannot create a dataset"}
          </p>
          <p className="page-empty-body">{whyNoCreate("dataset")}</p>
          <button
            type="button"
            onClick={() => router.push("/workspace?tab=datasets")}
            style={{
              marginTop: 16, padding: "9px 18px", fontSize: 13, fontWeight: 500,
              background: "var(--accent)", color: "#fff", border: "none",
              borderRadius: 10, cursor: "pointer",
            }}
          >
            Browse datasets
          </button>
        </div>
      </div>
    );
  }

  // What this installation can actually read, and therefore which path the
  // form offers. `engineCatalogs === null` means the answer is still in
  // flight; an empty array means there are none.
  const catalogCount = engineCatalogs?.length ?? 0;
  const availabilityKnown = engineCatalogs !== null && dataSourcesLoaded;
  const bothRegistered = catalogCount > 0 && dataSources.length > 0;
  const nothingRegistered = availabilityKnown && catalogCount === 0 && dataSources.length === 0;
  const engineQualifiedName = engineBinding
    ? `${engineBinding.catalog.name}.${engineBinding.schema.name}.${engineBinding.table.name}`
    : editingDataset && sourceKind === "engine"
    ? [editingDataset.database_name, editingDataset.schema_name, editingDataset.table_name]
        .filter(Boolean)
        .join(".")
    : null;

  return (
    <div className="page-shell page-shell-wide">
      <header className="page-header">
        <h1 className="page-header-title">
          {editingDatasetId ? "Edit dataset" : "New dataset"}
        </h1>
        <p className="page-header-subtitle">
          {sourceKind === "engine"
            ? "Bind a dataset to one table of a registered catalog. Its columns, metrics and date column are read from the Engine's definition."
            : sourceKind === "database"
            ? "Start from a table of a registered database connection as a flat dataset, or build a semantic model with dimensions."
            : "A dataset reads one table — either of a registered catalog, or of a database connection."}
        </p>
      </header>

      {!isAuthenticated && (
        <p className="muted">Sign in to create datasets.</p>
      )}

      {isAuthenticated && !sourceKind && (editingDatasetId ? !editingDataset : !availabilityKnown) && (
        <div className="card page-empty-card">
          <p className="page-empty-title">
            {editingDatasetId ? "Reading the dataset" : "Reading what is registered"}
          </p>
          <p className="page-empty-body">
            {editingDatasetId
              ? "Its definition and the source it reads."
              : "The catalogs and database connections this installation can read."}
          </p>
        </div>
      )}

      {/* A failed catalog read used to be stored as "no catalogs", which drew
          this card — "This installation has neither", with instructions to go
          and register a catalog source. Both sentences were made out of a
          request that did not answer, and the second sends an administrator to
          configure something that may already be correct. The verdict is kept
          for the case the server actually reported: nothing registered. */}
      {isAuthenticated && !editingDatasetId && nothingRegistered && (
        <div className="card page-empty-card">
          <p className="page-empty-title">
            {engineCatalogError
              ? "What a dataset may read is not known"
              : "Nothing is registered for a dataset to read"}
          </p>
          <p className="page-empty-body">
            {engineCatalogError ? (
              <>
                {unknownText({
                  what: "The registered catalogs",
                  says: "which catalogs a dataset may read",
                  detail: engineCatalogError,
                })}
                {" "}No database connection is registered either, so this page has nothing it can
                offer yet — but it is not saying your catalogs are gone.
              </>
            ) : (
              <>
                A dataset reads one table: either a table of a catalog registered against the
                Engine, or a table of an external database connection. This installation has
                neither. Register a catalog source under Settings &rarr; Storage, or a database
                connection under Settings &rarr; Data sources; both are administrator settings.
                {engineUnsynchronized.length > 0 && (
                  <>
                    {" "}
                    {engineUnsynchronized.length === 1
                      ? `The catalog source ${engineUnsynchronized[0]} is registered, but the Engine returned no definition for it`
                      : `${engineUnsynchronized.length} catalog sources are registered, but the Engine returned no definition for them`}
                    : either they have not been synchronized, or your account has not been granted
                    them.
                  </>
                )}
              </>
            )}
          </p>
        </div>
      )}

      {/* The same failure, where one database connection does exist: the form
          resolved straight to that path and the catalog read's failure was
          never mentioned anywhere, so a reader whose catalogs were simply
          unreachable was shown a page with no catalogs in it and no reason. */}
      {isAuthenticated && !editingDatasetId && engineCatalogError && !nothingRegistered && (
        <p className="muted" style={{ marginTop: 12, fontSize: 13 }} role="status">
          {unknownText({
            what: "The registered catalogs",
            says: "which catalogs a dataset may read",
            detail: engineCatalogError,
          })}
        </p>
      )}

      {isAuthenticated && !sourceKind && bothRegistered && (
        <div className={es.chooser}>
          <button type="button" className={es.choice} onClick={() => setSourceKind("engine")}>
            <span className={es.choiceTitle}>A table of a registered catalog</span>
            <p className={es.choiceBody}>
              The Engine reads the lake in place. The dataset binds to one table by its catalog id
              and takes its columns and metrics from the Engine&rsquo;s definition.
            </p>
            <p className={es.choiceCount}>
              {catalogCount === 1 ? "1 catalog" : `${catalogCount} catalogs`}
            </p>
          </button>
          <button type="button" className={es.choice} onClick={() => setSourceKind("database")}>
            <span className={es.choiceTitle}>A table of a database connection</span>
            <p className={es.choiceBody}>
              An external database — Fabric SQL, Azure SQL, PostgreSQL or MySQL. Flat, or a
              semantic model with dimension joins.
            </p>
            <p className={es.choiceCount}>
              {dataSources.length === 1 ? "1 connection" : `${dataSources.length} connections`}
            </p>
          </button>
        </div>
      )}

      {isAuthenticated && sourceKind && bothRegistered && !editingDatasetId && (
        <p className={es.switcher}>
          {sourceKind === "engine"
            ? "Reading a table of a registered catalog."
            : "Reading a table of a database connection."}
          <button
            type="button"
            className={es.switcherAction}
            onClick={() => setSourceKind(sourceKind === "engine" ? "database" : "engine")}
          >
            {sourceKind === "engine"
              ? "Use a database connection instead"
              : "Use a registered catalog instead"}
          </button>
        </p>
      )}

      {loadError && sourceKind !== "engine" && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading database metadata</p>
          <p className="page-empty-body">{loadError}</p>
        </div>
      )}

      {isAuthenticated && !!sourceKind && (sourceKind === "engine" || !loadError) && (
        // No `overflow: hidden` here. An overflow-hidden ancestor becomes the
        // sticky header's scrollport, and this box never scrolls — so the
        // header stopped sticking and scrolled away with the page, taking
        // Cancel and Create with it. A semantic dataset's form runs for
        // thousands of pixels and has no second submit at its foot.
        <form onSubmit={handleSubmit} className="card" style={{ padding: 0 }}>
          {/* Sticky Header with Action Buttons */}
          <div style={{
            position: "sticky",
            top: 0,
            zIndex: 10,
            background: "var(--bg-surface)",
            borderBottom: "1px solid var(--border)",
            padding: "12px 24px",
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
          }}>
            <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
              <h2 style={{
                fontSize: "15px",
                fontWeight: 600,
                color: "var(--text-primary)",
                margin: 0,
              }}>
                {editingDatasetId ? "Edit dataset" : "New dataset"}
              </h2>
              {name && (
                <span style={{
                  fontSize: "14px",
                  color: "var(--text-muted)",
                  fontWeight: 400,
                }}>
                  · {name}
                </span>
              )}
            </div>

            <div style={{ display: "flex", gap: 12 }}>
              <button
                type="button"
                onClick={() => router.back()}
                disabled={isSubmitting}
                style={{
                  padding: "8px 20px",
                  fontSize: "14px",
                  fontWeight: 500,
                  borderRadius: "6px",
                  border: "1px solid var(--border)",
                  background: "var(--bg-surface)",
                  color: "var(--text-secondary)",
                  cursor: isSubmitting ? "not-allowed" : "pointer",
                  opacity: isSubmitting ? 0.5 : 1,
                  transition: "all 0.2s",
                }}
                onMouseEnter={(e) => {
                  if (!isSubmitting) {
                    e.currentTarget.style.background = "var(--bg-primary)";
                    e.currentTarget.style.borderColor = "var(--text-muted)";
                  }
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.background = "var(--bg-surface)";
                  e.currentTarget.style.borderColor = "var(--border)";
                }}
              >
                Cancel
              </button>
              <button
                type="submit"
                disabled={!canSubmit}
                style={{
                  padding: "8px 20px",
                  fontSize: "14px",
                  fontWeight: 600,
                  borderRadius: "6px",
                  border: "none",
                  background: canSubmit
                    ? `linear-gradient(135deg, ${primaryColor} 0%, #0284c7 100%)`
                    : "var(--border)",
                  color: canSubmit ? "#ffffff" : "var(--text-muted)",
                  cursor: canSubmit ? "pointer" : "not-allowed",
                  transition: "all 0.2s",
                  boxShadow: canSubmit ? "0 2px 4px rgba(0, 0, 0, 0.1)" : "none",
                }}
                onMouseEnter={(e) => {
                  if (canSubmit) {
                    e.currentTarget.style.transform = "translateY(-1px)";
                    e.currentTarget.style.boxShadow = "0 4px 8px rgba(0, 0, 0, 0.15)";
                  }
                }}
                onMouseLeave={(e) => {
                  if (canSubmit) {
                    e.currentTarget.style.transform = "translateY(0)";
                    e.currentTarget.style.boxShadow = "0 2px 4px rgba(0, 0, 0, 0.1)";
                  }
                }}
              >
                {isSubmitting
                  ? editingDatasetId
                    ? "Updating…"
                    : "Creating…"
                  : editingDatasetId
                  ? "Update dataset"
                  : "Create dataset"}
              </button>
            </div>
          </div>

          {/* Form Content */}
          <div style={{ display: "grid", gridTemplateColumns: "280px 1fr", gap: 24, minHeight: 360, padding: 24 }}>
          <div style={{ borderRight: "1px solid var(--border)", paddingRight: 16 }}>
            {sourceKind === "engine" ? (
              editingDatasetId ? (
                <div className={es.field}>
                  <span className={es.label}>Source</span>
                  <p className={es.hint}>
                    This dataset is bound to one table of the Engine&rsquo;s catalog, shown beside.
                    Create a new dataset to read a different table.
                  </p>
                </div>
              ) : (
                <EngineTablePicker
                  catalogs={engineCatalogs ?? NO_CATALOGS}
                  unsynchronized={engineUnsynchronized}
                  onChange={handleEngineBinding}
                />
              )
            ) : (
              <>
            <div style={{ marginBottom: 16 }}>
              <DataSourceSelector
                value={currentDataSourceId}
                onChange={async (id) => {
                  if (id === null) return;
                  setCurrentDataSourceId(id);
                  // Skip database switching when editing - the edit useEffect handles it
                  if (editingDataset) {
                    console.log('[Datasets] Skipping switchDatabase in onChange - editing mode');
                    return;
                  }
                  // Look up the data source to get its database_name
                  const selectedDs = dataSources.find(ds => ds.id === id);
                  if (selectedDs?.database_name) {
                    await switchDatabase(selectedDs.database_name);
                  }
                }}
                label="Data Source"
                required={false}
                className="save-query-field-container"
              />
            </div>

            <div style={{ marginBottom: 16 }}>
              <label className="save-query-field-label" htmlFor="dataset-schema">
                Schema
              </label>
              <div style={{ display: "flex", gap: 4 }}>
                <select
                  id="dataset-schema"
                  className="save-query-input chart-builder-select"
                  value={selectedSchema ?? ""}
                  onChange={(e) => {
                    const value = e.target.value || null;
                    setSelectedSchema(value);
                    const firstForSchema =
                      filteredTables.find((t) => t.schema === value) ?? null;
                    setSelectedTableId(firstForSchema ? firstForSchema.id : null);
                  }}
                  disabled={isLoadingTables || schemaOptions.length === 0}
                >
                  <option value="" disabled>
                    {isLoadingTables ? "Loading schemas…" : "Select schema"}
                  </option>
                  {schemaOptions.map((schema) => (
                    <option key={schema} value={schema}>
                      {schema}
                    </option>
                  ))}
                </select>
                <button
                  type="button"
                  className="icon-button"
                  title="Re-read the schemas and tables of this connection"
                  aria-label="Re-read the schemas and tables of this connection"
                  onClick={() => void refreshTables()}
                  disabled={isLoadingTables}
                >
                  <i className="fas fa-sync-alt" aria-hidden="true" />
                </button>
              </div>
            </div>

            <div>
              <label className="save-query-field-label" htmlFor="dataset-table">
                Table
              </label>
              <div style={{ display: "flex", gap: 4 }}>
                <select
                  id="dataset-table"
                  className="save-query-input chart-builder-select"
                  value={selectedTableId ?? ""}
                  onChange={(e) => {
                    const value = e.target.value || null;
                    setSelectedTableId(value);
                  }}
                  disabled={isLoadingTables || filteredTables.length === 0}
                >
                  <option value="" disabled>
                    {isLoadingTables ? "Loading tables…" : "Select table"}
                  </option>
                  {filteredTables.map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.schema}.{t.name}
                    </option>
                  ))}
                </select>
                <button
                  type="button"
                  className="icon-button"
                  title="Refresh tables"
                  aria-label="Refresh tables"
                  onClick={() => void refreshTables()}
                  disabled={isLoadingTables}
                >
                  <i className="fas fa-sync-alt" aria-hidden="true" />
                </button>
              </div>
            </div>
            {refreshError && (
              <p style={{ marginTop: 8, fontSize: 12, color: "var(--danger)" }} role="status">
                {refreshError}
              </p>
            )}
              </>
            )}
          </div>

          <div style={{ display: "flex", flexDirection: "column", justifyContent: "space-between" }}>
            <div>
              <div style={{ marginBottom: 16 }}>
                <label className="save-query-field-label" htmlFor="dataset-name">
                  Dataset name
                </label>
                <input
                  id="dataset-name"
                  className="save-query-input"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="Give your dataset a friendly name"
                  required
                />
              </div>

              <div style={{ marginBottom: 16 }}>
                <label className="save-query-field-label" htmlFor="dataset-description">
                  Description
                </label>
                <textarea
                  id="dataset-description"
                  className="save-query-textarea"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder="Optional description to help others understand this dataset"
                />
              </div>

              {sourceKind === "engine" && (
                <EngineBindingPanel
                  qualifiedName={engineQualifiedName}
                  table={engineBinding?.table ?? boundEngineTable}
                  measurement={engineBinding?.measurement ?? null}
                  bound={!!editingDatasetId}
                  dateColumn={dateColumn}
                  onDateColumn={setDateColumn}
                />
              )}

              {/* Filters, metrics and dimension joins are built on the Lab's
                  metadata endpoints, which answer for a database connection.
                  An Engine-backed dataset takes its columns and metrics from
                  the Engine's definition and is refined on the dataset's own
                  page, so none of this is offered on that path. */}
              {sourceKind !== "engine" && (
              <>
              <div style={{ marginBottom: 16 }}>
                <label className="save-query-field-label" htmlFor="dataset-date-column">
                  Date column (optional)
                </label>
                <p className="muted" style={{ marginTop: 4, fontSize: 12 }}>
                  Choose the primary date/time column for this dataset. Queries can also use
                  helpers like <code>Date__year</code>, <code>Date__month</code>,
                  {" "}
                  <code>Date__day</code>, and <code>Date__hour</code> when your table exposes
                  those partition columns.
                </p>

                {!selectedTable && (
                  <p className="muted" style={{ marginTop: 8, fontSize: 12 }}>
                    Select a table on the left to choose a date column.
                  </p>
                )}

                {selectedTable && (
                  <div style={{ marginTop: 8 }}>
                    <select
                      id="dataset-date-column"
                      className="save-query-input chart-builder-select"
                      value={dateColumn}
                      onChange={(e) => setDateColumn(e.target.value)}
                      disabled={!factColumns || factColumns.length === 0}
                    >
                      <option value="">
                        {factColumns && factColumns.length > 0
                          ? "No specific date column"
                          : "No columns available"}
                      </option>
                      {(dateCandidateColumns.length > 0
                        ? dateCandidateColumns
                        : factColumns || []
                      ).map((c) => (
                        <option key={c.name} value={c.name}>
                          {c.name}
                        </option>
                      ))}
                    </select>
                  </div>
                )}
              </div>

              <div style={{ marginBottom: 16 }}>
                <span className="save-query-field-label">Dataset filters</span>
                <p className="muted" style={{ marginTop: 4, fontSize: 12 }}>
                  Optionally restrict this dataset to specific rows by applying filters on the
                  source table. Choose a column and a value; use the + button to add more
                  filters.
                </p>

                {!selectedTable && (
                  <p className="muted" style={{ marginTop: 8, fontSize: 12 }}>
                    Select a table on the left to configure filters.
                  </p>
                )}

                {selectedTable && (
                  <div style={{ display: "grid", gap: 6, marginTop: 8 }}>
                    {filters.map((f) => (
                      <div
                        key={f.id}
                        className="card"
                        style={{
                          padding: "6px 8px",
                          display: "grid",
                          gridTemplateColumns: "minmax(0, 2fr) minmax(0, 2fr) auto",
                          gap: 8,
                          alignItems: "center",
                        }}
                      >
                        <div>
                          <select
                            className="save-query-input chart-builder-select"
                            value={f.column}
                            onChange={async (e) => {
                              const column = e.target.value;
                              setFilters((prev) =>
                                prev.map((row) =>
                                  row.id === f.id
                                    ? {
                                        ...row,
                                        column,
                                        value: "",
                                        options: [],
                                        isLoading: !!column,
                                      }
                                    : row,
                                ),
                              );

                              if (!column || !selectedTable || !currentDatabase) {
                                console.log(`[Dataset Filters] Cannot fetch values yet - missing:`, {
                                  column: !!column,
                                  selectedTable: !!selectedTable,
                                  currentDatabase: !!currentDatabase
                                });
                                return;
                              }

                              try {
                                const schemaEncoded = encodeURIComponent(selectedTable.schema);
                                const tableEncoded = encodeURIComponent(selectedTable.name);
                                const colEncoded = encodeURIComponent(column);

                                // Pass database parameter to query the correct database
                                const databaseParam = `?database=${encodeURIComponent(currentDatabase)}`;
                                console.log(`[Dataset Filters] Fetching distinct values for ${selectedTable.schema}.${selectedTable.name}.${column} from database: ${currentDatabase}`);

                                const res = await msalFetch(
                                  `${API_BASE}/api/v1/lab/distinct/${schemaEncoded}/${tableEncoded}/${colEncoded}${databaseParam}`,
                                );
                                const data = await res.json();
                                if (!res.ok || !data.success) {
                                  console.error(`[Dataset Filters] Failed to fetch distinct values:`, data.error || 'Unknown error');
                                  throw new Error(data.error || "Failed to load values");
                                }
                                console.log(`[Dataset Filters] Loaded ${data.values?.length || 0} distinct values for ${column}`);
                                const values: string[] = Array.isArray(data.values)
                                  ? data.values
                                      .map((v: any) => {
                                        if (v == null) return "";
                                        if (
                                          typeof v === "string" ||
                                          typeof v === "number" ||
                                          typeof v === "boolean"
                                        ) {
                                          return String(v);
                                        }
                                        if (typeof v === "object" && "value" in v && v.value != null) {
                                          return String((v as any).value);
                                        }
                                        return "";
                                      })
                                      .filter((s: string) => s !== "")
                                  : [];
                                setFilters((prev) =>
                                  prev.map((row) =>
                                    row.id === f.id
                                      ? {
                                          ...row,
                                          options: values,
                                          isLoading: false,
                                        }
                                      : row,
                                  ),
                                );
                              } catch {
                                setFilters((prev) =>
                                  prev.map((row) =>
                                    row.id === f.id
                                      ? { ...row, options: [], isLoading: false }
                                      : row,
                                  ),
                                );
                              }
                            }}
                            disabled={!filterableColumns || filterableColumns.length === 0}
                          >
                            <option value="">
                              {filterableColumns && filterableColumns.length > 0
                                ? "Select column"
                                : "No columns available"}
                            </option>
                            {(filterableColumns || []).map((c) => (
                              <option key={c.name} value={c.name}>
                                {c.name}
                              </option>
                            ))}
                          </select>
                        </div>

                        <div>
                          <select
                            className="save-query-input chart-builder-select"
                            value={f.value}
                            onChange={(e) => {
                              const value = e.target.value;
                              setFilters((prev) =>
                                prev.map((row) =>
                                  row.id === f.id
                                    ? { ...row, value }
                                    : row,
                                ),
                              );
                            }}
                            disabled={f.isLoading || !f.column || f.options.length === 0}
                          >
                            <option value="">
                              {f.isLoading
                                ? "Loading values…"
                                : f.column
                                ? f.options.length > 0
                                  ? "Select value"
                                  : "No values found"
                                : "Select column first"}
                            </option>
                            {f.options.map((v) => (
                              <option key={v} value={v}>
                                {v}
                              </option>
                            ))}
                          </select>
                        </div>

                        <div style={{ display: "flex", gap: 4 }}>
                          <button
                            type="button"
                            className="icon-button"
                            title="Remove filter"
                            onClick={() =>
                              setFilters((prev) => prev.filter((row) => row.id !== f.id))
                            }
                          >
                            <i className="fas fa-trash" aria-hidden="true" />
                          </button>
                        </div>
                      </div>
                    ))}

                    <button
                      type="button"
                      className="save-query-btn-secondary"
                      onClick={() =>
                        setFilters((prev) => [
                          ...prev,
                          {
                            id: `filter-${Date.now()}-${prev.length}`,
                            column: "",
                            value: "",
                            options: [],
                            isLoading: false,
                          },
                        ])
                      }
                    >
                      <i className="fas fa-plus" aria-hidden="true" /> Add filter
                    </button>
                  </div>
                )}
              </div>

              <div style={{ marginBottom: 16 }}>
                <span className="save-query-field-label">Metrics (optional)</span>
                <p className="muted" style={{ marginTop: 4, fontSize: 12 }}>
                  Define reusable measures for this dataset, such as Sum of SalesAmount. Metrics
                  are based on fact table columns with basic aggregation types.
                </p>

                {!selectedTable && (
                  <p className="muted" style={{ marginTop: 8, fontSize: 12 }}>
                    Select a table on the left to configure metrics.
                  </p>
                )}

                {selectedTable && (
                  <div style={{ display: "grid", gap: 8, marginTop: 8 }}>
                    {metrics.map((m) => (
                      <div
                        key={m.id}
                        className="card"
                        style={{
                          padding: 8,
                          display: "grid",
                          gridTemplateColumns: "minmax(0, 2.5fr) minmax(0, 2fr) minmax(0, 1.5fr) auto",
                          gap: 8,
                          alignItems: "center",
                        }}
                      >
                        <div>
                          <span className="muted" style={{ fontSize: 11 }}>
                            Metric name
                          </span>
                          <input
                            className="save-query-input"
                            value={m.name}
                            onChange={(e) => {
                              const value = e.target.value;
                              setMetrics((prev) =>
                                prev.map((row) =>
                                  row.id === m.id
                                    ? { ...row, name: value }
                                    : row,
                                ),
                              );
                            }}
                            placeholder="e.g. Sum of SalesAmount"
                          />
                        </div>

                        <div>
                          <span className="muted" style={{ fontSize: 11 }}>
                            Column
                          </span>
                          <select
                            className="save-query-input chart-builder-select"
                            value={m.column}
                            onChange={(e) => {
                              const column = e.target.value;
                              setMetrics((prev) =>
                                prev.map((row) =>
                                  row.id === m.id
                                    ? { ...row, column }
                                    : row,
                                ),
                              );
                            }}
                            disabled={!filterableColumns || filterableColumns.length === 0}
                          >
                            <option value="">
                              {filterableColumns && filterableColumns.length > 0
                                ? "Select column"
                                : "No columns available"}
                            </option>
                            {(filterableColumns || []).map((c) => (
                              <option key={c.name} value={c.name}>
                                {c.name}
                              </option>
                            ))}
                          </select>
                        </div>

                        <div>
                          <span className="muted" style={{ fontSize: 11 }}>
                            Aggregation
                          </span>
                          <select
                            className="save-query-input chart-builder-select"
                            value={m.agg}
                            onChange={(e) => {
                              const agg = e.target.value;
                              setMetrics((prev) =>
                                prev.map((row) =>
                                  row.id === m.id
                                    ? { ...row, agg }
                                    : row,
                                ),
                              );
                            }}
                          >
                            <option value="SUM">SUM</option>
                            <option value="AVG">AVG</option>
                            <option value="COUNT">COUNT</option>
                            <option value="MIN">MIN</option>
                            <option value="MAX">MAX</option>
                          </select>
                        </div>

                        <div style={{ display: "flex", gap: 4 }}>
                          <button
                            type="button"
                            className="icon-button"
                            title="Remove metric"
                            onClick={() =>
                              setMetrics((prev) => prev.filter((row) => row.id !== m.id))
                            }
                          >
                            <i className="fas fa-trash" aria-hidden="true" />
                          </button>
                        </div>
                      </div>
                    ))}

                    <button
                      type="button"
                      className="save-query-btn-secondary"
                      onClick={() =>
                        setMetrics((prev) => [
                          ...prev,
                          {
                            id: `metric-${Date.now()}-${prev.length}`,
                            name: "",
                            column: "",
                            agg: "SUM",
                          },
                        ])
                      }
                    >
                      <i className="fas fa-plus" aria-hidden="true" /> Add metric
                    </button>
                  </div>
                )}
              </div>

              <div style={{ marginBottom: 16 }}>
                <span className="save-query-field-label">Dataset type</span>
                {/* Both halves of this toggle were drawn in a pre-rename
                    token declared once as #111827 with no dark value anywhere,
                    so on the dark theme the unselected half was near-black on
                    near-black: the only control that switches dataset type
                    could not be read. */}
                <div style={{ display: "inline-flex", borderRadius: 999, border: "1px solid var(--border)", padding: 2, marginTop: 4 }}>
                  <button
                    type="button"
                    onClick={() => setMode("flat")}
                    style={{
                      border: "none",
                      backgroundColor: mode === "flat" ? primaryColor : "transparent",
                      color: mode === "flat" ? "#ffffff" : "var(--text-secondary)",
                      borderRadius: 999,
                      padding: "4px 10px",
                      fontSize: 13,
                      cursor: "pointer",
                    }}
                  >
                    Flat table
                  </button>
                  <button
                    type="button"
                    onClick={() => setMode("semantic")}
                    style={{
                      border: "none",
                      backgroundColor: mode === "semantic" ? primaryColor : "transparent",
                      color: mode === "semantic" ? "#ffffff" : "var(--text-secondary)",
                      borderRadius: 999,
                      padding: "4px 10px",
                      fontSize: 13,
                      cursor: "pointer",
                    }}
                  >
                    Semantic (with dimensions)
                  </button>
                </div>
                <p className="muted" style={{ marginTop: 6, fontSize: 12 }}>
                  Flat datasets query the selected table directly. Semantic datasets can layer on
                  joins to dimension tables later so charts and Lab queries can use friendly fields.
                </p>
              </div>

              {/* A failed column read is reported in both modes. Rendered
                  inside the semantic block, a flat dataset showed only "No
                  columns available" in three selects — a verdict about the
                  table made out of a request that did not answer — while the
                  reason sat in a branch that was not on screen. */}
              {factColumnsError && (
                <div style={{
                  marginTop: 12, padding: 12, borderRadius: 8,
                  // `--warning-bg` is defined nowhere in the repo, so this card
                  // always fell back to #fff3cd with #856404 ink: a cream
                  // rectangle on the dark theme, and two literals for a state
                  // the danger token already carries.
                  background: "color-mix(in srgb, var(--danger) 8%, transparent)",
                  border: "1px solid color-mix(in srgb, var(--danger) 35%, transparent)",
                }}>
                  <p style={{ margin: 0, fontSize: 13, color: "var(--danger)" }}>
                    <strong>The table&rsquo;s columns could not be read:</strong> {factColumnsError}
                  </p>
                  <p style={{ margin: '8px 0 0 0', fontSize: 12, color: "var(--text-secondary)" }}>
                    Nothing about the dataset has changed. The columns, dimension joins and metrics
                    it already holds are left as they are until they can be read again.
                  </p>
                </div>
              )}

              {mode === "semantic" && (
                <div style={{ marginTop: 24 }}>
                  <div className="muted" style={{ marginBottom: 16, fontSize: 13 }}>
                    {selectedTable ? (
                      <>
                        Selected source: <strong>{selectedTable.schema}.{selectedTable.name}</strong>
                        {currentDatabase ? ` in ${currentDatabase}` : null}
                      </>
                    ) : (
                      <>Select a table to define your dataset source.</>
                    )}
                  </div>

                  <div style={{ display: "flex", alignItems: "center", gap: 16, marginBottom: 8 }}>
                    <span className="save-query-field-label" style={{ margin: 0 }}>Dimension relationships</span>
                    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                      <label className="muted" style={{ fontSize: 12, margin: 0, whiteSpace: "nowrap" }}>
                        Schema for dimension tables
                      </label>
                      <select
                        value={dimensionSchema || ""}
                        onChange={(e) => {
                          const newSchema = e.target.value || null;
                          setDimensionSchema(newSchema);
                          // Clear existing mappings when schema changes
                          setDimensionMappings([]);
                          if (newSchema) {
                            void loadDimensionCandidates(newSchema);
                          }
                        }}
                        className="dataset-input"
                        style={{ width: "auto", minWidth: 150 }}
                      >
                        <option value="">Select schema...</option>
                        {schemaOptions.map((s) => (
                          <option key={s} value={s}>
                            {s}
                          </option>
                        ))}
                      </select>
                    </div>
                  </div>

                  <p className="muted" style={{ marginTop: 8, fontSize: 12 }}>
                    {dimensionSchema ? (
                      <>
                        Choose which dimension tables from <strong>{dimensionSchema}</strong> to include and map the fact key and dimension key used for joins.
                      </>
                    ) : (
                      <>Select a schema above to find dimension tables.</>
                    )}
                  </p>

                  {dimensionSchema && isLoadingDimensions && (
                    <p className="muted" style={{ marginTop: 8, fontSize: 13 }}>
                      Loading dimension tables…
                    </p>
                  )}

                  {dimensionSchema && !isLoadingDimensions && !isLoadingFactColumns && !factColumnsError && dimensionMappings.length === 0 && (
                    <p className="muted" style={{ marginTop: 8, fontSize: 13 }}>
                      {!selectedTable ? (
                        <>Please select a table first.</>
                      ) : !factColumns || factColumns.length === 0 ? (
                        <>No columns found in the selected table.</>
                      ) : (
                        <>No key columns found in {selectedTable.schema}.{selectedTable.name}. The table should have columns ending in &quot;Key&quot; or &quot;ID&quot; to create dimension joins.</>
                      )}
                    </p>
                  )}

                  {isLoadingFactColumns && (
                    <p className="muted" style={{ marginTop: 8, fontSize: 13 }}>
                      Loading fact table columns…
                    </p>
                  )}

                  {!isLoadingDimensions && dimensionMappings.length > 0 && (
                    <div style={{ marginTop: 8, display: "grid", gap: 8 }}>
                      {dimensionMappings.map((dim) => (
                        <div
                          key={dim.id}
                          className="card"
                          style={{
                            padding: 8,
                            display: "grid",
                            gridTemplateColumns:
                              "minmax(0, 1.5fr) minmax(0, 2fr) minmax(0, 1.5fr) minmax(0, 2fr)",
                            gap: 8,
                            alignItems: "center",
                          }}
                        >
                          <div>
                            <span className="muted" style={{ fontSize: 11 }}>Fact key</span>
                            <div
                              style={{
                                marginTop: 2,
                                fontSize: 13,
                                fontWeight: dim.factKey ? 600 : 400,
                                color: dim.factKey ? "var(--text-primary)" : "var(--text-muted)",
                              }}
                            >
                              {isLoadingFactColumns
                                ? "Loading fact columns…"
                                : dim.factKey || "No key available"}
                            </div>
                            {dim.factKey && (
                              <button
                                type="button"
                                className="link-button"
                                style={{ fontSize: 11, marginTop: 4 }}
                                onClick={() => {
                                  setDimensionMappings((prev) => {
                                    const source = prev.find((d) => d.id === dim.id);
                                    if (!source) return prev;
                                    const cloneId = `${source.factKey || "dim"}.${Date.now()}`;
                                    const cloned: DimensionMapping = {
                                      ...source,
                                      id: cloneId,
                                      include: false,
                                      // Start fresh so the user can pick a different table/key/value
                                      dimKey: null,
                                      dimColumns: [],
                                      selectedColumns: [],
                                    };
                                    return [...prev, cloned];
                                  });
                                }}
                              >
                                + Join key
                              </button>
                            )}
                          </div>

                          <div>
                            <span className="muted" style={{ fontSize: 11 }}>Dimension table</span>
                            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                              <input
                                type="checkbox"
                                checked={dim.include}
                                onChange={async (e) => {
                                  const include = e.target.checked;
                                  setDimensionMappings((prev) =>
                                    prev.map((d) =>
                                      d.id === dim.id
                                        ? {
                                            ...d,
                                            include,
                                          }
                                        : d,
                                    ),
                                  );
                                  if (include && dim.schema && dim.name) {
                                    await ensureDimColumnsLoaded(dim);
                                  }
                                }}
                              />
                              <select
                                className="save-query-input chart-builder-select"
                                value={dim.schema && dim.name ? `${dim.schema}.${dim.name}` : ""}
                                onChange={async (e) => {
                                  const value = e.target.value;
                                  const [schema, name] = value ? value.split(".", 2) : ["", ""];

                                  setDimensionMappings((prev) =>
                                    prev.map((d) =>
                                      d.id === dim.id
                                        ? {
                                            ...d,
                                            schema,
                                            name,
                                            dimColumns: [],
                                            dimKey: null,
                                            selectedColumns: [],
                                          }
                                        : d,
                                    ),
                                  );

                                  if (dim.include && schema && name) {
                                    await ensureDimColumnsLoaded({
                                      ...dim,
                                      schema,
                                      name,
                                      dimColumns: [],
                                    });
                                  }
                                }}
                                disabled={!dim.include || dimensionTables.length === 0}
                              >
                                <option value="">
                                  {dimensionTables.length === 0
                                    ? "No dimension tables"
                                    : "Select dimension table"}
                                </option>
                                {dimensionTables
                                  .filter((t) => !dimensionSchema || t.schema === dimensionSchema)
                                  .map((t) => (
                                    <option key={t.id} value={`${t.schema}.${t.name}`}>
                                      {t.schema}.{t.name}
                                    </option>
                                  ))}
                              </select>
                            </div>
                          </div>

                          <div>
                            <span className="muted" style={{ fontSize: 11 }}>Dimension key</span>
                            <select
                              className="save-query-input chart-builder-select"
                              value={dim.dimKey ?? ""}
                              onChange={(e) => {
                                const value = e.target.value || null;
                                setDimensionMappings((prev) =>
                                  prev.map((d) =>
                                    d.id === dim.id
                                      ? {
                                          ...d,
                                          dimKey: value,
                                        }
                                      : d,
                                  ),
                                );
                              }}
                              disabled={!dim.include || dim.dimColumns.length === 0}
                            >
                              <option value="">Select dimension key</option>
                              {dim.dimColumns.map((c) => (
                                <option key={c.name} value={c.name}>
                                  {c.name}
                                </option>
                              ))}
                            </select>
                          </div>

                          {dim.include && dim.dimColumns.length > 0 && (
                            <div style={{ position: "relative" }}>
                              <span className="muted" style={{ fontSize: 11 }}>Dimension value</span>
                              <button
                                type="button"
                                className="save-query-input chart-builder-select"
                                style={{
                                  marginTop: 4,
                                  textAlign: "left",
                                  paddingRight: 28,
                                  display: "flex",
                                  alignItems: "center",
                                  justifyContent: "space-between",
                                  backgroundColor: "var(--bg-surface)",
                                }}
                                onClick={() =>
                                  setOpenDimValueId((prev) => (prev === dim.id ? null : dim.id))
                                }
                              >
                                <span style={{ fontSize: 13 }}>
                                  {dim.selectedColumns && dim.selectedColumns.length > 0
                                    ? `${dim.selectedColumns.length} selected`
                                    : "Select values"}
                                </span>
                              </button>
                              {openDimValueId === dim.id && (
                                <div
                                  className="card"
                                  style={{
                                    position: "absolute",
                                    zIndex: 20,
                                    marginTop: 4,
                                    right: 0,
                                    left: 0,
                                    maxHeight: 220,
                                    overflowY: "auto",
                                    padding: 8,
                                    backgroundColor: "var(--bg-surface)",
                                    boxShadow: "var(--shadow-md)",
                                  }}
                                >
                                  {dim.dimColumns.map((col) => (
                                    <label
                                      key={col.name}
                                      style={{
                                        display: "flex",
                                        alignItems: "center",
                                        gap: 6,
                                        fontSize: 13,
                                        padding: "2px 0",
                                      }}
                                    >
                                      <input
                                        type="checkbox"
                                        checked={
                                          (dim.selectedColumns || []).includes(col.name)
                                        }
                                        onChange={(e) => {
                                          const checked = e.target.checked;
                                          setDimensionMappings((prev) =>
                                            prev.map((d) => {
                                              if (d.id !== dim.id) return d;
                                              const set = new Set(d.selectedColumns || []);
                                              if (checked) {
                                                set.add(col.name);
                                              } else {
                                                set.delete(col.name);
                                              }
                                              return {
                                                ...d,
                                                selectedColumns: Array.from(set),
                                              };
                                            }),
                                          );
                                        }}
                                      />
                                      <span>{col.name}</span>
                                    </label>
                                  ))}
                                  {(!dim.selectedColumns || dim.selectedColumns.length === 0) && (
                                    <p className="muted" style={{ fontSize: 11, marginTop: 4 }}>
                                      Select one or more columns.
                                    </p>
                                  )}
                                </div>
                              )}
                            </div>
                          )}

                        </div>
                      ))}
                    </div>
                  )}

                  {/* Two different blockers, told apart. A missing dimension
                      key has a control on the row; a missing FACT key has
                      none — the fact key is static text, and the only control
                      that could set one is hidden exactly when it is absent.
                      Told "choose both", a reader went looking for a control
                      that is not there while Update stayed disabled. */}
                  {unkeyedDimensions.length > 0 && (
                    <p className="muted" style={{ marginTop: 8, fontSize: 12, color: "var(--danger)" }}>
                      The join condition stored for{" "}
                      <strong>{unkeyedDimensions.join(", ")}</strong>{" "}
                      {unkeyedDimensions.length === 1 ? "is not one this form can read" : "are not ones this form can read"}, so
                      no fact key can be shown for{" "}
                      {unkeyedDimensions.length === 1 ? "it" : "them"} and the dataset cannot be
                      saved while {unkeyedDimensions.length === 1 ? "it is" : "they are"} included.
                      Clear the Include box to save the dataset without{" "}
                      {unkeyedDimensions.length === 1 ? "that join" : "those joins"} — which
                      removes {unkeyedDimensions.length === 1 ? "it" : "them"} — or leave this
                      page without saving to keep{" "}
                      {unkeyedDimensions.length === 1 ? "it" : "them"} as{" "}
                      {unkeyedDimensions.length === 1 ? "it is" : "they are"}.
                    </p>
                  )}
                  {hasSemanticKeyIssues && unkeyedDimensions.length === 0 && (
                    <p
                      className="muted"
                      style={{ marginTop: 8, fontSize: 12, color: "var(--danger)" }}
                    >
                      For each dimension you include, choose both a fact key and a
                      dimension key.
                    </p>
                  )}
                </div>
              )}
              </>
              )}

              {submitError && (
                <p className="page-empty-body" style={{ color: "var(--danger)", marginTop: 12 }}>
                  {submitError}
                </p>
              )}
            </div>
          </div>
          </div>
        </form>
      )}
    </div>
  );
}
