"use client";

import React, { useEffect, useState, type ReactElement } from "react";

import { API_BASE } from "../../../config";
import { Button } from "../../../components/Button";
import { LoadingOverlay } from "../../../components/LoadingOverlay";
import { format as formatSql } from "sql-formatter";
import { msalFetch } from "../../../utils/msalFetch";
import { useAuth } from "../../../auth/useAuth";
import { useRouter, useSearchParams } from "next/navigation";
import { useTheme } from "../../../contexts/ThemeContext";

// using same-origin relative API calls

interface SavedQueryRow {
  id: number;
  name: string;
  description?: string | null;
  created_at?: string | null;
  modified_at?: string | null;
  created_by?: string | null;
  favorite?: boolean;
}

interface QueryHistoryRow {
  id: string | number;
  query_id?: string | number | null;
  dataset_id?: number | null;
  tables_used?: string | null;
  status: string;
  row_count?: number | null;
  duration_ms?: number | null;
  started_at: string;
  finished_at?: string | null;
  sql_text: string;
  executed_by?: string;
  user_email?: string | null;
  run_context?: string | null;
  trigger_source?: string | null;
  database_name?: string | null;
  executed_at?: string | null;
  execution_time?: number | null;
  error_message?: string | null;
  engine_query_id?: string | null;
  trace_id?: string | null;
  worker_count?: number | null;
  split_count?: number | null;
  completed_splits?: number | null;
  stage_count?: number | null;
  processed_bytes?: number | null;
  engine_details?: {
    rows_are_preview?: boolean;
    scan_metrics_complete?: boolean;
    timings?: Record<string, number | null>;
    scans?: Array<Record<string, number>>;
    stages?: Array<{
      stage_id?: number;
      state?: string;
      task_count?: number;
      completed_tasks?: number;
      elapsed_us?: number;
      tasks?: Array<{ node_id?: string }>;
    }>;
    context?: Record<string, unknown>;
  } | null;
}

interface QueryPreviewState {
  kind: "history" | "saved";
  title: string;
  sql: string;
  historyRow?: QueryHistoryRow;
  savedQueryId?: number;
}

type SortDirection = "asc" | "desc";

type SavedSortKey = "name" | "created_at" | "description";

type HistorySortKey =
  | "status"
  | "started_at"
  | "duration_ms"
  | "row_count"
  | "tables_used"
  | "source"
  | "sql_text"
  | "executed_by";

type HistoryStatusFilter = "all" | "success" | "error" | "running";

function historyStartedAt(row: QueryHistoryRow): string {
  return row.started_at || row.executed_at || "";
}

function historyDuration(row: QueryHistoryRow): number | null {
  return row.duration_ms ?? row.execution_time ?? null;
}

function historyUser(row: QueryHistoryRow): string {
  return row.executed_by || row.user_email || "Unknown user";
}

function formatDate(value?: string | null): string {
  if (!value) return "";
  // Return raw SQL date without formatting
  return value;
}

function formatDurationMs(ms?: number | null): string {
  if (ms == null) return "Unavailable";
  if (ms < 1000) return `${ms} ms`;
  const seconds = ms / 1000;
  return `${seconds.toFixed(2)} s`;
}

function formatCount(value?: number | null): string {
  return value == null ? "Unavailable" : new Intl.NumberFormat().format(value);
}

