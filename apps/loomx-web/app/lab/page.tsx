"use client";

import type * as monaco from "monaco-editor";

import { useEffect, useMemo, useRef, useState } from "react";

import { API_BASE } from "../../config";
import type React from "react";
import dynamic from "next/dynamic";
import { format as formatSql } from "sql-formatter";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useRouter, useSearchParams } from "next/navigation";
import { DataSourceSelector } from "../../components/DataSourceSelector";
// using same-origin relative API calls
const PRIMARY_DB_NAME = process.env.NEXT_PUBLIC_PRIMARY_DATABASE_NAME || "IDEASServingStoreLH";

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
  table_count?: number;
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

interface QueryResult {
  columns: string[];
  rows: unknown[][];
  executionTime?: number;
  rowCount?: number;
}

interface QueryTab {
  id: string;
  name: string;
  text: string;
  savedQueryId?: number;
  savedDescription?: string;
}

interface DatasetFilterDTO {
  column: string;
  op?: string;
  value: any;
}
interface DatasetDetailForLab {
  id: number;
  name: string;
  table_name: string;
  schema_name?: string | null;
  database_name?: string | null;
  filters?: DatasetFilterDTO[];
}

function quoteIdentifier(name: string): string {
  const safe = (name || "").replace(/]/g, "]]" );
  return `[${safe}]`;
}

function buildDatasetPreviewSql(dataset: DatasetDetailForLab): string | null {
  const schema = dataset.schema_name || "";
  const table = dataset.table_name;
  if (!table) return null;

  const tableIdent = schema ? `${quoteIdentifier(schema)}.${quoteIdentifier(table)}` : quoteIdentifier(table);

  const whereParts: string[] = [];
  for (const f of dataset.filters || []) {
    if (!f || !f.column) continue;
    const colIdent = quoteIdentifier(f.column);
    const op = (f.op || "=").toUpperCase();
    const rawVal: any = (f as any).value ?? (f as any).val;
    if (rawVal === undefined || rawVal === null || rawVal === "") continue;

    if (Array.isArray(rawVal)) {
      const vals = rawVal.map((v) => {
        if (v === null || v === undefined) return "NULL";
        if (typeof v === "string") return `'${v.replace(/'/g, "''")}'`;
        return String(v);
      });
      const inOp = op.includes("IN") ? op : "IN";
      whereParts.push(`${colIdent} ${inOp} (${vals.join(", ")})`);
    } else {
      let valStr: string;
      if (rawVal === null) {
        valStr = "NULL";
      } else if (typeof rawVal === "string") {
        valStr = `'${rawVal.replace(/'/g, "''")}'`;
      } else {
        valStr = String(rawVal);
      }
      whereParts.push(`${colIdent} ${op} ${valStr}`);
    }
  }

  const whereClause = whereParts.length > 0 ? ` WHERE ${whereParts.join(" AND ")}` : "";
  return `SELECT TOP 1000 * FROM ${tableIdent}${whereClause}`;
}

const MonacoEditor = dynamic(() => import("@monaco-editor/react"), {
  ssr: false,
});

