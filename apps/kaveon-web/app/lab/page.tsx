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
// using same-origin relative API calls
const PRIMARY_DB_NAME = process.env.NEXT_PUBLIC_PRIMARY_DATABASE_NAME || "";

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

interface QueryHistoryRow {
  id: number;
  sql_text: string;
  status: string;
  started_at: string;
  row_count?: number | null;
  duration_ms?: number | null;
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

/** Split a SQL string on `;` delimiters, respecting single/double-quoted strings. */
function splitStatements(sql: string): string[] {
  const stmts: string[] = [];
  let current = "";
  let inString = false;
  let stringChar = "";

  for (let i = 0; i < sql.length; i++) {
    const ch = sql[i];
    if (inString) {
      current += ch;
      if (ch === stringChar) {
        // doubled quote is an escape (e.g. '' or "")
        if (i + 1 < sql.length && sql[i + 1] === stringChar) {
          current += sql[++i];
        } else {
          inString = false;
        }
      }
    } else if (ch === "'" || ch === '"') {
      inString = true;
      stringChar = ch;
      current += ch;
    } else if (ch === ";") {
      const trimmed = current.trim();
      if (trimmed) stmts.push(trimmed);
      current = "";
    } else {
      current += ch;
    }
  }

  const trimmed = current.trim();
  if (trimmed) stmts.push(trimmed);
  return stmts;
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

  // Query history panel
  const [isHistoryPanelOpen, setIsHistoryPanelOpen] = useState(false);
  const [queryHistory, setQueryHistory] = useState<QueryHistoryRow[]>([]);
  const [isLoadingHistory, setIsLoadingHistory] = useState(false);
  const [historySearch, setHistorySearch] = useState("");

  // Resizable panels
  const [sidebarWidth, setSidebarWidth] = useState(380);
  const [editorHeightPx, setEditorHeightPx] = useState<number | null>(null);

  const [queries, setQueries] = useState<QueryTab[]>([
    { id: "q1", name: "Query 1", text: "" },
  ]);
  const [activeQueryId, setActiveQueryId] = useState<string>("q1");
  const [isExecuting, setIsExecuting] = useState(false);
  const [isEstimating, setIsEstimating] = useState(false);
  const [estimatedRows, setEstimatedRows] = useState<number | null>(null);
  const [rowLimit, setRowLimit] = useState<number>(1000);
  const [multiResults, setMultiResults] = useState<Array<{ sql: string; result?: QueryResult; error?: string }> | null>(null);
  const [results, setResults] = useState<QueryResult | null>(null);
  const [resultError, setResultError] = useState<string | null>(null);
  const [liveElapsedMs, setLiveElapsedMs] = useState<number | null>(null);
  const [sortColumnIndex, setSortColumnIndex] = useState<number | null>(null);
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [columnWidths, setColumnWidths] = useState<number[]>([]);

  // Tab rename
  const [renamingTabId, setRenamingTabId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");

  // Results filter (search within displayed rows)
  const [resultFilter, setResultFilter] = useState("");

  // Cell copy feedback toast
  const [copiedCell, setCopiedCell] = useState<boolean>(false);

  // Visualize modal
  const [isVisualizeModalOpen, setIsVisualizeModalOpen] = useState(false);
  const [isCreatingChart, setIsCreatingChart] = useState(false);
  const [visualizeError, setVisualizeError] = useState<string | null>(null);

  // Pagination
  const PAGE_SIZE = 100;
  const [currentPage, setCurrentPage] = useState(0);

  // Column visibility + fullscreen
  const [hiddenColumns, setHiddenColumns] = useState<Set<number>>(new Set());
  const [isColumnPickerOpen, setIsColumnPickerOpen] = useState(false);
  const [isResultsFullscreen, setIsResultsFullscreen] = useState(false);


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
  // Refs that give the Monaco completion provider access to latest React state
  // without re-registering the provider every render.
  const tablesRef = useRef<TableInfo[]>([]);
  const tableColumnsRef = useRef<Record<string, ColumnInfo[]>>({});
  const completionProviderRef = useRef<{ dispose: () => void } | null>(null);
  const runCurrentQueryRef = useRef<() => void>(() => {});
  const abortControllerRef = useRef<AbortController | null>(null);
  const applySqlFormattingRef = useRef<() => void>(() => {});
  const contentAreaRef = useRef<HTMLElement | null>(null);

  const currentDataSource = useMemo(
    () => dataSources.find((ds) => ds.id === currentDataSourceId) || null,
    [dataSources, currentDataSourceId],
  );

  const [expandedSchemas, setExpandedSchemas] = useState<Record<string, boolean>>({});

  // Keep schema refs in sync so the completion provider always sees fresh data.
  useEffect(() => { tablesRef.current = tables; }, [tables]);
  useEffect(() => { tableColumnsRef.current = tableColumns; }, [tableColumns]);
  // Dispose the completion provider when the Lab page unmounts.
  useEffect(() => () => { completionProviderRef.current?.dispose(); }, []);

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
    const key = `lens_lab_tables_v1_${userEmail || 'anon'}`;
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

  // Restore permalink query from URL (?q=base64)
  useEffect(() => {
    if (typeof window === "undefined") return;
    const param = new URLSearchParams(window.location.search).get("q");
    if (!param) return;
    try {
      const { sql, db } = JSON.parse(decodeURIComponent(escape(atob(param)))) as { sql: string; db?: string };
      if (sql) {
        const newId = `qlink${Date.now()}`;
        setQueries((prev) => [...prev, { id: newId, name: "Shared Query", text: sql }]);
        setActiveQueryId(newId);
        if (db) setCurrentDatabase(db);
        // Clean the param from URL without a page reload
        const clean = new URL(window.location.href);
        clean.searchParams.delete("q");
        window.history.replaceState({}, "", clean.toString());
      }
    } catch { /* invalid param — ignore */ }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Restore tabs from localStorage on mount
  useEffect(() => {
    if (!isAuthenticated || typeof window === 'undefined') return;
    const key = `lens_lab_tabs_v1_${account?.email || account?.username || 'anon'}`;
    try {
      const raw = localStorage.getItem(key);
      if (!raw) return;
      const { queries: saved, activeQueryId: savedActive } = JSON.parse(raw) as { queries: QueryTab[]; activeQueryId: string };
      if (Array.isArray(saved) && saved.length > 0) {
        setQueries(saved);
        setActiveQueryId(savedActive || saved[0].id);
      }
    } catch { /* ignore */ }
  }, [isAuthenticated]); // eslint-disable-line react-hooks/exhaustive-deps

  // Persist tabs to localStorage on every change
  useEffect(() => {
    if (!isAuthenticated || typeof window === 'undefined') return;
    const key = `lens_lab_tabs_v1_${account?.email || account?.username || 'anon'}`;
    try {
      localStorage.setItem(key, JSON.stringify({ queries, activeQueryId }));
    } catch { /* storage full */ }
  }, [queries, activeQueryId, isAuthenticated, account?.email, account?.username]);

  // Load query history when panel is first opened
  useEffect(() => {
    if (!isHistoryPanelOpen || !isAuthenticated || queryHistory.length > 0) return;
    const load = async () => {
      setIsLoadingHistory(true);
      try {
        const userEmail = account?.email || account?.username || null;
        const res = await msalFetch(`${API_BASE}/api/v1/lab/query-history?limit=100`, {
          headers: userEmail ? { 'x-user-email': userEmail } : undefined,
        });
        if (res.ok) {
          const data = await res.json();
          setQueryHistory(Array.isArray(data) ? data : []);
        }
      } catch { /* silent */ } finally {
        setIsLoadingHistory(false);
      }
    };
    void load();
  }, [isHistoryPanelOpen, isAuthenticated]); // eslint-disable-line react-hooks/exhaustive-deps

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
    const matched = tables.filter((t) => {
      if (
        t.name.toLowerCase().includes(term) ||
        t.schema.toLowerCase().includes(term) ||
        (t.fullName && t.fullName.toLowerCase().includes(term))
      ) return true;
      // Also match against any already-loaded columns for this table
      const cols = tableColumns[t.id];
      return cols?.some((c) => c.name.toLowerCase().includes(term) || c.dataType.toLowerCase().includes(term));
    });
    setFilteredTables(matched);
    // Auto-expand tables whose match was in a column (not the table name itself)
    setExpandedTables((prev) => {
      const next = { ...prev };
      for (const t of matched) {
        const nameMatch =
          t.name.toLowerCase().includes(term) ||
          t.schema.toLowerCase().includes(term) ||
          (t.fullName && t.fullName.toLowerCase().includes(term));
        if (!nameMatch && tableColumns[t.id]) {
          next[t.id] = true;
        }
      }
      return next;
    });
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

    // Append to the active tab rather than overwriting it.
    const editor = editorRef.current;
    const existing = editor?.getValue() ?? "";
    const snippet = `-- Preview: ${qualified}\n${sql}`;
    const appended = existing.trimEnd()
      ? `${existing.trimEnd()}\n\n${snippet}`
      : snippet;

    setQueries((prev) =>
      prev.map((q) => (q.id === activeQueryId ? { ...q, text: appended } : q)),
    );
    editor?.setValue(appended);

    setResults(null);
    await executeQuery(sql);
  };

  const toggleTableColumns = async (table: TableInfo) => {
    const tableId = table.id;

    setExpandedTables((prev) => ({
      ...prev,
      [tableId]: !prev[tableId],
    }));

    // If we're collapsing, clear its search term and bail
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
      setMultiResults(null);
      setEstimatedRows(null);
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
      abortControllerRef.current = new AbortController();
      const res = await msalFetch(`${API_BASE}/api/v1/lab/query`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(userEmail ? { 'x-user-email': userEmail } : {}),
        },
        signal: abortControllerRef.current.signal,
        body: JSON.stringify({
          query: text,
          database: currentDatabase,
          savedQueryId: active?.savedQueryId ?? null,
          executedBy: userEmail,
          ...(rowLimit > 0 ? { row_limit: rowLimit } : {}),
        }),
      });
      const responseText = await res.text();
      const data = JSON.parse(responseText, (key, value) => {
        if (typeof value === 'string' && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}/.test(value)) {
          return value;
        }
        return value;
      });

      if (!res.ok || !data.success) {
        throw new Error(data.error || "Query execution failed");
      }

      const allRows: unknown[][] = data.rows || [];
      const limitedRows = rowLimit > 0 ? allRows.slice(0, rowLimit) : allRows;
      setResults({
        columns: data.columns || [],
        rows: limitedRows,
        executionTime: data.executionTime,
        rowCount: data.rowCount,
      });
      setColumnWidths([]);
      setCurrentPage(0);
      setHiddenColumns(new Set());
    } catch (e: unknown) {
      if (e instanceof Error && e.name === "AbortError") {
        // User cancelled — clear state silently
        setResults(null);
      } else {
        setResultError(e instanceof Error ? e.message : "Unknown error");
        setResults(null);
      }
    } finally {
      setIsExecuting(false);
      abortControllerRef.current = null;
      if (executionTimerRef.current !== null) {
        window.clearInterval(executionTimerRef.current);
        executionTimerRef.current = null;
      }
      setLiveElapsedMs(null);
    }
  };

  const cancelQuery = () => {
    abortControllerRef.current?.abort();
  };

  const executeMultipleStatements = async (statements: string[]) => {
    try {
      setIsExecuting(true);
      setResultError(null);
      setResults(null);
      setMultiResults(null);
      setEstimatedRows(null);
      setSortColumnIndex(null);
      setSortDirection("asc");

      const start = performance.now();
      setLiveElapsedMs(0);
      if (executionTimerRef.current !== null) window.clearInterval(executionTimerRef.current);
      executionTimerRef.current = window.setInterval(() => setLiveElapsedMs(performance.now() - start), 100);

      const userEmail = account?.email || account?.username || null;
      const batch: Array<{ sql: string; result?: QueryResult; error?: string }> = [];

      for (const stmt of statements) {
        try {
          const res = await msalFetch(`${API_BASE}/api/v1/lab/query`, {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              ...(userEmail ? { "x-user-email": userEmail } : {}),
            },
            body: JSON.stringify({
              query: stmt,
              database: currentDatabase,
              executedBy: userEmail,
              ...(rowLimit > 0 ? { row_limit: rowLimit } : {}),
            }),
          });
          const data = await res.json();
          if (!res.ok || !data.success) {
            batch.push({ sql: stmt, error: data.error || "Query failed" });
          } else {
            const rows: unknown[][] = data.rows || [];
            batch.push({
              sql: stmt,
              result: {
                columns: data.columns || [],
                rows: rowLimit > 0 ? rows.slice(0, rowLimit) : rows,
                executionTime: data.executionTime,
                rowCount: data.rowCount,
              },
            });
          }
        } catch (e) {
          batch.push({ sql: stmt, error: e instanceof Error ? e.message : "Unknown error" });
        }
      }

      setMultiResults(batch);
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

  const handleSidebarResizeMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = sidebarWidth;
    const onMove = (ev: MouseEvent) => {
      setSidebarWidth(Math.max(240, Math.min(600, startWidth + ev.clientX - startX)));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const handlePanelResizeMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = editorHeightPx ?? (contentAreaRef.current ? contentAreaRef.current.clientHeight * 0.46 : 300);
    const onMove = (ev: MouseEvent) => {
      const total = contentAreaRef.current?.clientHeight ?? 700;
      setEditorHeightPx(Math.max(120, Math.min(total - 120, startHeight + ev.clientY - startY)));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const estimateRowCount = async () => {
    const active = getActiveQuery();
    const text = (active?.text ?? "").trim();
    if (!text || !currentDatabase) return;
    setIsEstimating(true);
    setEstimatedRows(null);
    try {
      const userEmail = account?.email || account?.username || null;
      // Strip trailing ORDER BY — T-SQL rejects ORDER BY in a subquery without TOP/OFFSET.
      const stripped = text.replace(/\bORDER\s+BY\b[\s\S]*$/i, "").trim();
      const countSql = `SELECT COUNT_BIG(*) AS estimated_rows FROM (\n${stripped}\n) AS _lens_estimate`;
      // Use /lab/execute (not /lab/query) so estimates never appear in query history.
      const res = await msalFetch(`${API_BASE}/api/v1/lab/execute`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(userEmail ? { "x-user-email": userEmail } : {}),
        },
        body: JSON.stringify({ sql: countSql, database: currentDatabase }),
      });
      const data = await res.json();
      if (data.rowCount != null || data.rows?.[0]?.[0] != null) {
        const val = data.rows?.[0]?.[0] ?? data.rowCount;
        setEstimatedRows(Number(val));
      }
    } catch {
      // Estimate is best-effort — don't surface errors
    } finally {
      setIsEstimating(false);
    }
  };

  const exportCsv = () => {
    if (!results) return;
    const escape = (v: unknown) => {
      const s = v == null ? "" : String(v);
      return s.includes(",") || s.includes('"') || s.includes("\n")
        ? `"${s.replace(/"/g, '""')}"`
        : s;
    };
    const lines = [
      results.columns.map(escape).join(","),
      ...results.rows.map((row) => (row as unknown[]).map(escape).join(",")),
    ];
    const blob = new Blob([lines.join("\r\n")], { type: "text/csv;charset=utf-8;" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `query-results-${Date.now()}.csv`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const exportJson = () => {
    if (!results) return;
    const rows = results.rows.map((row) =>
      Object.fromEntries(results.columns.map((col, i) => [col, (row as unknown[])[i]]))
    );
    const blob = new Blob([JSON.stringify(rows, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `query-results-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const exportExcel = () => {
    if (!results) return;
    const esc = (v: unknown) => String(v ?? "").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const html = `<table><tr>${results.columns.map((c) => `<th>${esc(c)}</th>`).join("")}</tr>${results.rows.map((row) => `<tr>${(row as unknown[]).map((cell) => `<td>${esc(cell)}</td>`).join("")}</tr>`).join("")}</table>`;
    const blob = new Blob([html], { type: "application/vnd.ms-excel;charset=utf-8;" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `query-results-${Date.now()}.xls`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const copyPermalink = () => {
    const sql = getActiveQuery()?.text ?? "";
    if (!sql.trim() || typeof window === "undefined") return;
    const encoded = btoa(unescape(encodeURIComponent(JSON.stringify({ sql, db: currentDatabase }))));
    const url = `${window.location.origin}/lab?q=${encoded}`;
    void navigator.clipboard.writeText(url).then(() => setCopiedCell(true));
    setTimeout(() => setCopiedCell(false), 1500);
  };


  const runCurrentQuery = () => {
    const editor = editorRef.current;
    let text: string;
    if (editor) {
      const model = editor.getModel();
      const selection = editor.getSelection();
      if (model && selection && !selection.isEmpty()) {
        text = model.getValueInRange(selection);
      } else {
        text = model?.getValue() ?? "";
      }
    } else {
      text = getActiveQuery()?.text ?? "";
    }

    const statements = splitStatements(text).filter(Boolean);
    if (statements.length > 1) {
      void executeMultipleStatements(statements);
    } else {
      executeQuery(text);
    }
  };

  // Keep refs current so Monaco commands always call the latest closures.
  useEffect(() => { runCurrentQueryRef.current = runCurrentQuery; });
  useEffect(() => { applySqlFormattingRef.current = applySqlFormattingToActiveQuery; });

  // ── Inline AI ──────────────────────────────────────────────────────────────
  const [showAiBar, setShowAiBar] = useState(false);
  const [aiPrompt, setAiPrompt] = useState("");
  const [aiGenerating, setAiGenerating] = useState(false);
  const [aiError, setAiError] = useState<string | null>(null);
  const aiInputRef = useRef<HTMLInputElement>(null);

  const generateSqlWithAi = async () => {
    const prompt = aiPrompt.trim();
    if (!prompt || aiGenerating) return;
    setAiGenerating(true);
    setAiError(null);
    const currentSql = editorRef.current?.getModel()?.getValue() ?? getActiveQuery()?.text ?? "";
    const userEmail = account?.email || account?.username || "";
    const ctx: Record<string, unknown> = {};
    if (currentDataSourceId) ctx.data_source_id = currentDataSourceId;
    if (currentSql.trim()) ctx.sql = currentSql.trim();
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/ai/chat`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          messages: [{ role: "user", content: `Generate a T-SQL query that: ${prompt}` }],
          context: Object.keys(ctx).length ? ctx : undefined,
        }),
      });
      if (!res.ok) {
        const d = await res.json().catch(() => ({}));
        throw new Error((d as any)?.detail?.message || `HTTP ${res.status}`);
      }
      const data = await res.json() as { reply: string };
      // Extract SQL from ```sql ... ``` block, or fallback to full reply
      const match = data.reply.match(/```sql\n?([\s\S]*?)```/i);
      const sql = (match ? match[1] : data.reply).trim();
      if (sql) {
        const editor = editorRef.current;
        if (editor) {
          editor.setValue(sql);
          editor.focus();
        }
        setQueries(prev => prev.map(q => q.id === activeQueryId ? { ...q, text: sql } : q));
      }
      setShowAiBar(false);
      setAiPrompt("");
    } catch (e) {
      setAiError(e instanceof Error ? e.message : "AI request failed");
    } finally {
      setAiGenerating(false);
    }
  };

  // Escape exits fullscreen / closes column picker / AI bar
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setIsResultsFullscreen(false);
        setIsColumnPickerOpen(false);
        setShowAiBar(false);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  useEffect(() => { if (showAiBar) setTimeout(() => aiInputRef.current?.focus(), 50); }, [showAiBar]);

  /** Commit an in-progress tab rename. */
  const commitRename = () => {
    const trimmed = renameValue.trim();
    if (trimmed && renamingTabId) {
      setQueries((prev) => prev.map((q) => q.id === renamingTabId ? { ...q, name: trimmed } : q));
    }
    setRenamingTabId(null);
  };

  /** Copy a cell value to the clipboard and show a brief toast. */
  const copyCell = (value: string) => {
    if (typeof navigator !== "undefined" && navigator.clipboard) {
      navigator.clipboard.writeText(value).catch(() => {});
    }
    setCopiedCell(true);
    setTimeout(() => setCopiedCell(false), 1200);
  };

  /**
   * Visualize: creates a virtual dataset backed by
   * the Lab SQL, creates a chart on that dataset, then navigates to the builder.
   */
  const visualizeResults = async (chartType: string) => {
    const active = getActiveQuery();
    const text = (active?.text ?? "").trim();
    if (!text || !currentDatabase) return;
    setIsCreatingChart(true);
    setVisualizeError(null);
    try {
      const userEmail = account?.email || account?.username || null;
      const headers: Record<string, string> = { "Content-Type": "application/json" };
      if (userEmail) headers["x-user-email"] = userEmail;

      // 1. Create a virtual dataset from the SQL + known columns.
      // Infer column types from the first data row to enable metric/dimension selection.
      const firstRow: any[] = results?.rows?.[0] ?? [];
      const colDefs = (results?.columns ?? []).map((col, idx) => {
        const sample = firstRow[idx];
        const isNumeric = sample !== null && sample !== undefined && sample !== "" && !isNaN(Number(sample));
        const colLower = col.toLowerCase();
        const looksLikeDateCol = ["date", "time", "created", "modified", "updated", "at"].some(k => colLower.includes(k));
        const inferredType = looksLikeDateCol ? "datetime" : isNumeric ? "decimal" : "varchar";
        return {
          column_name: col,
          table_name: "",
          data_type: inferredType,
          is_dimension: !isNumeric && !looksLikeDateCol,
          is_metric: isNumeric && !looksLikeDateCol,
        };
      });
      const displayName = (account as any)?.name || (account?.username || account?.email || "").split("@")[0] || "Lab";
      const vizName = `${displayName} – ${active?.name ?? "Query"} (${new Date().toLocaleDateString()})`;

      const dsRes = await msalFetch(`${API_BASE}/api/v1/datasets`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          name: vizName,
          sql_text: text,
          database_name: currentDatabase,
          columns: colDefs,
        }),
      });
      if (!dsRes.ok) {
        const err = await dsRes.json().catch(() => ({}));
        throw new Error((err as any).detail || `Failed to create dataset (${dsRes.status})`);
      }
      const ds = await dsRes.json();

      // 2. Create a chart with the new dataset.
      const chartRes = await msalFetch(`${API_BASE}/api/v1/charts`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          name: vizName,
          dataset_id: parseInt(ds.id, 10),
          chart_type: chartType,
        }),
      });
      if (!chartRes.ok) {
        const err = await chartRes.json().catch(() => ({}));
        throw new Error((err as any).detail || `Failed to create chart (${chartRes.status})`);
      }
      const chart = await chartRes.json();

      // 3. Navigate to the chart builder.
      router.push(`/charts/${chart.id}`);
    } catch (e) {
      setVisualizeError(e instanceof Error ? e.message : "Failed to create visualization");
      setIsCreatingChart(false);
    }
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
    const newId = `qt${Date.now()}`;
    setQueries((prev) => {
      const nextIndex = prev.length + 1;
      const newTab: QueryTab = { id: newId, name: `Query ${nextIndex}`, text: "" };
      return [...prev, newTab];
    });
    setActiveQueryId(newId);
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
  const filteredRows = resultFilter.trim()
    ? sortedRows.filter((row) =>
        (row as unknown[]).some((cell) =>
          String(cell ?? "").toLowerCase().includes(resultFilter.toLowerCase())
        )
      )
    : sortedRows;

  const totalPages = Math.max(1, Math.ceil(filteredRows.length / PAGE_SIZE));
  const safePage = Math.min(currentPage, totalPages - 1);
  const displayRows = filteredRows.slice(safePage * PAGE_SIZE, (safePage + 1) * PAGE_SIZE);

  return (
    <>
      {!isAuthenticated && (
        <p className="muted" style={{ margin: "1.5rem" }}>
          Sign in and connect to Kaveon to use SQL Lab.
        </p>
      )}

      {isAuthenticated && (
        <div className="main-content">
          {/* Sidebar */}
          <aside className="sidebar" style={{ width: sidebarWidth, minWidth: 240, maxWidth: 600 }}>
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

            <div className="sidebar-controls">
              <div className="sidebar-db-wrap">
                <i className="fas fa-database sidebar-db-icon" />
                <select
                  className="sidebar-db-select"
                  value={currentDataSourceId ?? ""}
                  disabled={isLoadingDatabases || dataSources.length === 0}
                  onChange={async (e) => {
                    const id = e.target.value ? parseInt(e.target.value) : null;
                    if (!id) return;
                    setCurrentDataSourceId(id);
                    const ds = dataSources.find((d) => d.id === id);
                    if (ds?.database_name) await switchDatabase(ds.database_name);
                  }}
                >
                  {isLoadingDatabases && <option value="">Loading…</option>}
                  {!isLoadingDatabases && dataSources.length === 0 && (
                    <option value="">No data sources</option>
                  )}
                  {dataSources.map((ds) => (
                    <option key={ds.id} value={ds.id}>{ds.name}</option>
                  ))}
                </select>
              </div>

              <div className="sidebar-search-wrap">
                <i className="fas fa-search sidebar-search-icon" />
                <input
                  type="text"
                  className="sidebar-search-input"
                  placeholder="Search tables or columns…"
                  value={tableSearch}
                  onChange={(e) => handleSearchChange(e.target.value)}
                  disabled={isLoadingDatabases || !currentDatabase}
                />
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
                      <div className="schema-tables">
                      {items.map((t) => (
                        <div key={t.id} className="schema-table-wrapper">
                          <div
                            className={
                              "table-item" + (t.id === selectedTableId ? " selected" : "")
                            }
                            draggable
                            onDragStart={(e) => {
                              const qualified = buildQualifiedName(t);
                              e.dataTransfer.setData("text/plain", `SELECT TOP 100 * FROM ${qualified};`);
                              e.dataTransfer.effectAllowed = "copy";
                            }}
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
                                  {(tableColumns[t.id] || [])
                                  .filter((col) => {
                                    const term = tableSearch.trim().toLowerCase();
                                    return !term || col.name.toLowerCase().includes(term) || col.dataType.toLowerCase().includes(term);
                                  })
                                  .map((col) => (
                                    <div
                                      key={col.name}
                                      className="column-item"
                                      draggable
                                      onDragStart={(e) => {
                                        const qualified = buildQualifiedName(t);
                                        e.dataTransfer.setData(
                                          "text/plain",
                                          `SELECT DISTINCT [${col.name}] FROM ${qualified};`,
                                        );
                                        e.dataTransfer.effectAllowed = "copy";
                                        e.stopPropagation();
                                      }}
                                      style={{ display: 'flex', alignItems: 'center', gap: '8px' }}
                                    >
                                      <span
                                        className="column-name"
                                        title={col.name}
                                        style={{
                                          width: '180px',
                                          overflow: 'hidden',
                                          textOverflow: 'ellipsis',
                                          whiteSpace: 'nowrap',
                                          display: 'inline-block',
                                          cursor: 'grab',
                                          fontWeight: 500
                                        }}
                                      >
                                        {col.name}
                                      </span>
                                      <span className="column-type" style={{ color: 'var(--text-muted)', fontSize: '12px' }}>{col.dataType}</span>
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
            {/* Query History Panel */}
            <div style={{ borderTop: "1px solid var(--border)", flexShrink: 0 }}>
              <div
                style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "0.5rem 0.75rem", cursor: "pointer", fontSize: "0.78rem", fontWeight: 600, color: "var(--text-secondary)", userSelect: "none" }}
                onClick={() => setIsHistoryPanelOpen((p) => !p)}
              >
                <span><i className="fas fa-history" style={{ marginRight: 6 }} />Query History</span>
                <i className={`fas fa-chevron-${isHistoryPanelOpen ? "up" : "down"}`} style={{ fontSize: "0.7rem", color: "var(--text-muted)" }} />
              </div>
              {isHistoryPanelOpen && (
                <div style={{ maxHeight: 220, overflowY: "auto", borderTop: "1px solid var(--border)" }}>
                  <div style={{ padding: "0.5rem" }}>
                    <div className="sidebar-search-wrap">
                      <i className="fas fa-search sidebar-search-icon" />
                      <input
                        className="sidebar-search-input"
                        placeholder="Search history…"
                        value={historySearch}
                        onChange={(e) => setHistorySearch(e.target.value)}
                      />
                    </div>
                  </div>
                  {isLoadingHistory && (
                    <div className="column-loading"><i className="fas fa-spinner fa-spin" /><span>Loading…</span></div>
                  )}
                  {!isLoadingHistory && queryHistory
                    .filter((h) => !historySearch || h.sql_text?.toLowerCase().includes(historySearch.toLowerCase()))
                    .slice(0, 50)
                    .map((h) => (
                      <div
                        key={h.id}
                        style={{ padding: "0.4rem 0.75rem", cursor: "pointer", borderBottom: "1px solid var(--border)", fontSize: "0.75rem" }}
                        onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-hover)")}
                        onMouseLeave={(e) => (e.currentTarget.style.background = "")}
                        onClick={() => {
                          const newId = `hist${Date.now()}`;
                          setQueries((prev) => [...prev, { id: newId, name: `History #${h.id}`, text: h.sql_text }]);
                          setActiveQueryId(newId);
                        }}
                      >
                        <div style={{ fontFamily: "monospace", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", color: "var(--text-primary)" }}>
                          {h.sql_text?.slice(0, 80)}{(h.sql_text?.length ?? 0) > 80 ? "…" : ""}
                        </div>
                        <div style={{ color: "var(--text-muted)", fontSize: "0.7rem", marginTop: 2 }}>
                          {h.started_at?.slice(0, 10)} · {(h.row_count ?? 0).toLocaleString()} rows
                          {h.duration_ms != null && ` · ${h.duration_ms}ms`}
                        </div>
                      </div>
                    ))
                  }
                  {!isLoadingHistory && queryHistory.length === 0 && (
                    <div style={{ padding: "0.5rem 0.75rem", fontSize: "0.75rem", color: "var(--text-muted)" }}>No history yet.</div>
                  )}
                </div>
              )}
            </div>
          </aside>

          {/* Sidebar resize handle */}
          <div
            style={{ width: 5, cursor: "col-resize", flexShrink: 0, background: "transparent", zIndex: 10 }}
            onMouseDown={handleSidebarResizeMouseDown}
            onMouseEnter={(e) => (e.currentTarget.style.background = "var(--border)")}
            onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
          />

          {/* Main content area */}
          <section className="content-area" ref={contentAreaRef}>
            <div className="query-section" id="querySection" style={editorHeightPx ? { height: editorHeightPx, maxHeight: editorHeightPx } : undefined}>
              <div className="query-tabs" id="queryTabs">
                {queries.map((q) => (
                  <button
                    key={q.id}
                    type="button"
                    className={"tab" + (q.id === activeQueryId ? " active" : "")}
                    onClick={() => setActiveQueryId(q.id)}
                  >
                    {renamingTabId === q.id ? (
                      <input
                        autoFocus
                        className="tab-rename-input"
                        value={renameValue}
                        onChange={(e) => setRenameValue(e.target.value)}
                        onBlur={commitRename}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") commitRename();
                          if (e.key === "Escape") setRenamingTabId(null);
                        }}
                        onClick={(e) => e.stopPropagation()}
                        style={{ width: 90, padding: "0 4px", fontSize: "0.8rem", border: "1px solid var(--accent)", borderRadius: 3 }}
                      />
                    ) : (
                      <span
                        className="tab-name"
                        title="Double-click to rename"
                        onDoubleClick={(e) => {
                          e.stopPropagation();
                          setRenamingTabId(q.id);
                          setRenameValue(q.name);
                        }}
                      >
                        {q.name}
                      </span>
                    )}
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

              <div
                className="query-editor-container"
                onDragOver={(e) => e.preventDefault()}
                onDrop={(e) => {
                  e.preventDefault();
                  const sql = e.dataTransfer.getData("text/plain");
                  if (!sql) return;
                  const editor = editorRef.current;
                  const existing = editor?.getValue() ?? "";
                  const appended = existing.trimEnd()
                    ? `${existing.trimEnd()}\n\n${sql}`
                    : sql;
                  setQueries((prev) =>
                    prev.map((q) => (q.id === activeQueryId ? { ...q, text: appended } : q)),
                  );
                  editor?.setValue(appended);
                }}
              >
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
                  onMount={(editorInstance, monacoInstance) => {
                    editorRef.current = editorInstance;
                    // Ctrl/Cmd+Enter → run query
                    editorInstance.addCommand(
                      monacoInstance.KeyMod.CtrlCmd | monacoInstance.KeyCode.Enter,
                      () => { runCurrentQueryRef.current(); },
                    );
                    // Ctrl/Cmd+Shift+F → format SQL
                    editorInstance.addCommand(
                      monacoInstance.KeyMod.CtrlCmd | monacoInstance.KeyMod.Shift | monacoInstance.KeyCode.KeyF,
                      () => { applySqlFormattingRef.current(); },
                    );
                    // Register schema-aware SQL completion provider.
                    // Dispose any previous registration first so we don't stack providers.
                    completionProviderRef.current?.dispose();
                    completionProviderRef.current = monacoInstance.languages.registerCompletionItemProvider("sql", {
                      triggerCharacters: [".", "["],
                      provideCompletionItems(_model, position) {
                        const word = _model.getWordUntilPosition(position);
                        // Only suggest when the user has typed at least one character
                        if (!word.word) return { suggestions: [] };
                        const range = {
                          startLineNumber: position.lineNumber,
                          endLineNumber: position.lineNumber,
                          startColumn: word.startColumn,
                          endColumn: word.endColumn,
                        };
                        const suggestions: monaco.languages.CompletionItem[] = [];
                        // Table name suggestions
                        for (const t of tablesRef.current) {
                          suggestions.push({
                            label: t.name,
                            kind: monacoInstance.languages.CompletionItemKind.Class,
                            insertText: t.schema ? `[${t.schema}].[${t.name}]` : `[${t.name}]`,
                            detail: `Table — ${t.schema}.${t.name}`,
                            sortText: `0_${t.name}`,
                            range,
                          });
                        }
                        // Column suggestions from all loaded schemas (deduplicated by name)
                        const seen = new Set<string>();
                        for (const cols of Object.values(tableColumnsRef.current)) {
                          for (const col of cols) {
                            if (!seen.has(col.name)) {
                              seen.add(col.name);
                              suggestions.push({
                                label: col.name,
                                kind: monacoInstance.languages.CompletionItemKind.Field,
                                insertText: `[${col.name}]`,
                                detail: col.dataType,
                                sortText: `1_${col.name}`,
                                range,
                              });
                            }
                          }
                        }
                        return { suggestions };
                      },
                    });
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

              <div className="query-templates" style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                {/* ── Execute group ── */}
                <button
                  type="button"
                  className="execute-btn"
                  style={{ padding: "0.35rem 0.9rem", fontSize: "0.8rem" }}
                  onClick={runCurrentQuery}
                  disabled={isExecuting || isEstimating}
                  title="Run query (Ctrl+Enter)"
                >
                  <i className="fas fa-play" /> Run
                </button>
                {isExecuting && (
                  <button
                    type="button"
                    className="template-btn"
                    style={{ padding: "0.35rem 0.7rem", fontSize: "0.8rem", color: "var(--error)" }}
                    onClick={cancelQuery}
                    title="Cancel running query"
                  >
                    <i className="fas fa-stop" /> Cancel
                  </button>
                )}
                <select
                  value={rowLimit}
                  onChange={(e) => setRowLimit(Number(e.target.value))}
                  className="template-btn"
                  style={{ padding: "0.35rem 0.4rem", fontSize: "0.78rem", cursor: "pointer" }}
                  title="Row limit"
                >
                  <option value={100}>100 rows</option>
                  <option value={1000}>1 000 rows</option>
                  <option value={5000}>5 000 rows</option>
                  <option value={0}>All rows</option>
                </select>

                {/* ── Templates group (centre) ── */}
                <span style={{ width: 1, background: "var(--border)", alignSelf: "stretch", margin: "4px 4px" }} />
                <span style={{ fontSize: "0.72rem", color: "var(--text-muted)", whiteSpace: "nowrap" }}>Templates</span>
                <button type="button" className="template-btn" style={{ fontSize: "0.78rem" }} onClick={() => applyTemplate("sample")} title="SELECT TOP 100 * template">Sample</button>
                <button type="button" className="template-btn" style={{ fontSize: "0.78rem" }} onClick={() => applyTemplate("rowcount")} title="COUNT(*) template">Count</button>
                <button type="button" className="template-btn" style={{ fontSize: "0.78rem" }} onClick={() => applyTemplate("schema")} title="Schema info template">Schema</button>

                {/* ── Utility group (right) ── */}
                <div style={{ flex: 1 }} />
                <button
                  type="button"
                  className="template-btn"
                  style={{ fontSize: "0.78rem", color: "var(--text-secondary)" }}
                  onClick={() => void estimateRowCount()}
                  disabled={isExecuting || isEstimating || !currentDatabase}
                  title="Estimate row count"
                >
                  {isEstimating
                    ? <><i className="fas fa-spinner fa-spin" /> Estimating…</>
                    : estimatedRows != null
                    ? <><i className="fas fa-calculator" /> ~{estimatedRows.toLocaleString()}</>
                    : <><i className="fas fa-calculator" /> Estimate</>}
                </button>
                <span style={{ width: 1, background: "var(--border)", alignSelf: "stretch", margin: "4px 2px" }} />
                <button
                  type="button"
                  className="format-btn"
                  onClick={applySqlFormattingToActiveQuery}
                  title="Format SQL (Ctrl+Shift+F)"
                  style={{ fontSize: "0.78rem" }}
                >
                  <i className="fas fa-magic" /> Format
                </button>
                <button
                  type="button"
                  className="save-btn"
                  style={{ padding: "0.35rem 0.8rem", fontSize: "0.78rem" }}
                  onClick={openSaveModal}
                  title="Save query"
                >
                  <i className="fas fa-save" /> Save
                </button>
                <span style={{ width: 1, background: "var(--border)", alignSelf: "stretch", margin: "4px 2px" }} />
                <button
                  type="button"
                  className="template-btn"
                  onClick={() => { setShowAiBar(v => !v); setAiError(null); }}
                  title="Generate SQL with AI"
                  style={{
                    fontSize: "0.78rem",
                    color: showAiBar ? "#7c3aed" : "var(--text-secondary)",
                    background: showAiBar ? "#f5f3ff" : undefined,
                    borderColor: showAiBar ? "#ddd6fe" : undefined,
                    fontWeight: showAiBar ? 600 : undefined,
                  }}
                >
                  <i className="fas fa-wand-magic-sparkles" style={{ marginRight: 3 }} />AI
                </button>
              </div>

              {/* ── Inline AI bar ── */}
              {showAiBar && (
                <div style={{
                  display: "flex", alignItems: "center", gap: 6,
                  padding: "0.45rem 0.75rem",
                  background: "var(--bg-elevated)",
                  borderTop: "1px solid var(--border)",
                  flexShrink: 0,
                }}>
                  <i className="fas fa-magic" style={{ color: "#7c3aed", fontSize: 12, flexShrink: 0 }} />
                  <input
                    ref={aiInputRef}
                    type="text"
                    value={aiPrompt}
                    onChange={e => setAiPrompt(e.target.value)}
                    onKeyDown={e => { if (e.key === "Enter") void generateSqlWithAi(); }}
                    placeholder="Describe the query you need… e.g. top 10 customers by revenue last month"
                    style={{
                      flex: 1, border: "1px solid var(--border)", borderRadius: 7,
                      padding: "0.3rem 0.65rem", fontSize: "0.8rem",
                      color: "var(--text-primary)", background: "var(--bg-surface)", outline: "none",
                      minWidth: 0,
                    }}
                    onFocus={e => { e.currentTarget.style.borderColor = "#7c3aed"; e.currentTarget.style.boxShadow = "0 0 0 2px #ede9fe"; }}
                    onBlur={e => { e.currentTarget.style.borderColor = "#ddd6fe"; e.currentTarget.style.boxShadow = "none"; }}
                  />
                  <button
                    type="button"
                    onClick={() => void generateSqlWithAi()}
                    disabled={!aiPrompt.trim() || aiGenerating}
                    style={{
                      padding: "0.3rem 0.75rem", borderRadius: 7, fontSize: "0.8rem", fontWeight: 600,
                      background: aiPrompt.trim() && !aiGenerating ? "#7c3aed" : "#e9d5ff",
                      color: aiPrompt.trim() && !aiGenerating ? "white" : "#a78bfa",
                      border: "none", cursor: aiPrompt.trim() && !aiGenerating ? "pointer" : "not-allowed",
                      flexShrink: 0, whiteSpace: "nowrap",
                    }}
                  >
                    {aiGenerating ? <><i className="fas fa-spinner fa-spin" style={{ marginRight: 4 }} />Generating…</> : <>Generate SQL</>}
                  </button>
                  {aiError && <span style={{ fontSize: "0.75rem", color: "var(--error)", flexShrink: 0 }}><i className="fas fa-exclamation-circle" style={{ marginRight: 3 }} />{aiError}</span>}
                  <button
                    type="button"
                    onClick={() => { setShowAiBar(false); setAiPrompt(""); setAiError(null); }}
                    style={{ background: "none", border: "none", color: "var(--text-muted)", cursor: "pointer", padding: "0.2rem", fontSize: 14, flexShrink: 0 }}
                    title="Close"
                  >
                    <i className="fas fa-times" />
                  </button>
                </div>
              )}
            </div>

            {/* Vertical resize handle between editor and results */}
            <div
              style={{ height: 5, cursor: "row-resize", flexShrink: 0, background: "transparent" }}
              onMouseDown={handlePanelResizeMouseDown}
              onMouseEnter={(e) => (e.currentTarget.style.background = "var(--border)")}
              onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
            />

            <div className={`results-section${isResultsFullscreen ? " results-section--fullscreen" : ""}`}>
              <div className="section-header">
                <h3 id="resultsTitle">
                  <i className="fas fa-table" /> Query Results
                </h3>
                <div className="results-actions">
                  {/* Filter + stats */}
                  {(results || multiResults) && !isExecuting && (
                    <input
                      type="text"
                      placeholder="Filter rows…"
                      value={resultFilter}
                      onChange={(e) => { setResultFilter(e.target.value); setCurrentPage(0); }}
                      style={{ padding: "0.2rem 0.5rem", fontSize: "0.75rem", border: "1px solid var(--border)", borderRadius: 4, width: 130 }}
                    />
                  )}
                  <span id="resultStats" className="result-stats">
                    {isExecuting && (
                      <><i className="fas fa-spinner fa-spin" style={{ marginRight: "0.4rem" }} />{`Running • ${formatExecutionTime((liveElapsedMs ?? 0) / 1000)}`}</>
                    )}
                    {!isExecuting && rowCount > 0 && `${rowCount.toLocaleString()} rows • ${formatExecutionTime(executionTime)}`}
                  </span>

                  {results && !isExecuting && (
                    <>
                      {/* Analyse */}
                      <button type="button" className="template-btn" style={{ padding: "0.25rem 0.6rem", fontSize: "0.75rem" }} onClick={() => { setVisualizeError(null); setIsVisualizeModalOpen(true); }} title="Create a chart from this query">
                        <i className="fas fa-chart-bar" /> Visualize
                      </button>

                      {/* Export */}
                      <button type="button" className="template-btn" style={{ padding: "0.25rem 0.6rem", fontSize: "0.75rem" }} onClick={exportCsv} title="Download as CSV"><i className="fas fa-file-csv" /> CSV</button>
                      <button type="button" className="template-btn" style={{ padding: "0.25rem 0.6rem", fontSize: "0.75rem" }} onClick={exportExcel} title="Download as Excel"><i className="fas fa-file-excel" /> Excel</button>
                      <button type="button" className="template-btn" style={{ padding: "0.25rem 0.6rem", fontSize: "0.75rem" }} onClick={exportJson} title="Download as JSON"><i className="fas fa-file-code" /> JSON</button>

                      {/* Persist + share */}

                      <button type="button" className="template-btn" style={{ padding: "0.25rem 0.6rem", fontSize: "0.75rem" }} onClick={copyPermalink} title="Copy shareable link">
                        <i className="fas fa-link" /> Share
                      </button>

                      {/* View controls */}
                      <span style={{ width: 1, background: "var(--border)", alignSelf: "stretch", margin: "4px 2px" }} />
                      <div style={{ position: "relative" }}>
                        <button type="button" className="template-btn" style={{ padding: "0.25rem 0.6rem", fontSize: "0.75rem" }} onClick={() => setIsColumnPickerOpen((p) => !p)} title="Show/hide columns">
                          <i className="fas fa-columns" /> Columns
                          {hiddenColumns.size > 0 && (
                            <span style={{ marginLeft: 4, background: "var(--accent)", color: "#fff", borderRadius: 9, padding: "0 5px", fontSize: "0.7rem" }}>{hiddenColumns.size}</span>
                          )}
                        </button>
                        {isColumnPickerOpen && (
                          <>
                            <div style={{ position: "fixed", inset: 0, zIndex: 199 }} onClick={() => setIsColumnPickerOpen(false)} />
                            <div style={{ position: "absolute", top: "110%", right: 0, zIndex: 200, background: "var(--bg-surface)", border: "1px solid var(--border)", borderRadius: 8, boxShadow: "var(--shadow-md)", minWidth: 180, maxHeight: 280, overflowY: "auto", padding: "0.4rem 0" }}>
                              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "0.3rem 0.75rem", borderBottom: "1px solid var(--border)", marginBottom: "0.25rem" }}>
                                <span style={{ fontSize: "0.7rem", fontWeight: 600, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.05em" }}>Columns</span>
                                <div style={{ display: "flex", gap: 6 }}>
                                  <button type="button" style={{ fontSize: "0.7rem", color: "var(--accent)", background: "none", border: "none", cursor: "pointer", padding: 0 }} onClick={() => setHiddenColumns(new Set())}>All</button>
                                  <button type="button" style={{ fontSize: "0.7rem", color: "var(--text-secondary)", background: "none", border: "none", cursor: "pointer", padding: 0 }} onClick={() => setHiddenColumns(new Set(results.columns.map((_, i) => i)))}>None</button>
                                </div>
                              </div>
                              {results.columns.map((col, idx) => (
                                <label key={idx} style={{ display: "flex", alignItems: "center", gap: 8, padding: "0.3rem 0.75rem", cursor: "pointer", fontSize: "0.8rem", color: "var(--text-primary)" }}
                                  onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-hover)")}
                                  onMouseLeave={(e) => (e.currentTarget.style.background = "")}
                                >
                                  <input type="checkbox" checked={!hiddenColumns.has(idx)} onChange={() => setHiddenColumns((prev) => { const next = new Set(prev); next.has(idx) ? next.delete(idx) : next.add(idx); return next; })} style={{ accentColor: "var(--accent)" }} />
                                  {col}
                                </label>
                              ))}
                            </div>
                          </>
                        )}
                      </div>

                      {/* Expand/compress — second to last */}
                      <button type="button" className="template-btn" style={{ padding: "0.25rem 0.5rem", fontSize: "0.75rem" }} onClick={() => setIsResultsFullscreen((p) => !p)} title={isResultsFullscreen ? "Exit fullscreen (Esc)" : "Expand fullscreen"}>
                        <i className={`fas fa-${isResultsFullscreen ? "compress-alt" : "expand-alt"}`} />
                      </button>
                    </>
                  )}

                  {/* Clear — always last */}
                  {(results || multiResults || resultError) && !isExecuting && !isResultsFullscreen && (
                    <button type="button" className="template-btn" style={{ padding: "0.25rem 0.5rem", fontSize: "0.75rem", color: "var(--text-secondary)" }} onClick={() => { setResults(null); setMultiResults(null); setResultError(null); setResultFilter(""); }} title="Clear results">
                      <i className="fas fa-times" />
                    </button>
                  )}
                </div>
              </div>

              <div id="resultsContainer" className="results-container">
                {/* ── Multiple-statement results ── */}
                {multiResults && !isExecuting && (
                  <div>
                    {multiResults.map((item, idx) => (
                      <div key={idx} style={{ marginBottom: "1rem", border: "1px solid var(--border)", borderRadius: 4 }}>
                        <div style={{ padding: "0.4rem 0.8rem", background: "var(--bg-primary)", borderBottom: "1px solid var(--border)", fontSize: "0.75rem", fontFamily: "monospace", color: "var(--text-secondary)" }}>
                          Statement {idx + 1}: {item.sql.length > 100 ? `${item.sql.slice(0, 100)}…` : item.sql}
                        </div>
                        {item.error ? (
                          <div style={{ padding: "0.8rem", color: "var(--error)", fontSize: "0.85rem" }}>
                            <i className="fas fa-exclamation-triangle" style={{ marginRight: "0.4rem" }} />
                            {item.error}
                          </div>
                        ) : item.result ? (
                          <div>
                            <div style={{ padding: "0.3rem 0.8rem", fontSize: "0.75rem", color: "var(--text-secondary)" }}>
                              {(item.result.rowCount ?? item.result.rows.length).toLocaleString()} rows
                              {item.result.executionTime != null && ` · ${formatExecutionTime(item.result.executionTime)}`}
                            </div>
                            <div style={{ overflowX: "auto", maxHeight: 280 }}>
                              <table className="results-table">
                                <thead>
                                  <tr>{item.result.columns.map((col) => <th key={col}>{col}</th>)}</tr>
                                </thead>
                                <tbody>
                                  {item.result.rows.map((row, rIdx) => (
                                    <tr key={rIdx}>
                                      {(row as unknown[]).map((cell, cIdx) => (
                                        <td key={cIdx}>{cell == null ? "" : String(cell)}</td>
                                      ))}
                                    </tr>
                                  ))}
                                </tbody>
                              </table>
                            </div>
                          </div>
                        ) : null}
                      </div>
                    ))}
                  </div>
                )}

                {/* ── Single-statement results ── */}
                {!multiResults && !results && !resultError && (
                  <div className="empty-state">
                    <i className="fas fa-chart-bar" />
                    <h3>Ready for Analysis</h3>
                    <p>
                      Select a table from the left sidebar or write a custom SQL query to get started.
                    </p>
                  </div>
                )}

                {!multiResults && resultError && (
                  <div className="empty-state" style={{ color: "var(--error)" }}>
                    <i className="fas fa-exclamation-triangle" />
                    <h3>Error</h3>
                    <p>{resultError}</p>
                  </div>
                )}

                {!multiResults && results && !resultError && filteredRows.length > PAGE_SIZE && (
                  <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", padding: "0.3rem 0.75rem", borderBottom: "1px solid var(--border)", fontSize: "0.75rem", color: "var(--text-secondary)" }}>
                    <button
                      type="button"
                      className="template-btn"
                      style={{ padding: "0.15rem 0.5rem", fontSize: "0.72rem" }}
                      disabled={safePage === 0}
                      onClick={() => setCurrentPage((p) => Math.max(0, p - 1))}
                    >
                      <i className="fas fa-chevron-left" />
                    </button>
                    <span>
                      {(safePage * PAGE_SIZE + 1).toLocaleString()}–{Math.min((safePage + 1) * PAGE_SIZE, filteredRows.length).toLocaleString()} of {filteredRows.length.toLocaleString()} rows
                    </span>
                    <button
                      type="button"
                      className="template-btn"
                      style={{ padding: "0.15rem 0.5rem", fontSize: "0.72rem" }}
                      disabled={safePage >= totalPages - 1}
                      onClick={() => setCurrentPage((p) => Math.min(totalPages - 1, p + 1))}
                    >
                      <i className="fas fa-chevron-right" />
                    </button>
                  </div>
                )}

                {!multiResults && results && !resultError && (
                  <div className="results-table-container">
                    <table className={`results-table${hiddenColumns.size > 0 ? " results-table--columns-hidden" : ""}`}>
                      <thead>
                        <tr>
                          {results.columns.map((col, colIndex) => {
                            if (hiddenColumns.has(colIndex)) return null;
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
                        {displayRows.map((row, idx) => (
                          <tr key={idx}>
                            {(row as unknown[]).map((cell, cIdx) => {
                              if (hiddenColumns.has(cIdx)) return null;
                              const isNull = cell == null;
                              const stringValue = isNull ? "" : cell instanceof Date ? (cell as Date).toISOString() : String(cell);
                              const isNumeric = !isNull && !Number.isNaN(Number.parseFloat(stringValue)) && stringValue.trim() !== "";
                              const explicitWidth = columnWidths[cIdx];

                              // eslint-disable-next-line react/no-array-index-key
                              return (
                                <td
                                  key={cIdx}
                                  className={isNumeric ? "numeric-cell" : undefined}
                                  style={{
                                    ...(explicitWidth ? { width: explicitWidth, minWidth: explicitWidth } : {}),
                                    cursor: "copy",
                                  }}
                                  title="Click to copy"
                                  onClick={() => copyCell(isNull ? "NULL" : stringValue)}
                                >
                                  {isNull
                                    ? <span style={{ color: "var(--text-muted)", fontStyle: "italic", fontSize: "0.85em" }}>NULL</span>
                                    : stringValue}
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
                      {typeof sessionStorage !== "undefined" && sessionStorage.getItem("lens_setup_ok") !== "1"
                        ? "Metadata database not configured"
                        : "Connection problem"}
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
                    {typeof sessionStorage !== "undefined" && sessionStorage.getItem("lens_setup_ok") !== "1" ? (
                      <p style={{ marginBottom: 8 }}>
                        Kaveon needs a metadata database to load data sources.{" "}
                        <a href="/settings/system" style={{ color: "var(--accent)", textDecoration: "underline" }}>
                          Go to System Settings
                        </a>{" "}
                        to configure it.
                      </p>
                    ) : (
                      <>
                        <p style={{ marginBottom: 8 }}>
                          We couldn&apos;t connect to the primary database for Lab.
                        </p>
                        <p className="save-query-modal-error">{loadError}</p>
                        <p style={{ fontSize: "0.8rem", color: "var(--text-secondary)", marginTop: 8 }}>
                          You can try again from the header refresh button or adjust the
                          data source connection settings.
                        </p>
                      </>
                    )}
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

      {/* ── Visualize modal ── */}

      {isAuthenticated && isVisualizeModalOpen && (
        <div className="save-query-modal-overlay">
          <div className="save-query-modal" role="dialog" aria-modal="true" aria-labelledby="visualize-title">
            <div className="save-query-modal-header">
              <h3 id="visualize-title" className="save-query-modal-title">
                <i className="fas fa-chart-bar" style={{ marginRight: 8 }} />Visualize results
              </h3>
              <button
                type="button"
                className="save-query-modal-close"
                onClick={() => { if (!isCreatingChart) setIsVisualizeModalOpen(false); }}
                aria-label="Close"
              >
                &times;
              </button>
            </div>
            <div className="save-query-modal-body">
              <p style={{ fontSize: "0.85rem", color: "var(--text-secondary)", marginBottom: 12 }}>
                Kaveon will create a virtual dataset from your SQL query, then open the chart builder.
              </p>
              <p style={{ fontSize: "0.8rem", fontWeight: 600, marginBottom: 8 }}>Choose chart type:</p>
              <div style={{ display: "flex", gap: 10, flexWrap: "wrap" }}>
                {[
                  { id: "bar", label: "Bar", icon: "fa-chart-bar" },
                  { id: "line", label: "Line", icon: "fa-chart-line" },
                  { id: "pie", label: "Pie", icon: "fa-chart-pie" },
                  { id: "table", label: "Table", icon: "fa-table" },
                  { id: "area", label: "Area", icon: "fa-chart-area" },
                ].map((ct) => (
                  <button
                    key={ct.id}
                    type="button"
                    className="template-btn"
                    style={{ padding: "0.6rem 1rem", display: "flex", flexDirection: "column", alignItems: "center", gap: 6, minWidth: 70 }}
                    onClick={() => void visualizeResults(ct.id)}
                    disabled={isCreatingChart}
                  >
                    <i className={`fas ${ct.icon}`} style={{ fontSize: "1.3rem" }} />
                    <span style={{ fontSize: "0.75rem" }}>{ct.label}</span>
                  </button>
                ))}
              </div>
              {isCreatingChart && (
                <p style={{ marginTop: 12, fontSize: "0.85rem", color: "var(--accent)" }}>
                  <i className="fas fa-spinner fa-spin" style={{ marginRight: 6 }} />
                  Creating dataset and chart…
                </p>
              )}
              {visualizeError && (
                <p style={{ marginTop: 10, fontSize: "0.85rem", color: "var(--error)" }}>
                  <i className="fas fa-exclamation-triangle" style={{ marginRight: 6 }} />
                  {visualizeError}
                </p>
              )}
            </div>
          </div>
        </div>
      )}

      {/* ── Copy-cell toast ── */}
      {copiedCell && (
        <div style={{ position: "fixed", bottom: 24, right: 24, background: "var(--text-primary)", color: "var(--bg-primary)", padding: "0.4rem 0.9rem", borderRadius: 6, fontSize: "0.8rem", zIndex: 9999, pointerEvents: "none" }}>
          <i className="fas fa-check" style={{ marginRight: 6 }} />Copied
        </div>
      )}
    </>
  );
}