function formatBytes(value?: number | null): string {
  if (value == null) return "Unavailable";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let amount = value;
  let index = 0;
  while (amount >= 1024 && index < units.length - 1) {
    amount /= 1024;
    index += 1;
  }
  return `${amount.toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function engineSummary(row: QueryHistoryRow) {
  const details = row.engine_details;
  const stages = details?.stages || [];
  const scans = details?.scans || [];
  const workers = new Set(
    stages.flatMap((stage) => stage.tasks || []).map((task) => task.node_id).filter(Boolean),
  );
  const taskCount = stages.reduce((total, stage) => total + (stage.task_count || 0), 0);
  const completedTasks = stages.reduce((total, stage) => total + (stage.completed_tasks || 0), 0);
  const processedBytes = scans.length
    ? scans.reduce((total, scan) => total + (scan.compressed_bytes_selected || 0), 0)
    : row.processed_bytes ?? null;
  return {
    workers: workers.size || row.worker_count || null,
    stages: stages.length || row.stage_count || null,
    taskCount: stages.length ? taskCount : null,
    completedTasks: stages.length ? completedTasks : null,
    processedBytes,
  };
}

function formatTablesUsed(raw?: string | null): string {
  if (!raw) return "";
  try {
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return parsed.join(", ");
    }
  } catch {
    // fall through
  }
  return raw;
}

function renderStatus(status: string): ReactElement {
  const value = (status || "").toLowerCase();
  let variant: "success" | "error" | "default" = "default";
  if (value === "success") {
    variant = "success";
  } else if (value === "error") {
    variant = "error";
  }

  const label = value || "unknown";

  return (
    <span className={`status-pill status-pill-${variant}`}>
      <span className="status-dot" />
      <span className="status-label">{label}</span>
    </span>
  );
}

function parseRunContext(raw?: string | null): any {
  if (!raw) return null;
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

function getHistorySourceLabel(row: QueryHistoryRow): string {
  const ctx = parseRunContext(row.run_context);

  const sourceKey =
    (typeof ctx === "object" && ctx && typeof ctx.source === "string"
      ? ctx.source
      : typeof ctx === "string"
      ? ctx
      : row.trigger_source) || "";

  const normalized = sourceKey.toLowerCase();

  if (normalized === "chart-builder" || normalized === "chart_builder") {
    return "Chart builder";
  }
  if (normalized === "dataset-detail" || normalized === "dataset_detail" || normalized === "dataset-preview" || normalized === "dataset_preview") {
    return "Dataset preview";
  }
  if (normalized === "dataset-filter-values" || normalized === "filter-values") {
    return "Dataset filters";
  }
  if (normalized === "dashboard" || normalized === "dashboard-chart") {
    return "Dashboard";
  }
  if (normalized === "dashboard-filter") {
    return "Dashboard filters";
  }
  if (normalized === "chart-builder-filter") {
    return "Chart builder filters";
  }

  if (row.query_id != null) {
    return "Saved query";
  }

  return "Lab editor";
}

function compareStrings(a: string | null | undefined, b: string | null | undefined, direction: SortDirection): number {
  const va = (a || "").toLowerCase();
  const vb = (b || "").toLowerCase();
  if (va < vb) return direction === "asc" ? -1 : 1;
  if (va > vb) return direction === "asc" ? 1 : -1;
  return 0;
}

function compareNumbers(a: number | null | undefined, b: number | null | undefined, direction: SortDirection): number {
  const va = a == null ? Number.POSITIVE_INFINITY : a;
  const vb = b == null ? Number.POSITIVE_INFINITY : b;
  if (va < vb) return direction === "asc" ? -1 : 1;
  if (va > vb) return direction === "asc" ? 1 : -1;
  return 0;
}

function compareDates(a: string | null | undefined, b: string | null | undefined, direction: SortDirection): number {
  const va = a ? new Date(a).getTime() : Number.POSITIVE_INFINITY;
  const vb = b ? new Date(b).getTime() : Number.POSITIVE_INFINITY;
  if (va < vb) return direction === "asc" ? -1 : 1;
  if (va > vb) return direction === "asc" ? 1 : -1;
  return 0;
}

function getSortIcon(currentKey: string | null, currentDirection: SortDirection, column: string): string {
  if (currentKey !== column) return "↕";
  return currentDirection === "asc" ? "▲" : "▼";
}

const LabQueriesPage: React.FC = () => {
  const { isAuthenticated, account } = useAuth();
  const { primaryColor, gradientColors } = useTheme();
  const router = useRouter();
  const searchParams = useSearchParams();

  const userEmail = account?.email || account?.username || undefined;

  const [activeTab, setActiveTab] = useState<"saved" | "history">("saved");
  const [savedQueries, setSavedQueries] = useState<SavedQueryRow[]>([]);
  const [history, setHistory] = useState<QueryHistoryRow[]>([]);
  const [isLoadingSaved, setIsLoadingSaved] = useState(false);
  const [isLoadingHistory, setIsLoadingHistory] = useState(false);
  const [errorSaved, setErrorSaved] = useState<string | null>(null);
  const [errorHistory, setErrorHistory] = useState<string | null>(null);
  const [preview, setPreview] = useState<QueryPreviewState | null>(null);
  const [toastMessage, setToastMessage] = useState<string | null>(null);
  const [savedSortBy, setSavedSortBy] = useState<SavedSortKey | null>(null);
  const [savedSortDirection, setSavedSortDirection] = useState<SortDirection>("asc");
  const [historySortBy, setHistorySortBy] = useState<HistorySortKey | null>("started_at");
  const [historySortDirection, setHistorySortDirection] = useState<SortDirection>("desc");
  const [historyPageSize, setHistoryPageSize] = useState<number>(50);
  const [historyPage, setHistoryPage] = useState<number>(1);
  const [historySearch, setHistorySearch] = useState("");
  const [historyStatus, setHistoryStatus] = useState<HistoryStatusFilter>("all");

  useEffect(() => {
    if (!preview) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setPreview(null);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [preview]);

  // Respect an optional `view` query parameter so links like
  // /lab/queries?view=history can open directly to the history tab.
  useEffect(() => {
    const view = searchParams?.get('view');

    if (view === "history") {
      setActiveTab("history");
    } else if (view === "saved") {
      setActiveTab("saved");
    }
  }, [searchParams]);

  const showToast = (message: string) => {
    setToastMessage(message);
    setTimeout(() => {
      setToastMessage((current) => (current === message ? null : current));
    }, 3000);
  };

  const loadSavedQueries = () => {
    if (!isAuthenticated) return;

    setIsLoadingSaved(true);
    setErrorSaved(null);

    (async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/lab/saved-queries`);
        if (!res.ok) throw new Error("Failed to load saved queries");
        const data = (await res.json()) as SavedQueryRow[];
        setSavedQueries(data || []);
      } catch (err) {
        console.error("Error loading saved queries:", err);
        const message = err instanceof Error ? err.message : "Failed to load saved queries";
        setErrorSaved(message);
      } finally {
        setIsLoadingSaved(false);
      }
    })();
  };

  const loadQueryHistory = () => {
    if (!isAuthenticated) return;

    setIsLoadingHistory(true);
    setErrorHistory(null);

    (async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/lab/query-history?limit=1000`);
        if (!res.ok) throw new Error("Failed to load query history");
        const data = (await res.json()) as QueryHistoryRow[];
        setHistory(data || []);
        console.log(`[QueryHistory UI] Loaded ${data.length} query history entries`);
      } catch (err) {
        console.error("Error loading query history:", err);
        const message = err instanceof Error ? err.message : "Failed to load query history";
        setErrorHistory(message);
      } finally {
        setIsLoadingHistory(false);
      }
    })();
  };

  useEffect(() => {
    if (!isAuthenticated) return;
    loadSavedQueries();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isAuthenticated]);

  useEffect(() => {
    if (!isAuthenticated) return;
    loadQueryHistory();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isAuthenticated]);

  // Auto-refresh query history when switching to the history tab
  useEffect(() => {
    if (activeTab === 'history' && isAuthenticated) {
      loadQueryHistory();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab]);

  const handleOpenSavedQuery = (id: number) => {
    window.location.href = `/lab?savedQueryId=${id}`;
  };

  const handleViewSavedQuery = (id: number) => {
    handleOpenSavedQuery(id);
  };

  const handleEditSavedQuery = (id: number) => {
    handleOpenSavedQuery(id);
  };

  const handleCloneSavedQuery = async (id: number) => {
    try {
      const detailRes = await msalFetch(`${API_BASE}/api/v1/lab/saved-queries/${id}`);
      if (!detailRes.ok) {
        throw new Error("Failed to load saved query for cloning");
      }

      const detail: {
        name?: string;
        description?: string | null;
        sql?: string;
        dataset_id?: number | null;
      } = await detailRes.json();

      const cloneName = (detail.name || "Saved query").concat(" (Copy)");

      const createRes = await msalFetch(`${API_BASE}/api/v1/lab/saved-queries`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          name: cloneName,
          description: detail.description ?? null,
          sql: detail.sql || "",
          dataset_id: detail.dataset_id ?? null,
          created_by: userEmail,
        }),
      });

      if (!createRes.ok) {
        throw new Error("Failed to clone saved query");
      }

      loadSavedQueries();
    } catch (err) {
      console.error("Error cloning saved query:", err);
      const message = err instanceof Error ? err.message : "Failed to clone saved query";
      setErrorSaved(message);
    }
  };

  const handleShareSavedQuery = async (id: number) => {
    if (typeof window === "undefined") return;

    const shareUrl = `${window.location.origin}/lab?savedQueryId=${id}`;

    try {
      if (navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(shareUrl);
        showToast("Link copied to clipboard");
      } else {
        // eslint-disable-next-line no-alert
        alert(shareUrl);
      }
    } catch {
      // Fallback: show the URL so the user can copy it manually.
      // eslint-disable-next-line no-alert
      alert(shareUrl);
    }
  };

  const handleDeleteSavedQuery = async (id: number) => {
    if (typeof window !== "undefined") {
      // eslint-disable-next-line no-alert
      const confirmed = window.confirm("Delete this saved query? This cannot be undone.");
      if (!confirmed) return;
    }

    try {
      const res = await msalFetch(`${API_BASE}/api/v1/lab/saved-queries/${id}` , {
        method: "DELETE",
      });
      if (!res.ok && res.status !== 204) {
        throw new Error("Failed to delete saved query");
      }

      setSavedQueries((prev) => prev.filter((q) => q.id !== id));
    } catch (err) {
      console.error("Error deleting saved query:", err);
      const message = err instanceof Error ? err.message : "Failed to delete saved query";
      setErrorSaved(message);
    }
  };

  const handleOpenHistoryPreview = (row: QueryHistoryRow) => {
    setPreview({
      kind: "history",
      title: "Executed query",
      sql: row.sql_text,
      historyRow: row,
    });
  };

  const handleOpenSavedPreview = async (row: SavedQueryRow) => {
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/lab/saved-queries/${row.id}`);
      if (!res.ok) {
        throw new Error("Failed to load saved query");
      }

      const detail: { name?: string; sql?: string } = await res.json();

      setPreview({
        kind: "saved",
        title: detail.name || row.name,
        sql: detail.sql || "",
        savedQueryId: row.id,
      });
    } catch (err) {
      console.error("Error loading saved query for preview:", err);
      const message = err instanceof Error ? err.message : "Failed to load saved query";
      setErrorSaved(message);
    }
  };

  const handleClosePreview = () => {
    setPreview(null);
  };

  const handleCopyFromPreview = async () => {
    if (!preview) return;
    if (typeof navigator === "undefined" || !navigator.clipboard) return;

    try {
      await navigator.clipboard.writeText(preview.sql || "");
      showToast("SQL copied to clipboard");
    } catch {
      showToast("Copy failed. Please copy manually.");
    }
  };

  const handleOpenHistoryInEditor = (row: QueryHistoryRow) => {
    if (typeof window === "undefined") return;

    // If this execution is linked to a Saved query, open that directly.
    if (row.query_id) {
      window.location.href = `/lab?savedQueryId=${row.query_id}`;
      return;
    }

    const payload = {
      sql: row.sql_text,
      name: "History query",
    };

    try {
      window.localStorage.setItem("lab-pending-sql", JSON.stringify(payload));
    } catch {
      // Ignore storage errors; we'll still navigate to Lab.
    }

    window.location.href = "/lab";
  };

  const handleSavedSort = (column: SavedSortKey) => {
    if (savedSortBy === column) {
      setSavedSortDirection(savedSortDirection === "asc" ? "desc" : "asc");
    } else {
      setSavedSortBy(column);
      setSavedSortDirection("asc");
    }
  };

  const handleHistorySort = (column: HistorySortKey) => {
    if (historySortBy === column) {
      setHistorySortDirection(historySortDirection === "asc" ? "desc" : "asc");
    } else {
      setHistorySortBy(column);
      setHistorySortDirection("asc");
    }
  };

  const getSortedSavedQueries = () => {
    let items = [...savedQueries];

    if (userEmail) {
      items = items.filter((q) => q.created_by === userEmail);
    }
    if (!savedSortBy) return items;

    items.sort((a, b) => {
      switch (savedSortBy) {
        case "created_at":
          return compareDates(a.created_at, b.created_at, savedSortDirection);
        case "description":
          return compareStrings(a.description, b.description, savedSortDirection);
        case "name":
        default:
          return compareStrings(a.name, b.name, savedSortDirection);
      }
    });

    return items;
  };

  const getSortedHistory = () => {
    const needle = historySearch.trim().toLowerCase();
    const items = history.filter((row) => {
      const normalizedStatus = (row.status || "").toLowerCase();
      if (historyStatus !== "all" && normalizedStatus !== historyStatus) return false;
      if (!needle) return true;
      return [
        row.id,
        row.engine_query_id,
        row.trace_id,
        row.sql_text,
        historyUser(row),
        getHistorySourceLabel(row),
        row.database_name,
        formatTablesUsed(row.tables_used),
        row.error_message,
      ].some((value) => String(value || "").toLowerCase().includes(needle));
    });
    if (!historySortBy) return items;

    items.sort((a, b) => {
      switch (historySortBy) {
        case "started_at":
          return compareDates(historyStartedAt(a), historyStartedAt(b), historySortDirection);
        case "duration_ms":
          return compareNumbers(historyDuration(a), historyDuration(b), historySortDirection);
        case "row_count":
          return compareNumbers(a.row_count, b.row_count, historySortDirection);
        case "tables_used":
          return compareStrings(
            formatTablesUsed(a.tables_used),
            formatTablesUsed(b.tables_used),
            historySortDirection,
          );
        case "source":
          return compareStrings(
            getHistorySourceLabel(a),
            getHistorySourceLabel(b),
            historySortDirection,
          );
        case "sql_text":
          return compareStrings(a.sql_text, b.sql_text, historySortDirection);
        case "executed_by":
          return compareStrings(historyUser(a), historyUser(b), historySortDirection);
        case "status":
        default:
          return compareStrings(a.status, b.status, historySortDirection);
      }
    });

    return items;
  };

  const sortedSavedQueries = getSortedSavedQueries();
  const sortedHistory = getSortedHistory();

  const totalHistoryPages = Math.max(1, Math.ceil(sortedHistory.length / historyPageSize));
  const currentHistoryPage = Math.min(historyPage, totalHistoryPages);
  const historyStartIndex = (currentHistoryPage - 1) * historyPageSize;
  const pagedHistory = sortedHistory.slice(
    historyStartIndex,
    historyStartIndex + historyPageSize,
  );

  const formatSqlSafe = (sql: string | undefined | null): string => {
    const text = (sql || "").trim();
    if (!text) return "";
    try {
      return formatSql(text, { language: "tsql" });
    } catch {
      return sql || "";
    }
  };

  const getSqlSnippet = (sql: string | undefined | null, maxLength: number = 80): string => {
    const formatted = formatSqlSafe(sql);
    if (!formatted) return "";

    // Collapse whitespace and newlines so we only show a compact, single-line preview.
    const singleLine = formatted.replace(/\s+/g, " ").trim();
    if (singleLine.length <= maxLength) {
      return singleLine;
    }
    return `${singleLine.slice(0, maxLength)}…`;
  };

  return (
    <div className="page-shell animate-fade-in">
      <div style={{
        display: "flex", alignItems: "center", justifyContent: "space-between",
        flexWrap: "wrap", gap: "1rem",
        marginBottom: "1.25rem",
        padding: "1.25rem 1.5rem",
        background: "var(--bg-surface)",
        borderRadius: 12,
        border: "1px solid var(--border)",
        boxShadow: "var(--shadow-sm)",
      }}>
        <div style={{ display: "flex", alignItems: "center", gap: "1.25rem", flexWrap: "wrap", flex: 1, minWidth: 0 }}>
          <div style={{
            width: 48, height: 48, borderRadius: 12, flexShrink: 0,
            background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
            display: "flex", alignItems: "center", justifyContent: "center",
            boxShadow: `0 4px 14px ${primaryColor}35`,
          }}>
            <i className="fas fa-bookmark" style={{ color: "white", fontSize: 20 }} />
          </div>
          <div style={{ minWidth: 0 }}>
            <h1 style={{ margin: 0, fontSize: "1.15rem", fontWeight: 700, color: "var(--text-primary)" }}>Query Activity</h1>
            <p style={{ margin: "3px 0 0", fontSize: "0.85rem", color: "var(--text-muted)", lineHeight: 1.4 }}>
              Your saved SQL queries and execution history.
            </p>
          </div>
          {(!isLoadingSaved || !isLoadingHistory) && (
            <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
              {!isLoadingSaved && sortedSavedQueries.length > 0 && (
                <span style={{
                  display: "inline-flex", alignItems: "center", gap: "0.35rem",
                  padding: "0.3rem 0.75rem", borderRadius: 20,
                  background: `${primaryColor}12`, border: `1px solid ${primaryColor}30`,
                  fontSize: 12, fontWeight: 600, color: primaryColor, whiteSpace: "nowrap",
                }}>
                  <i className="fas fa-bookmark" style={{ fontSize: 10 }} />
                  {sortedSavedQueries.length} Saved Quer{sortedSavedQueries.length !== 1 ? "ies" : "y"}
                </span>
              )}
              {!isLoadingHistory && history.length > 0 && (
                <span style={{
                  display: "inline-flex", alignItems: "center", gap: "0.35rem",
                  padding: "0.3rem 0.75rem", borderRadius: 20,
                  background: "rgba(var(--accent-rgb), 0.08)", border: "1px solid rgba(var(--accent-rgb), 0.25)",
                  fontSize: 12, fontWeight: 600, color: "var(--accent)", whiteSpace: "nowrap",
                }}>
                  <i className="fas fa-history" style={{ fontSize: 10 }} />
                  {history.length} Total Runs
                </span>
              )}
              {!isLoadingHistory && userEmail && history.filter(h => h.executed_by === userEmail).length > 0 && (
                <span style={{
                  display: "inline-flex", alignItems: "center", gap: "0.35rem",
                  padding: "0.3rem 0.75rem", borderRadius: 20,
                  background: "var(--bg-hover)", border: "1px solid var(--border)",
                  fontSize: 12, fontWeight: 600, color: "var(--text-muted)", whiteSpace: "nowrap",
                }}>
                  <i className="fas fa-user" style={{ fontSize: 10 }} />
                  {history.filter(h => h.executed_by === userEmail).length} My Runs
                </span>
              )}
            </div>
          )}
        </div>
      </div>

      {!isAuthenticated && (
        <p className="muted">Sign in to view your saved queries and history.</p>
      )}

      {isAuthenticated && (
        <div className="card" style={{ marginTop: 12 }}>
          <div className="query-tabs">
            <button
              type="button"
              className={`tab ${activeTab === "saved" ? "active" : ""}`}
              onClick={() => setActiveTab("saved")}
            >
              <span className="tab-name">Saved queries</span>
            </button>
            <button
              type="button"
              className={`tab ${activeTab === "history" ? "active" : ""}`}
              onClick={() => setActiveTab("history")}
            >
              <span className="tab-name">Query history</span>
            </button>
          </div>

          {activeTab === "saved" && (
            <div className="results-table-container">
              {isLoadingSaved && <LoadingOverlay />}
              {!isLoadingSaved && errorSaved && (
                typeof sessionStorage !== "undefined" && sessionStorage.getItem("lens_setup_ok") !== "1"
                  ? <p className="muted">Metadata database not configured. <a href="/settings/system" style={{ color: "var(--accent)" }}>Go to System Settings</a> to set it up.</p>
                  : <p className="muted">{errorSaved}</p>
              )}
              {!isLoadingSaved && !errorSaved && sortedSavedQueries.length === 0 && (
                <p className="muted">No saved queries yet.</p>
              )}
              {!isLoadingSaved && !errorSaved && sortedSavedQueries.length > 0 && (
                <table className="results-table">
                  <thead>
                    <tr>
                      <th
                        className={savedSortBy === "name" ? "sorted" : undefined}
                        onClick={() => handleSavedSort("name")}
                      >
                        <span className="column-header-label">Name</span>
                        <span className="column-sort-icon">{getSortIcon(savedSortBy, savedSortDirection, "name")}</span>
                      </th>
                      <th
                        className={savedSortBy === "created_at" ? "sorted" : undefined}
                        onClick={() => handleSavedSort("created_at")}
                      >
                        <span className="column-header-label">Created on</span>
                        <span className="column-sort-icon">{getSortIcon(savedSortBy, savedSortDirection, "created_at")}</span>
                      </th>
                      <th>
                        <span className="column-header-label">Last modified</span>
                      </th>
                      <th>
                        <span className="column-header-label">Owner</span>
                      </th>
                      <th>
                        <span className="column-header-label">Actions</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {sortedSavedQueries.map((q) => (
                      <tr key={q.id} onClick={() => handleOpenSavedQuery(q.id)} style={{ cursor: "pointer" }}>
                        <td>
                          <strong>{q.name}</strong>
                          {q.description && (
                            <div className="muted" style={{ marginTop: 4, fontSize: 13 }}>
                              {q.description}
                            </div>
                          )}
                        </td>
                        <td className="muted" style={{ fontSize: 13 }}>
                          {formatDate(q.created_at)}
                        </td>
                        <td className="muted" style={{ fontSize: 13 }}>
                          {formatDate(q.modified_at)}
                        </td>
                        <td className="muted" style={{ fontSize: 13 }}>
                            {q.created_by || "—"}
                        </td>
                        <td className="actions-cell">
                          <div className="row-actions">
                            <button
                              type="button"
                              className="action-icon-btn"
                              title="View"
                              aria-label="View saved query"
                              onClick={(e) => {
                                e.stopPropagation();
                                void handleOpenSavedPreview(q);
                              }}
                            >
                              <i className="fas fa-eye" aria-hidden="true" />
                            </button>
                            <button
                              type="button"
                              className="action-icon-btn"
                              title="Edit"
                              aria-label="Edit saved query"
                              onClick={(e) => {
                                e.stopPropagation();
                                handleEditSavedQuery(q.id);
                              }}
                            >
                              <i className="fas fa-edit" aria-hidden="true" />
                            </button>
                            <button
                              type="button"
                              className="action-icon-btn"
                              title="Clone"
                              aria-label="Clone saved query"
                              onClick={(e) => {
                                e.stopPropagation();
                                void handleCloneSavedQuery(q.id);
                              }}
                            >
                              <i className="fas fa-copy" aria-hidden="true" />
                            </button>
                            <button
                              type="button"
                              className="action-icon-btn"
                              title="Share"
                              aria-label="Share saved query link"
                              onClick={(e) => {
                                e.stopPropagation();
                                void handleShareSavedQuery(q.id);
                              }}
                            >
                              <i className="fas fa-share-alt" aria-hidden="true" />
                            </button>
                            <button
                              type="button"
                              className="action-icon-btn"
                              title="Delete"
                              aria-label="Delete saved query"
                              onClick={(e) => {
                                e.stopPropagation();
                                void handleDeleteSavedQuery(q.id);
                              }}
                            >
                              <i className="fas fa-trash" aria-hidden="true" />
                            </button>
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          )}

          {activeTab === "history" && (
            <div className="results-table-container">
              <div className="query-history-toolbar" aria-label="Query history filters">
                <label className="query-history-search">
                  <i className="fas fa-search" aria-hidden="true" />
                  <span className="sr-only">Search query history</span>
                  <input
                    type="search"
                    value={historySearch}
                    placeholder="Search SQL, user, source, table, activity ID, or error"
                    onChange={(event) => {
                      setHistorySearch(event.target.value);
                      setHistoryPage(1);
                    }}
                  />
                </label>
                <div className="query-status-filters" role="group" aria-label="Filter by query status">
                  {(["all", "success", "error", "running"] as HistoryStatusFilter[]).map((status) => (
                    <button
                      type="button"
                      key={status}
                      className={historyStatus === status ? "active" : ""}
                      aria-pressed={historyStatus === status}
                      onClick={() => {
                        setHistoryStatus(status);
                        setHistoryPage(1);
                      }}
                    >
                      {status === "all" ? "All states" : status}
                    </button>
                  ))}
                </div>
                <button type="button" className="query-refresh-btn" onClick={loadQueryHistory} disabled={isLoadingHistory}>
                  <i className="fas fa-rotate" aria-hidden="true" /> Refresh
                </button>
              </div>
              {isLoadingHistory && <LoadingOverlay />}
              {!isLoadingHistory && errorHistory && (
                typeof sessionStorage !== "undefined" && sessionStorage.getItem("lens_setup_ok") !== "1"
                  ? <p className="muted">Metadata database not configured. <a href="/settings/system" style={{ color: "var(--accent)" }}>Go to System Settings</a> to set it up.</p>
                  : <p className="muted">{errorHistory}</p>
              )}
              {!isLoadingHistory && !errorHistory && history.length === 0 && (
                <p className="muted">No query history available yet.</p>
              )}
              {!isLoadingHistory && !errorHistory && history.length > 0 && sortedHistory.length === 0 && (
                <div className="query-history-empty">No queries match these filters.</div>
              )}
              {!isLoadingHistory && !errorHistory && sortedHistory.length > 0 && (
                <>
                  <table className="results-table">
                    <thead>
                      <tr>
                      <th
                        className={historySortBy === "status" ? "sorted" : undefined}
                        onClick={() => handleHistorySort("status")}
                      >
                        <span className="column-header-label">Status</span>
                        <span className="column-sort-icon">{getSortIcon(historySortBy, historySortDirection, "status")}</span>
                      </th>
                      <th
                        className={historySortBy === "started_at" ? "sorted" : undefined}
                        onClick={() => handleHistorySort("started_at")}
                      >
                        <span className="column-header-label">Started</span>
                        <span className="column-sort-icon">{getSortIcon(historySortBy, historySortDirection, "started_at")}</span>
                      </th>
                      <th
                        className={historySortBy === "duration_ms" ? "sorted" : undefined}
                        onClick={() => handleHistorySort("duration_ms")}
                      >
                        <span className="column-header-label">Duration</span>
                        <span className="column-sort-icon">{getSortIcon(historySortBy, historySortDirection, "duration_ms")}</span>
                      </th>
                      <th
                        className={historySortBy === "row_count" ? "sorted" : undefined}
                        onClick={() => handleHistorySort("row_count")}
                      >
                        <span className="column-header-label">Rows</span>
                        <span className="column-sort-icon">{getSortIcon(historySortBy, historySortDirection, "row_count")}</span>
                      </th>
                      <th
                        className={historySortBy === "tables_used" ? "sorted" : undefined}
                        onClick={() => handleHistorySort("tables_used")}
                      >
                        <span className="column-header-label">Tables</span>
                        <span className="column-sort-icon">{getSortIcon(historySortBy, historySortDirection, "tables_used")}</span>
                      </th>
                      <th
                        className={historySortBy === "source" ? "sorted" : undefined}
                        onClick={() => handleHistorySort("source")}
                      >
                        <span className="column-header-label">Source</span>
                        <span className="column-sort-icon">{getSortIcon(historySortBy, historySortDirection, "source")}</span>
                      </th>
                      <th
                        className={historySortBy === "sql_text" ? "sorted" : undefined}
                        onClick={() => handleHistorySort("sql_text")}
                      >
                        <span className="column-header-label">SQL query</span>
                        <span className="column-sort-icon">{getSortIcon(historySortBy, historySortDirection, "sql_text")}</span>
                      </th>
                      <th
                        className={historySortBy === "executed_by" ? "sorted" : undefined}
                        onClick={() => handleHistorySort("executed_by")}
                      >
                        <span className="column-header-label">Executed by</span>
                        <span className="column-sort-icon">{getSortIcon(historySortBy, historySortDirection, "executed_by")}</span>
                      </th>
                      <th>
                        <span className="column-header-label">Actions</span>
                      </th>
                      </tr>
                    </thead>
                    <tbody>
                      {pagedHistory.map((h) => (
                        <tr
                          key={h.id}
                          className="query-history-row"
                          tabIndex={0}
                          onClick={() => handleOpenHistoryPreview(h)}
                          onKeyDown={(event) => {
                            if (event.key === "Enter" || event.key === " ") {
                              event.preventDefault();
                              handleOpenHistoryPreview(h);
                            }
                          }}
                        >
                          <td>{renderStatus(h.status)}</td>
                          <td>{formatDate(historyStartedAt(h))}</td>
                          <td>{formatDurationMs(historyDuration(h))}</td>
                          <td className="numeric-cell">{formatCount(h.row_count)}</td>
                          <td
                            title={formatTablesUsed(h.tables_used)}
                            style={{
                              maxWidth: 200,
                              whiteSpace: "nowrap",
                              overflow: "hidden",
                              textOverflow: "ellipsis",
                            }}
                          >
                            {formatTablesUsed(h.tables_used)}
                          </td>
                          <td>{getHistorySourceLabel(h)}</td>
                          <td className="sql-snippet-cell" style={{ maxWidth: 260 }}>
                            <pre
                              className="sql-snippet sql-snippet-clickable"
                              title="Click to preview full SQL and copy"
                              style={{
                                whiteSpace: "nowrap",
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                              }}
                              onClick={(event) => { event.stopPropagation(); handleOpenHistoryPreview(h); }}
                            >
                              {getSqlSnippet(h.sql_text)}
                            </pre>
                          </td>
                          <td>{historyUser(h)}</td>
                          <td className="actions-cell">
                            <div className="row-actions">
                              <button
                                type="button"
                                className="action-icon-btn"
                                title="Open in SQL Lab"
                                aria-label="Open query in SQL Lab"
                                onClick={(event) => { event.stopPropagation(); handleOpenHistoryInEditor(h); }}
                              >
                                <i className="fas fa-external-link-alt" aria-hidden="true" />
                              </button>
                            </div>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>

                  <div
                    className="results-footer"
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                      marginTop: 8,
                      fontSize: 12,
                    }}
                  >
                    <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                      <span>Rows per page</span>
                      <select
                        value={historyPageSize}
                        onChange={(e) => {
                          const next = Number(e.target.value) || 10;
                          setHistoryPageSize(next);
                          setHistoryPage(1);
                        }}
                        style={{ padding: "2px 6px", fontSize: 12 }}
                      >
                        {[10, 25, 50, 100].map((size) => (
                          <option key={size} value={size}>
                            {size}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                      <button
                        type="button"
                        className="link-button"
                        disabled={currentHistoryPage <= 1}
                        onClick={() => setHistoryPage((p) => Math.max(1, p - 1))}
                      >
                        Previous
                      </button>
                      <span>
                        Page {currentHistoryPage} of {totalHistoryPages}
                      </span>
                      <button
                        type="button"
                        className="link-button"
                        disabled={currentHistoryPage >= totalHistoryPages}
                        onClick={() =>
                          setHistoryPage((p) =>
                            p >= totalHistoryPages ? totalHistoryPages : p + 1,
                          )
                        }
                      >
                        Next
                      </button>
                    </div>
                  </div>
                </>
              )}
            </div>
          )}
          {preview && (
            <div className="query-preview-overlay" onMouseDown={(event) => {
              if (event.target === event.currentTarget) handleClosePreview();
            }}>
              <div
                className={`query-preview-modal ${preview.kind === "history" ? "query-details-modal" : ""}`}
                role="dialog"
                aria-modal="true"
                aria-labelledby="query-preview-title"
              >
                <div className="query-preview-header">
                  <div>
                    <div className="query-preview-eyebrow">{preview.kind === "history" ? "Query details" : "Saved query"}</div>
                    <h2 className="query-preview-title" id="query-preview-title">
                      {preview.kind === "history" && preview.historyRow
                        ? preview.historyRow.engine_query_id || `Activity ${preview.historyRow.id}`
                        : preview.title || "Query preview"}
                    </h2>
                  </div>
                  <button
                    type="button"
                    className="query-preview-close"
                    aria-label="Close preview"
                    onClick={handleClosePreview}
                  >
                    ×
                  </button>
                </div>
                <div className="query-preview-body">
                  {preview.kind === "history" && preview.historyRow && (() => {
                    const row = preview.historyRow;
                    const tables = formatTablesUsed(row.tables_used);
                    const engine = engineSummary(row);
                    const splitProgress = row.split_count == null
                      ? "Unavailable"
                      : `${formatCount(row.completed_splits ?? 0)} / ${formatCount(row.split_count)}`;
                    return (
                      <div className="query-details-content">
                        <div className="query-details-hero">
                          <div>
                            {renderStatus(row.status)}
                            <div className="query-details-time">Started {formatDate(historyStartedAt(row)) || "Unavailable"}</div>
                          </div>
                          <div className="query-detail-identity">
                            <span>Executed by</span>
                            <strong>{historyUser(row)}</strong>
                          </div>
                          <div className="query-detail-identity">
                            <span>Entry point</span>
                            <strong>{getHistorySourceLabel(row)}</strong>
                          </div>
                        </div>

                        <section className="query-metric-grid" aria-label="Query metrics">
                          <div><span>Elapsed</span><strong>{formatDurationMs(historyDuration(row))}</strong></div>
                          <div><span>Returned rows</span><strong>{formatCount(row.row_count)}</strong></div>
                          <div><span>Compressed read</span><strong>{formatBytes(engine.processedBytes)}</strong></div>
                          <div><span>Workers</span><strong>{formatCount(engine.workers)}</strong></div>
                          <div><span>Splits complete</span><strong>{splitProgress}</strong></div>
                          <div><span>Stages</span><strong>{formatCount(engine.stages)}</strong></div>
                        </section>

                        {row.engine_details && (
                          <section className="query-details-section">
                            <h3>Engine execution</h3>
                            <dl className="query-details-list">
                              <div><dt>Tasks complete</dt><dd>{engine.taskCount == null ? "Unavailable" : `${formatCount(engine.completedTasks)} / ${formatCount(engine.taskCount)}`}</dd></div>
                              {Object.entries(row.engine_details.timings || {}).map(([name, microseconds]) => (
                                <div key={name}><dt>{name.replace(/_us$/, "").replaceAll("_", " ")}</dt><dd>{microseconds == null ? "Unavailable" : formatDurationMs(microseconds / 1000)}</dd></div>
                              ))}
                              <div><dt>Scan telemetry</dt><dd>{row.engine_details.scan_metrics_complete === true ? "Complete" : row.engine_details.scan_metrics_complete === false ? "Partial" : "Unavailable"}</dd></div>
                              <div><dt>Result rows</dt><dd>{row.engine_details.rows_are_preview === true ? "Preview retained" : row.engine_details.rows_are_preview === false ? "Complete" : "Unavailable"}</dd></div>
                            </dl>
                          </section>
                        )}

                        <section className="query-details-section">
                          <h3>Execution context</h3>
                          <dl className="query-details-list">
                            <div><dt>Activity ID</dt><dd className="query-mono">{String(row.id)}</dd></div>
                            <div><dt>Engine query ID</dt><dd className="query-mono">{row.engine_query_id || "Unavailable"}</dd></div>
                            <div><dt>Trace ID</dt><dd className="query-mono">{row.trace_id || "Unavailable"}</dd></div>
                            <div><dt>Database / catalog</dt><dd>{row.database_name || "Unavailable"}</dd></div>
                            <div><dt>Tables</dt><dd>{tables || "Unavailable"}</dd></div>
                            <div><dt>Saved query</dt><dd>{row.query_id == null ? "Not linked" : String(row.query_id)}</dd></div>
                            {row.engine_details?.context && Object.entries(row.engine_details.context)
                              .filter(([name, value]) => ["engine_version", "environment", "client", "catalog", "schema", "time_zone", "client_tags"].includes(name) && value != null)
                              .map(([name, value]) => (
                                <div key={name}>
                                  <dt>{name.replaceAll("_", " ")}</dt>
                                  <dd>{Array.isArray(value) ? value.join(", ") || "None" : String(value)}</dd>
                                </div>
                              ))}
                          </dl>
                        </section>

                        {row.error_message && (
                          <section className="query-error-panel" role="alert">
                            <h3><i className="fas fa-circle-exclamation" aria-hidden="true" /> Error</h3>
                            <pre>{row.error_message}</pre>
                          </section>
                        )}
                      </div>
                    );
                  })()}
                  <div className="query-sql-heading">
                    <h3>SQL</h3>
                    <span>Exact submitted statement</span>
                  </div>
                  <pre className="query-preview-sql">{formatSqlSafe(preview.sql)}</pre>
                </div>
                <div className="query-preview-footer">
                  <button
                    type="button"
                    className="query-preview-btn query-preview-btn-secondary"
                    title="Copy SQL to clipboard"
                    onClick={handleCopyFromPreview}
                  >
                    Copy
                  </button>
                  <button
                    type="button"
                    className="query-preview-btn query-preview-btn-primary"
                    title="Open in SQL Lab"
                    onClick={() => {
                      if (preview.kind === "history" && preview.historyRow) {
                        handleOpenHistoryInEditor(preview.historyRow);
                      } else if (preview.kind === "saved" && preview.savedQueryId) {
                        handleOpenSavedQuery(preview.savedQueryId);
                      }
                      handleClosePreview();
                    }}
                  >
                    Open in SQL Lab
                  </button>
                </div>
              </div>
            </div>
          )}
        </div>
      )}
      {toastMessage && (
        <div className="toast">
          <span className="toast-icon" aria-hidden="true">
            ✓
          </span>
          <span className="toast-text">{toastMessage}</span>
        </div>
      )}
    </div>
  );
};

export default LabQueriesPage;