export default function LabPage() {
  const { isAuthenticated, account } = useAuth();
  const router = useRouter();
  const searchParams = useSearchParams();
  const savedQueryId = searchParams.get('savedQueryId');
  const datasetId = searchParams.get('datasetId');

  const [dataSources, setDataSources] = useState<DataSource[]>([]);
  const [currentDataSourceId, setCurrentDataSourceId] = useState<number | null>(null);
  const [databases, setDatabases] = useState<DatabaseConfig[]>([]);
  const [currentDatabase, setCurrentDatabase] = useState<string | null>(null);
  const [tables, setTables] = useState<TableInfo[]>([]);
  const [filteredTables, setFilteredTables] = useState<TableInfo[]>([]);
  const [tableSearch, setTableSearch] = useState("");
  const [selectedTableId, setSelectedTableId] = useState<string | null>(null);
  const [tableColumns, setTableColumns] = useState<Record<string, ColumnInfo[]>>({});
  const [expandedTables, setExpandedTables] = useState<Record<string, boolean>>({});
  const [loadingColumnsFor, setLoadingColumnsFor] = useState<string | null>(null);

  const [queries, setQueries] = useState<QueryTab[]>([
    { id: "q1", name: "Query 1", text: "" },
  ]);
  const [activeQueryId, setActiveQueryId] = useState<string>("q1");
  const [isExecuting, setIsExecuting] = useState(false);
  const [results, setResults] = useState<QueryResult | null>(null);
  const [resultError, setResultError] = useState<string | null>(null);
  const [liveElapsedMs, setLiveElapsedMs] = useState<number | null>(null);
  const [sortColumnIndex, setSortColumnIndex] = useState<number | null>(null);
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [columnWidths, setColumnWidths] = useState<number[]>([]);

  const [isSaveModalOpen, setIsSaveModalOpen] = useState(false);
  const [saveName, setSaveName] = useState("");
  const [saveDescription, setSaveDescription] = useState("");
  const [isSavingQuery, setIsSavingQuery] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
   const [duplicateConflict, setDuplicateConflict] = useState<{
     id: number;
     name: string;
   } | null>(null);

  const [isLoadingDatabases, setIsLoadingDatabases] = useState(false);
  const [isLoadingTables, setIsLoadingTables] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [isDbDropdownOpen, setIsDbDropdownOpen] = useState(false);
  const [initialSavedQueryLoaded, setInitialSavedQueryLoaded] = useState(false);
  const [pendingHistoryLoaded, setPendingHistoryLoaded] = useState(false);
  const [initialDatasetPreviewLoaded, setInitialDatasetPreviewLoaded] = useState(false);

  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const executionTimerRef = useRef<number | null>(null);

  const currentDataSource = useMemo(
    () => dataSources.find((ds) => ds.id === currentDataSourceId) || null,
    [dataSources, currentDataSourceId],
  );

  const [expandedSchemas, setExpandedSchemas] = useState<Record<string, boolean>>({});

  const getActiveQuery = (): QueryTab | null => {
    return queries.find((q) => q.id === activeQueryId) ?? null;
  };

  const applySqlFormattingToActiveQuery = () => {
    const active = getActiveQuery();
    if (!active) return;
    const original = active.text ?? "";
    const trimmed = original.trim();
    if (!trimmed) return;

    let formatted = original;
    try {
      formatted = formatSql(original, { language: "tsql" });
    } catch {
      // If formatting fails, fall back to original text.
      formatted = original;
    }

    if (formatted === original) return;

    setQueries((prev) =>
      prev.map((q) =>
        q.id === activeQueryId
          ? {
              ...q,
              text: formatted,
            }
          : q,
      ),
    );

    if (editorRef.current) {
      editorRef.current.setValue(formatted);
    }
  };

  // When navigated from the homepage with a savedQueryId query
  // parameter, open that saved query in its own tab and focus it.
  useEffect(() => {
    if (!isAuthenticated) return;
    if (initialSavedQueryLoaded) return;

    const idString = Array.isArray(savedQueryId) ? savedQueryId[0] : savedQueryId;
    if (!idString) return;

    const id = Number(idString);
    if (!Number.isFinite(id)) return;

    setInitialSavedQueryLoaded(true);

    // If a tab for this saved query already exists, just focus it.
    const existing = queries.find((q) => q.savedQueryId === id);
    if (existing) {
      setActiveQueryId(existing.id);
      return;
    }

    const load = async () => {
      try {
        const userEmail = account?.email || account?.username || null;
        const res = await msalFetch(`${API_BASE}/api/v1/lab/saved-queries/${id}`, {
          headers: userEmail ? { 'x-user-email': userEmail } : undefined,
        });
        if (!res.ok) return;

        const data = await res.json();
        const name: string = data.name || `Saved Query ${id}`;
        const sql: string = data.sql || "";
        const description: string = data.description || "";

        setQueries((prev) => {
          const newId = `saved-${id}`;
          const newTab: QueryTab = {
            id: newId,
            name,
            text: sql,
            savedQueryId: id,
            savedDescription: description,
          };
          return [...prev, newTab];
        });

        setActiveQueryId(`saved-${id}`);
      } catch (e) {
        // For now, log and continue without interrupting other Lab flows.
        // eslint-disable-next-line no-console
        console.error("Failed to load saved query", e);
      }
    };

    void load();
  }, [isAuthenticated, initialSavedQueryLoaded, savedQueryId, queries, account?.email, account?.username]);

  // When coming from query history without a savedQueryId, load any
  // pending SQL text stored in localStorage by the history page and
  // open it in a new Lab tab.
  useEffect(() => {
    if (!isAuthenticated) return;
    if (pendingHistoryLoaded) return;
    if (savedQueryId) return;
    if (typeof window === "undefined") return;

    const raw = window.localStorage.getItem("lab-pending-sql");
    if (!raw) return;

    window.localStorage.removeItem("lab-pending-sql");

    try {
      const parsed = JSON.parse(raw) as { sql?: string; name?: string };
      const text = (parsed.sql || "").trim();
      if (!text) return;

      setPendingHistoryLoaded(true);

      const newId = `history-${Date.now()}`;
      const name = parsed.name || "History query";

      setQueries((prev) => [
        ...prev,
        {
          id: newId,
          name,
          text,
        },
      ]);
      setActiveQueryId(newId);
    } catch {
      // Ignore JSON/parse errors and continue without disrupting Lab.
    }
  }, [isAuthenticated, pendingHistoryLoaded, savedQueryId]);

  // When navigated from a dataset detail page with datasetId, open a
  // new Lab tab pre-populated with a simple SELECT preview for that
  // dataset, including any dataset-level filters.
  useEffect(() => {
    if (!isAuthenticated) return;
    if (initialSavedQueryLoaded) return; // prefer savedQueryId when present
    if (initialDatasetPreviewLoaded) return;

    const idString = Array.isArray(datasetId) ? datasetId[0] : datasetId;
    if (!idString) return;

    const id = Number(idString);
    if (!Number.isFinite(id)) return;

    setInitialDatasetPreviewLoaded(true);

    const load = async () => {
      try {
        const userEmail = account?.email || account?.username || null;
        const res = await msalFetch(`${API_BASE}/api/v1/datasets/${id}`, {
          headers: userEmail ? { 'x-user-email': userEmail } : undefined,
        });
        if (!res.ok) return;

        const ds: DatasetDetailForLab = await res.json();

        if (ds.database_name) {
          await switchDatabase(ds.database_name);
        }

        const sql = buildDatasetPreviewSql(ds);
        if (!sql) return;

        const newId = `dataset-${id}`;
        setQueries((prev) => [
          ...prev,
          {
            id: newId,
            name: ds.name || `Dataset ${id}`,
            text: sql,
          },
        ]);
        setActiveQueryId(newId);
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error("Failed to load dataset for Lab preview", e);
      }
    };

    void load();
  }, [isAuthenticated, initialSavedQueryLoaded, initialDatasetPreviewLoaded, datasetId, account?.email, account?.username]);

  // Pre-populate the table sidebar from the cache written by the home page.
  // This gives instant table visibility while the full data-sources/active
  // round-trip (warehouse cold start) runs in the background below.
  useEffect(() => {
    if (!isAuthenticated || typeof window === 'undefined') return;
    const userEmail = account?.email || account?.username || null;
    const key = `loomx_lab_tables_v1_${userEmail || 'anon'}`;
    try {
      const raw = localStorage.getItem(key);
      if (!raw) return;
      const { database, tables: rawTables, cachedAt } = JSON.parse(raw);
      // Skip if stale (> 15 minutes old)
      if (!rawTables || Date.now() - cachedAt > 15 * 60 * 1000) return;
      const withIds: TableInfo[] = (rawTables as any[]).map((t: any, idx: number) => ({
        id: t.id || `${t.schema}.${t.name}.${idx}`,
        schema: t.schema,
        name: t.name,
        fullName: t.fullName,
      }));
      setCurrentDatabase(database);
      setTables(withIds);
      setFilteredTables(withIds);
    } catch { /* ignore parse errors */ }
  }, [isAuthenticated, account?.email, account?.username]);

  useEffect(() => {
    if (!isAuthenticated) return;

    const loadInitial = async () => {
      try {
        setIsLoadingDatabases(true);
        setLoadError(null);

        const userEmail = account?.email || account?.username || null;

        // Load data sources from the new data_sources table
        const res = await msalFetch(`${API_BASE}/api/v1/data-sources/active`, {
          headers: userEmail ? { 'x-user-email': userEmail } : undefined,
        });
        if (!res.ok) {
          throw new Error("Failed to load data sources");
        }
        const data = await res.json();
        const sources: DataSource[] = data.dataSources || [];

        setDataSources(sources);

        // Auto-select favorite data source, or first available
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
        setIsLoadingDatabases(false);
      }
    };

    loadInitial();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isAuthenticated]);
  const switchDatabase = async (databaseName: string) => {
    try {
      setIsLoadingTables(true);
      setLoadError(null);

      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/lab/switch-database`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(userEmail ? { 'x-user-email': userEmail } : {}),
        },
        body: JSON.stringify({ database_name: databaseName }),
      });
      const data = await res.json();
      if (!res.ok || !data.success) {
        throw new Error(data.error || "Failed to switch database");
      }

      setCurrentDatabase(databaseName);
      await loadTables(false, databaseName);
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      setLoadError(message);
    } finally {
      setIsLoadingTables(false);
    }
  };

  // Allow the global header refresh button to force a fresh SQL connection
  // and reload table metadata when on the Lab page.
  useEffect(() => {
    if (typeof window === "undefined") return;

    const handleLabRefresh = () => {
      setResultError(null);
      setResults(null);
      setSelectedTableId(null);
      setTableColumns({});
      setExpandedTables({});
      setExpandedSchemas({});
      setTableSearch("");

      const run = async () => {
        try {
          setIsLoadingTables(true);
          if (currentDatabase) {
            await switchDatabase(currentDatabase);
          } else if (currentDataSource) {
            await switchDatabase(currentDataSource.database_name);
          }
          // After re-establishing the connection, force-refresh the
          // table metadata so any schema changes are reflected.
          await loadTables(true);
        } finally {
          setIsLoadingTables(false);
        }
      };

      void run();
    };

    window.addEventListener("labRefresh", handleLabRefresh);
    return () => {
      window.removeEventListener("labRefresh", handleLabRefresh);
    };
  }, [currentDatabase, currentDataSource, switchDatabase]);

  const loadTables = async (refresh: boolean = false, databaseName?: string) => {
    const dbToUse = databaseName || currentDatabase;

    if (!dbToUse) {
      console.warn('[Lab] No database selected, skipping table load');
      return;
    }

    const params = new URLSearchParams();
    if (refresh) params.append('refresh', '1');
    params.append('database', dbToUse);

    const queryString = params.toString() ? `?${params.toString()}` : '';
    const userEmail = account?.email || account?.username || null;

    console.log(`[Lab] Loading tables from database: ${dbToUse}`);
    const res = await msalFetch(`${API_BASE}/api/v1/lab/tables${queryString}`, {
      headers: userEmail ? { 'x-user-email': userEmail } : undefined,
    });
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

    console.log(`[Lab] Loaded ${withIds.length} tables from ${dbToUse}`);
    setTables(withIds);
    setFilteredTables(withIds);
  };

  const schemaGroups = useMemo(() => {
    const groups: Record<string, TableInfo[]> = {};
    for (const t of filteredTables) {
      const key = t.schema || "default";
      if (!groups[key]) groups[key] = [];
      groups[key].push(t);
    }
    return Object.entries(groups).sort((a, b) => a[0].localeCompare(b[0]));
  }, [filteredTables]);

  const handleSearchChange = (value: string) => {
    setTableSearch(value);
    const term = value.trim().toLowerCase();
    if (!term) {
      setFilteredTables(tables);
      return;
    }
    setFilteredTables(
      tables.filter(
        (t) => {
          const basicMatch =
            t.name.toLowerCase().includes(term) ||
            t.schema.toLowerCase().includes(term) ||
            (t.fullName && t.fullName.toLowerCase().includes(term));

          const cols = tableColumns[t.id] || [];
          const columnMatch = cols.some(
            (c) =>
              c.name.toLowerCase().includes(term) ||
              c.dataType.toLowerCase().includes(term),
          );

          return basicMatch || columnMatch;
        },
      ),
    );
  };

  const getSelectedTable = (): TableInfo | null => {
    if (!selectedTableId) return null;
    return tables.find((t) => t.id === selectedTableId) || null;
  };

  const buildQualifiedName = (table: TableInfo | null) => {
    if (!table) return "";
    return `[${table.schema}].[${table.name}]`;
  };

  const selectTable = async (table: TableInfo) => {
    setSelectedTableId(table.id);
    setResultError(null);
    const qualified = buildQualifiedName(table);
    const sql = `SELECT TOP 100 * FROM ${qualified};`;

    setResults(null);
    await executeQuery(sql);
  };

  const toggleTableColumns = async (table: TableInfo) => {
    const tableId = table.id;

    setExpandedTables((prev) => ({
      ...prev,
      [tableId]: !prev[tableId],
    }));

    // If we're collapsing, no need to load columns
    if (expandedTables[tableId]) {
      return;
    }

    // If columns are already loaded, don't refetch
    if (tableColumns[tableId]) {
      return;
    }

    try {
      setLoadingColumnsFor(tableId);
      const schemaEncoded = encodeURIComponent(table.schema);
      const nameEncoded = encodeURIComponent(table.name);
      const userEmail = account?.email || account?.username || null;

      const params = new URLSearchParams();
      if (currentDatabase) {
        params.append('database', currentDatabase);
      }
      const queryString = params.toString() ? `?${params.toString()}` : '';

      const res = await msalFetch(`${API_BASE}/api/v1/lab/schema/${schemaEncoded}/${nameEncoded}${queryString}`, {
        headers: userEmail ? { 'x-user-email': userEmail } : undefined,
      });
      const data = await res.json();
      if (!res.ok || !data.success) {
        throw new Error(data.error || "Failed to load column schema");
      }

      const cols: ColumnInfo[] = (data.schema?.columns || []).map((c: any) => ({
        name: c.name,
        dataType: c.dataType,
      }));

      setTableColumns((prev) => ({
        ...prev,
        [tableId]: cols,
      }));
    } catch (e) {
      // Surface schema errors in the main error area
      const message = e instanceof Error ? e.message : "Failed to load column schema";
      setResultError(message);
    } finally {
      setLoadingColumnsFor(null);
    }
  };

  const executeQuery = async (overrideSql?: string) => {
    const active = getActiveQuery();
    const raw = typeof overrideSql === "string" ? overrideSql : active?.text ?? "";
    const text = (raw ?? "").toString().trim();
    if (!text) {
      setResultError("Enter a query to execute");
      return;
    }

    try {
      setIsExecuting(true);
      setResultError(null);
      setResults(null);
      setSortColumnIndex(null);
      setSortDirection("asc");

      const start = performance.now();
      setLiveElapsedMs(0);

      if (executionTimerRef.current !== null) {
        window.clearInterval(executionTimerRef.current);
      }
      executionTimerRef.current = window.setInterval(() => {
        setLiveElapsedMs(performance.now() - start);
      }, 100);

      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/lab/query`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(userEmail ? { 'x-user-email': userEmail } : {}),
        },
        body: JSON.stringify({
          query: text,
          database: currentDatabase,
          savedQueryId: active?.savedQueryId ?? null,
          executedBy: userEmail,
        }),
      });
      const responseText = await res.text();
      const data = JSON.parse(responseText, (key, value) => {
        // Prevent automatic Date object conversion - keep dates as strings
        // Match ISO 8601 formats: 2026-02-05T02:48:38, 2026-02-05T02:48:38.173, 2026-02-05T02:48:38Z, etc.
        if (typeof value === 'string' && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}/.test(value)) {
          return value; // Return ISO string as-is, don't convert to Date - display exactly as from database
        }
        return value;
      });

      if (!res.ok || !data.success) {
        throw new Error(data.error || "Query execution failed");
      }

      setResults({
        columns: data.columns || [],
        rows: data.rows || [],
        executionTime: data.executionTime,
        rowCount: data.rowCount,
      });
      setColumnWidths([]);
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      setResultError(message);
      setResults(null);
    } finally {
      setIsExecuting(false);
      if (executionTimerRef.current !== null) {
        window.clearInterval(executionTimerRef.current);
        executionTimerRef.current = null;
      }
      setLiveElapsedMs(null);
    }
  };

  const handleSort = (columnIndex: number) => {
    if (!results) return;

    // 3-state sort cycle per column: unsorted -> asc -> desc -> unsorted
    if (sortColumnIndex === columnIndex) {
      if (sortDirection === "asc") {
        setSortDirection("desc");
      } else {
        setSortColumnIndex(null);
        setSortDirection("asc");
      }
    } else {
      setSortColumnIndex(columnIndex);
      setSortDirection("asc");
    }
  };

  const getSortedRows = () => {
    if (!results) return [];
    if (sortColumnIndex === null) return results.rows;

    const rowsCopy = [...results.rows];
    const colIndex = sortColumnIndex;
    const directionMultiplier = sortDirection === "asc" ? 1 : -1;

    rowsCopy.sort((a, b) => {
      const aVal = a[colIndex];
      const bVal = b[colIndex];

      // Convert Date objects to ISO strings for proper sorting, otherwise use String()
      const aStr = aVal == null ? "" : (aVal instanceof Date ? aVal.toISOString() : String(aVal));
      const bStr = bVal == null ? "" : (bVal instanceof Date ? bVal.toISOString() : String(bVal));

      const aNum = parseFloat(aStr);
      const bNum = parseFloat(bStr);

      const aIsNum = !Number.isNaN(aNum) && aStr.trim() !== "";
      const bIsNum = !Number.isNaN(bNum) && bStr.trim() !== "";

      if (aIsNum && bIsNum) {
        if (aNum === bNum) return 0;
        return aNum > bNum ? directionMultiplier : -directionMultiplier;
      }

      if (aStr === bStr) return 0;
      return aStr > bStr ? directionMultiplier : -directionMultiplier;
    });

    return rowsCopy;
  };

  const handleColumnResizeMouseDown = (
    event: React.MouseEvent<HTMLDivElement>,
    columnIndex: number,
  ) => {
    if (!results) return;

    event.preventDefault();
    event.stopPropagation();

    const startX = event.clientX;
    const thElement = event.currentTarget.parentElement as HTMLTableCellElement | null;
    if (!thElement) return;

    const startWidth = thElement.offsetWidth;

    const onMouseMove = (moveEvent: MouseEvent) => {
      const delta = moveEvent.clientX - startX;
      const newWidth = Math.max(80, startWidth + delta);

      setColumnWidths((prev) => {
        const base = prev.length ? prev : new Array(results.columns.length).fill(0);
        const next = [...base];
        next[columnIndex] = newWidth;
        return next;
      });
    };

    const onMouseUp = () => {
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    };

    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
  };

  const runCurrentQuery = () => {
    const editor = editorRef.current;
    if (editor) {
      const model = editor.getModel();
      const selection = editor.getSelection();

      if (model && selection && !selection.isEmpty()) {
        const selected = model.getValueInRange(selection);
        executeQuery(selected);
        return;
      }

      const fullText = model?.getValue() ?? "";
      executeQuery(fullText);
      return;
    }

    executeQuery();
  };

  const openSaveModal = () => {
    const active = getActiveQuery();
    const defaultName = active?.name || "Untitled Query";

    setSaveName(defaultName);
    setSaveDescription(active?.savedDescription ?? "");
    setSaveError(null);
    setDuplicateConflict(null);
    setIsSaveModalOpen(true);
  };

  const saveCurrentQuery = async (
    mode: "auto" | "saveNew" | "updateExisting" = "auto",
  ) => {
    const active = getActiveQuery();
    const text = active?.text.trim() ?? "";
    if (!text) {
      setResultError("Cannot save an empty query");
      return;
    }

    const trimmedName = saveName.trim();
    const trimmedDescription = saveDescription.trim();
    if (!trimmedName) {
      setSaveError("Name is required to save this query.");
      return;
    }

    try {
      setIsSavingQuery(true);
      setSaveError(null);
      setResultError(null);
      const userEmail = account?.email || account?.username;

      let targetSavedQueryId = active?.savedQueryId ?? null;
      let isUpdate = !!targetSavedQueryId;

      if (mode === "saveNew") {
        // Always create a new saved query, even if this tab already
        // has a savedQueryId or a conflicting name.
        isUpdate = false;
        targetSavedQueryId = null;
      } else if (mode === "updateExisting") {
        // Force updating the conflicting saved query when we have one.
        if (duplicateConflict) {
          targetSavedQueryId = duplicateConflict.id;
          isUpdate = true;
        }
      } else if (!isUpdate && typeof window !== "undefined") {
        // auto mode: for brand new saves, check for existing queries
        // with the same name and, if found, surface options in the dialog.
        try {
          const userEmail = account?.email || account?.username || null;
          const existingRes = await msalFetch(`${API_BASE}/api/v1/lab/saved-queries`, {
            headers: userEmail ? { 'x-user-email': userEmail } : undefined,
          });
          if (existingRes.ok) {
            const existing = (await existingRes.json()) as Array<{
              id: number;
              name?: string;
            }>;
            const conflict = existing.find(
              (q) =>
                (q.name || "").trim().toLowerCase() ===
                trimmedName.toLowerCase(),
            );
            if (conflict) {
              setDuplicateConflict({
                id: conflict.id,
                name: conflict.name || trimmedName,
              });
              setSaveError(
                `A saved query named "${trimmedName}" already exists. Choose "Save as new" to create another copy, or "Update" to overwrite the existing one.`,
              );
              setIsSavingQuery(false);
              return;
            }
          }
        } catch {
          // If the duplicate check fails, fall back to normal save behaviour.
        }
      }

      const url = isUpdate && targetSavedQueryId
        ? `${API_BASE}/api/v1/lab/saved-queries/${targetSavedQueryId}`
        : `${API_BASE}/api/v1/lab/saved-queries`;
      const method = isUpdate ? "PUT" : "POST";

      const headers: Record<string, string> = { "Content-Type": "application/json" };
      if (userEmail) headers["x-user-email"] = userEmail;
      const res = await msalFetch(url, {
        method,
        headers,
        body: JSON.stringify({
          name: trimmedName,
          description: trimmedDescription,
          sql: text,
          // dataset_id is optional and not used from Lab today
          dataset_id: null,
          ...(isUpdate
            ? { modified_by: userEmail || undefined }
            : { created_by: userEmail || undefined }),
        }),
      });
      if (!res.ok) {
        let errorMessage = "Failed to save query";
        try {
          const data = await res.json();
          if (data && (data.error || data.detail)) {
            errorMessage = data.error || data.detail;
          }
        } catch {
          // Ignore JSON parse errors and use default message
        }
        throw new Error(errorMessage);
      }

      const saved = await res.json();

      // Update the active tab title and saved metadata based on
      // the server response so the UI reflects the saved name.
      setQueries((prev) =>
        prev.map((q) =>
          q.id === activeQueryId
            ? {
                ...q,
                name: saved.name ?? trimmedName,
                savedQueryId: saved.id ?? targetSavedQueryId ?? q.savedQueryId,
                savedDescription: saved.description ?? trimmedDescription,
              }
            : q,
        ),
      );

      setIsSaveModalOpen(false);
      setDuplicateConflict(null);
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      setSaveError(message);
    } finally {
      setIsSavingQuery(false);
    }
  };

  const applyTemplate = (type: "sample" | "rowcount" | "schema") => {
    const table = getSelectedTable();
    if (!table) {
      setResultError("Select a table first to use templates");
      return;
    }

    const qualified = buildQualifiedName(table);
    const active = getActiveQuery();
    const existingText = active?.text ?? "";

    const appendTemplate = (templateSql: string) => {
      const trimmedExisting = existingText.trimEnd();
      const separator = trimmedExisting ? "\n\n" : "";
      const newText = `${trimmedExisting}${separator}${templateSql}`;

      setQueries((prev) =>
        prev.map((q) =>
          q.id === activeQueryId
            ? {
                ...q,
                text: newText,
              }
            : q,
        ),
      );
    };

    if (type === "sample") {
      const sql = `-- Template: sample 100 rows from ${qualified}\nSELECT TOP 100 * FROM ${qualified};`;
      appendTemplate(sql);
    } else if (type === "rowcount") {
      const sql = `-- Template: row count for ${qualified}\nSELECT COUNT_BIG(*) AS TotalRows FROM ${qualified};`;
      appendTemplate(sql);
    } else {
      const sql =
        `-- Template: column schema for ${qualified}\n` +
        "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = '" +
        table.name +
        "' AND TABLE_SCHEMA = '" +
        table.schema +
        "';";
      appendTemplate(sql);
    }
  };

  const addNewQueryTab = () => {
    setResults(null);
    setResultError(null);
    setQueries((prev) => {
      const nextIndex = prev.length + 1;
      const newId = `q${nextIndex}`;
      const newTab: QueryTab = {
        id: newId,
        name: `Query ${nextIndex}`,
        text: "",
      };
      setActiveQueryId(newId);
      return [...prev, newTab];
    });
  };

  const closeQueryTab = (id: string) => {
    if (queries.length === 1) return;

    setQueries((prev) => {
      const remaining = prev.filter((q) => q.id !== id);
      if (id === activeQueryId) {
        const nextActive = remaining[remaining.length - 1];
        if (nextActive) {
          setActiveQueryId(nextActive.id);
        }
      }
      return remaining;
    });
  };

  const rowCount = results?.rowCount ?? results?.rows?.length ?? 0;

  const formatExecutionTime = (seconds: number | undefined | null): string => {
    if (seconds == null || Number.isNaN(seconds)) {
      return "0.0 ms";
    }

    // Show sub-second timings in milliseconds for better visibility
    if (seconds < 1) {
      const ms = Math.max(seconds * 1000, 1);
      return `${ms.toFixed(0)} ms`;
    }

    return `${seconds.toFixed(2)} s`;
  };

  const executionTime = results?.executionTime ?? 0;
  const sortedRows = getSortedRows();

  return (
    <>
      {!isAuthenticated && (
        <p className="muted" style={{ margin: "1.5rem" }}>
          Sign in and connect to LoomX to use SQL Lab.
        </p>
      )}

      {isAuthenticated && (
        <div className="main-content">
          {/* Sidebar */}
          <aside className="sidebar">
            <div className="sidebar-header">
              <div className="connection-status">
                <i
                  className={
                    "fas fa-circle " +
                    (currentDataSource ? "status-connected" : "status-disconnected")
                  }
                />
                <span className="connection-text">
                  {currentDataSource
                    ? `Connected to ${currentDataSource.name}`
                    : "Select Data Source"}
                </span>
              </div>
              <div className="sidebar-header-main-row">
                <h3>
                  <i className="fas fa-table" /> Database Tables
                  <span className="table-stats" style={{ marginLeft: '0.75rem' }}>
                    {isLoadingTables ? "Loading tables..." : `${filteredTables.length} tables`}
                  </span>
                </h3>
                <button
                  type="button"
                  className="sidebar-sync-btn"
                  title="Sync schema from source"
                  onClick={() => {
                    setResultError(null);
                    setSelectedTableId(null);
                    setTableColumns({});
                    setExpandedTables({});
                    setExpandedSchemas({});
                    setTableSearch("");

                    const run = async () => {
                      try {
                        setIsLoadingTables(true);
                        await loadTables(true);
                      } finally {
                        setIsLoadingTables(false);
                      }
                    };

                    void run();
                  }}
                  disabled={isLoadingTables || !currentDatabase}
                >
                  <i className={isLoadingTables ? "fas fa-sync-alt fa-spin" : "fas fa-sync-alt"} />
                </button>
              </div>
            </div>

            <div style={{ padding: '1rem', borderBottom: '1px solid #e1e5e9' }}>
              <DataSourceSelector
                value={currentDataSourceId}
                onChange={async (id) => {
                  if (id === null) return;
                  setCurrentDataSourceId(id);
                  // Look up the data source to get its database_name
                  const selectedDs = dataSources.find(ds => ds.id === id);
                  if (selectedDs?.database_name) {
                    await switchDatabase(selectedDs.database_name);
                  }
                }}
                label="Data Source"
                required={false}
              />
            </div>

            <div className="search-container">
              <label className="chart-builder-label">
                <span>Search Tables</span>
              </label>
              <div style={{ position: 'relative' }}>
                <input
                  type="text"
                  className="search-input"
                  placeholder="Search tables..."
                  value={tableSearch}
                  onChange={(e) => handleSearchChange(e.target.value)}
                  disabled={isLoadingDatabases || !currentDatabase}
                />
                <i className="fas fa-search search-icon" />
              </div>
            </div>

            <div className="tables-list" id="tablesList">
              {isLoadingTables && (
                <div className="loading-tables">
                  <i className="fas fa-spinner fa-spin" />
                  <span>Loading tables...</span>
                </div>
              )}
              {!isLoadingTables && schemaGroups.length === 0 && (
                <div className="loading-tables">
                  <span>No tables found.</span>
                </div>
              )}
              {!isLoadingTables &&
                schemaGroups.map(([schema, items]) => (
                  <div key={schema} className="schema-group">
                    <div
                      className="schema-header"
                      onClick={() =>
                        setExpandedSchemas((prev) => {
                          const isExpanded = prev[schema] ?? false;
                          return {
                            ...prev,
                            [schema]: !isExpanded,
                          };
                        })
                      }
                    >
                      <span className="schema-name">
                        <i className="fas fa-layer-group schema-icon" />
                        {schema}
                      </span>
                      <span className="schema-count">
                        {items.length} tables
                        <i
                          className={
                            "fas schema-toggle-icon " +
                            ((expandedSchemas[schema] ?? false) ? "fa-chevron-up" : "fa-chevron-down")
                          }
                        />
                      </span>
                    </div>
                    {(expandedSchemas[schema] ?? false) && (
                      <div className="schema-tables" style={{ maxHeight: '40vh', overflowY: 'auto' }}>
                      {items.map((t) => (
                        <div key={t.id} className="schema-table-wrapper">
                          <div
                            className={
                              "table-item" + (t.id === selectedTableId ? " selected" : "")
                            }
                            onClick={async () => {
                              await selectTable(t);
                            }}
                          >
                            <div className="table-icon">
                              <i className="fas fa-table" />
                            </div>
                            <div className="table-info">
                              <div className="table-name-row">
                                <span
                                  className="table-name"
                                  title={t.fullName || `${t.schema}.${t.name}`}
                                >
                                  {t.name}
                                </span>
                              </div>
                            </div>
                            <div
                              className="column-toggle-icon"
                              onClick={async (e) => {
                                e.stopPropagation();
                                await toggleTableColumns(t);
                              }}
                            >
                              <i
                                className={
                                  "fas " +
                                  (expandedTables[t.id] ? "fa-chevron-up" : "fa-chevron-down")
                                }
                              />
                            </div>
                          </div>

                          {expandedTables[t.id] && (
                            <div className="column-list">
                              {loadingColumnsFor === t.id && (
                                <div className="column-loading">
                                  <i className="fas fa-spinner fa-spin" />
                                  <span>Loading columns...</span>
                                </div>
                              )}
                              {loadingColumnsFor !== t.id && (tableColumns[t.id] || []).length === 0 && (
                                <div className="column-empty">No columns found.</div>
                              )}
                              {loadingColumnsFor !== t.id && (tableColumns[t.id] || []).length > 0 && (
                                <>
                                  {(tableColumns[t.id] || []).map((col) => (
                                    <div key={col.name} className="column-item" style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                                      <span
                                        className="column-name"
                                        title={col.name}
                                        style={{
                                          width: '180px',
                                          overflow: 'hidden',
                                          textOverflow: 'ellipsis',
                                          whiteSpace: 'nowrap',
                                          display: 'inline-block',
                                          cursor: 'pointer',
                                          fontWeight: 500
                                        }}
                                      >
                                        {col.name}
                                      </span>
                                      <span className="column-type" style={{ color: '#888', fontSize: '12px' }}>{col.dataType}</span>
                                    </div>
                                  ))}
                                </>
                              )}
                            </div>
                          )}
                        </div>
                      ))}
                      </div>
                    )}
                  </div>
                ))}
            </div>
          </aside>

          {/* Main content area */}
          <section className="content-area">
            <div className="query-section" id="querySection">
              <div className="query-tabs" id="queryTabs">
                {queries.map((q) => (
                  <button
                    key={q.id}
                    type="button"
                    className={"tab" + (q.id === activeQueryId ? " active" : "")}
                    onClick={() => setActiveQueryId(q.id)}
                  >
                    <span className="tab-name">{q.name}</span>
                    {queries.length > 1 && (
                      <span
                        className="tab-close"
                        onClick={(e) => {
                          e.stopPropagation();
                          closeQueryTab(q.id);
                        }}
                      >
                        &times;
                      </span>
                    )}
                  </button>
                ))}
                <button
                  type="button"
                  className="tab new-tab"
                  onClick={addNewQueryTab}
                >
                  <i className="fas fa-plus" />
                  <span className="tab-name">New Query</span>
                </button>
              </div>

              <div className="query-editor-container">
                <MonacoEditor
                  height="100%"
                  defaultLanguage="sql"
                  theme="vs"
                  value={getActiveQuery()?.text ?? ""}
                  onChange={(value) => {
                    const text = value ?? "";
                    const currentId = activeQueryId;
                    setQueries((prev) =>
                      prev.map((q) =>
                        q.id === currentId
                          ? {
                              ...q,
                              text,
                            }
                          : q,
                      ),
                    );
                  }}
                  onMount={(editorInstance) => {
                    editorRef.current = editorInstance;
                  }}
                  options={{
                    minimap: { enabled: false },
                    wordWrap: "on",
                    fontSize: 13,
                    automaticLayout: true,
                    glyphMargin: false,
                    lineNumbersMinChars: 2,
                  }}
                />
              </div>

              <div className="query-templates" style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <button
                  type="button"
                  className="execute-btn"
                  style={{ padding: "0.35rem 0.8rem", fontSize: "0.8rem" }}
                  onClick={runCurrentQuery}
                  disabled={isExecuting}
                >
                  <i className="fas fa-play" /> Run
                </button>
                <button
                  type="button"
                  className="save-btn"
                  style={{ padding: "0.35rem 0.8rem", fontSize: "0.8rem" }}
                  onClick={openSaveModal}
                >
                  <i className="fas fa-save" /> Save
                </button>
                <span>Quick Templates:</span>
                <button
                  type="button"
                  className="template-btn"
                  onClick={() => applyTemplate("sample")}
                >
                  Sample Data
                </button>
                <button
                  type="button"
                  className="template-btn"
                  onClick={() => applyTemplate("rowcount")}
                >
                  Row Count
                </button>
                <button
                  type="button"
                  className="template-btn"
                  onClick={() => applyTemplate("schema")}
                >
                  Schema Info
                </button>
                <div style={{ flex: 1 }} />
                <button
                  type="button"
                  className="format-btn"
                  onClick={applySqlFormattingToActiveQuery}
                  style={{ marginLeft: 'auto' }}
                >
                  <i className="fas fa-magic" aria-hidden="true" /> Format
                </button>
              </div>
            </div>

            <div className="results-section">
              <div className="section-header">
                <h3 id="resultsTitle">
                  <i className="fas fa-table" /> Query Results
                </h3>
                <div className="results-actions">
                  <span id="resultStats" className="result-stats">
                    {isExecuting && (
                      <>
                        <i className="fas fa-spinner fa-spin" style={{ marginRight: "0.4rem" }} />
                        {`Running • ${formatExecutionTime((liveElapsedMs ?? 0) / 1000)}`}
                      </>
                    )}
                    {!isExecuting && rowCount > 0 &&
                      `${rowCount.toLocaleString()} rows • ${formatExecutionTime(executionTime)}`}
                  </span>
                </div>
              </div>

              <div id="resultsContainer" className="results-container">
                {!results && !resultError && (
                  <div className="empty-state">
                    <i className="fas fa-chart-bar" />
                    <h3>Ready for Analysis</h3>
                    <p>
                      Select a table from the left sidebar or write a custom SQL query to get started.
                    </p>
                  </div>
                )}

                {resultError && (
                  <div className="empty-state" style={{ color: "#b91c1c" }}>
                    <i className="fas fa-exclamation-triangle" />
                    <h3>Error</h3>
                    <p>{resultError}</p>
                  </div>
                )}

                {results && !resultError && (
                  <div className="results-table-container">
                    <table className="results-table">
                      <thead>
                        <tr>
                          {results.columns.map((col, colIndex) => {
                            const isSorted = sortColumnIndex === colIndex;
                            const sortIconClass = !isSorted
                              ? "fas fa-sort column-sort-icon"
                              : sortDirection === "asc"
                              ? "fas fa-sort-up column-sort-icon"
                              : "fas fa-sort-down column-sort-icon";

                            const explicitWidth = columnWidths[colIndex];

                            return (
                              <th
                                key={col}
                                onClick={() => handleSort(colIndex)}
                                className={isSorted ? "sorted" : undefined}
                                style={explicitWidth ? { width: explicitWidth } : undefined}
                              >
                                <span className="column-header-label">{col}</span>
                                <i className={sortIconClass} />
                                <div
                                  className="column-resizer"
                                  onMouseDown={(event) =>
                                    handleColumnResizeMouseDown(event, colIndex)
                                  }
                                />
                              </th>
                            );
                          })}
                        </tr>
                      </thead>
                      <tbody>
                        {sortedRows.map((row, idx) => (
                          <tr key={idx}>
                            {row.map((cell, cIdx) => {
                              const value = cell ?? "";
                              // If value is a Date object, use ISO string; otherwise convert to string
                              const stringValue = value instanceof Date ? value.toISOString() : String(value);

                              const numericCandidate =
                                typeof value === "number" ||
                                (typeof value === "string" && value.trim() !== "");

                              const parsed =
                                numericCandidate &&
                                !Number.isNaN(Number.parseFloat(stringValue))
                                  ? Number.parseFloat(stringValue)
                                  : null;

                              const isNumeric = parsed !== null;
                              const explicitWidth = columnWidths[cIdx];

                              // eslint-disable-next-line react/no-array-index-key
                              return (
                                <td
                                  key={cIdx}
                                  className={isNumeric ? "numeric-cell" : undefined}
                                  style={
                                    explicitWidth
                                      ? { width: explicitWidth, minWidth: explicitWidth }
                                      : undefined
                                  }
                                >
                                  {stringValue}
                                </td>
                              );
                            })}
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </div>
            </div>

            {loadError && (
              <div className="save-query-modal-overlay">
                <div
                  className="save-query-modal"
                  role="alertdialog"
                  aria-modal="true"
                  aria-labelledby="lab-error-title"
                >
                  <div className="save-query-modal-header">
                    <h3 id="lab-error-title" className="save-query-modal-title">
                      Connection problem
                    </h3>
                    <button
                      type="button"
                      className="save-query-modal-close"
                      onClick={() => setLoadError(null)}
                      aria-label="Dismiss error dialog"
                    >
                      &times;
                    </button>
                  </div>
                  <div className="save-query-modal-body">
                    <p style={{ marginBottom: 8 }}>
                      We couldn&apos;t connect to the primary database for Lab.
                    </p>
                    <p className="save-query-modal-error">{loadError}</p>
                    <p style={{ fontSize: "0.8rem", color: "#6b7280", marginTop: 8 }}>
                      You can try again from the header refresh button or adjust the
                      Fabric SQL connection settings.
                    </p>
                  </div>
                  <div className="save-query-modal-footer">
                    <button
                      type="button"
                      className="save-query-btn-primary"
                      onClick={() => setLoadError(null)}
                    >
                      OK
                    </button>
                  </div>
                </div>
              </div>
            )}
          </section>
        </div>
      )}

      {isAuthenticated && isSaveModalOpen && (
        <div className="save-query-modal-overlay">
          <div className="save-query-modal" role="dialog" aria-modal="true" aria-labelledby="save-query-title">
            <div className="save-query-modal-header">
              <h3 id="save-query-title" className="save-query-modal-title">
                Save query
              </h3>
              <button
                type="button"
                className="save-query-modal-close"
                onClick={() => {
                  if (!isSavingQuery) {
                    setIsSaveModalOpen(false);
                    setDuplicateConflict(null);
                  }
                }}
                aria-label="Close save dialog"
              >
                &times;
              </button>
            </div>
            <div className="save-query-modal-body">
              <div>
                <div className="save-query-field-label">Name</div>
                <input
                  type="text"
                  className="save-query-input"
                  value={saveName}
                  onChange={(e) => setSaveName(e.target.value)}
                  disabled={isSavingQuery}
                />
              </div>
              <div>
                <div className="save-query-field-label">Description</div>
                <textarea
                  className="save-query-textarea"
                  value={saveDescription}
                  onChange={(e) => setSaveDescription(e.target.value)}
                  disabled={isSavingQuery}
                />
              </div>
              {saveError && <div className="save-query-modal-error">{saveError}</div>}
            </div>
            <div className="save-query-modal-footer">
              <button
                type="button"
                className="save-query-btn-cancel"
                onClick={() => {
                  if (!isSavingQuery) {
                    setIsSaveModalOpen(false);
                    setDuplicateConflict(null);
                  }
                }}
                disabled={isSavingQuery}
              >
                Cancel
              </button>
              {duplicateConflict ? (
                <>
                  <button
                    type="button"
                    className="save-query-btn-cancel"
                    onClick={() => void saveCurrentQuery("saveNew")}
                    disabled={isSavingQuery || !saveName.trim()}
                  >
                    Save as new
                  </button>
                  <button
                    type="button"
                    className="save-query-btn-primary"
                    onClick={() => void saveCurrentQuery("updateExisting")}
                    disabled={isSavingQuery || !saveName.trim()}
                  >
                    {isSavingQuery ? "Saving..." : "Update"}
                  </button>
                </>
              ) : (
                <button
                  type="button"
                  className="save-query-btn-primary"
                  onClick={() => void saveCurrentQuery("auto")}
                  disabled={isSavingQuery || !saveName.trim()}
                >
                  {isSavingQuery ? "Saving..." : "Save"}
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </>
  );
}
