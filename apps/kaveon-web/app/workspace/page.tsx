"use client";

import React, { useEffect, useState, useCallback } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { API_BASE } from "../../config";
import { useAuth } from "../../auth/useAuth";
import { useSetup } from "../../components/ClientLayout";
import { msalFetch } from "../../utils/msalFetch";

// ── Types ────────────────────────────────────────────────────────────────────

type TabKey = "dashboards" | "charts" | "datasets" | "queries";

interface WorkspaceItem {
  id: string | number;
  name?: string;
  title?: string;
  created_by?: string | null;
  updated_at?: string | null;
  created_at?: string | null;
}

// ── Tab config ───────────────────────────────────────────────────────────────

const TABS: { key: TabKey; label: string; endpoint: string; newRoute: string }[] = [
  { key: "dashboards", label: "Dashboards",    endpoint: "/api/v1/dashboards",    newRoute: "/dashboards/new" },
  { key: "charts",     label: "Charts",        endpoint: "/api/v1/charts",        newRoute: "/charts/new"     },
  { key: "datasets",   label: "Datasets",      endpoint: "/api/v1/datasets",      newRoute: "/datasets/new"   },
  { key: "queries",    label: "Saved Queries", endpoint: "/api/v1/lab/saved-queries", newRoute: "/lab"            },
];

function itemNav(tab: TabKey, id: string | number): string {
  switch (tab) {
    case "dashboards": return `/dashboards/${id}/view`;
    case "charts":     return `/charts/${id}`;
    case "datasets":   return `/datasets/${id}`;
    case "queries":    return `/lab?savedQueryId=${id}`;
  }
}

// ── Date helper ──────────────────────────────────────────────────────────────

function fmtDate(value?: string | null): string {
  if (!value) return "—";
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return "—";
  const diff = Date.now() - d.getTime();
  const min  = Math.floor(diff / 60_000);
  const hr   = Math.floor(diff / 3_600_000);
  const day  = Math.floor(diff / 86_400_000);
  if (min  <  60) return `${min}m ago`;
  if (hr   <  24) return `${hr}h ago`;
  if (day  <  30) return `${day}d ago`;
  return d.toLocaleDateString();
}

function ownerLabel(email?: string | null): string {
  if (!email) return "—";
  const at = email.indexOf("@");
  return at > 0 ? email.slice(0, at) : email;
}

// ── Page ─────────────────────────────────────────────────────────────────────

const WorkspacePage: React.FC = () => {
  const router       = useRouter();
  const searchParams = useSearchParams();
  const { isAuthenticated } = useAuth();
  const { isSetupOk }       = useSetup();

  const rawTab  = searchParams.get("tab") as TabKey | null;
  const activeTab: TabKey = TABS.some(t => t.key === rawTab) ? rawTab! : "dashboards";

  const [scope,   setScope]   = useState<"mine" | "all">("all");
  const [search,  setSearch]  = useState("");
  const [items,   setItems]   = useState<WorkspaceItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error,   setError]   = useState<string | null>(null);

  const tab = TABS.find(t => t.key === activeTab)!;

  const switchTab = (key: TabKey) => {
    setItems([]);
    setError(null);
    setSearch("");
    router.push(`/workspace?tab=${key}`);
  };

  const load = useCallback(async () => {
    if (!isAuthenticated || isSetupOk !== true) return;
    setLoading(true);
    setError(null);
    try {
      const res = await msalFetch(`${API_BASE}${tab.endpoint}`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      const arr: WorkspaceItem[] = Array.isArray(data)
        ? data
        : Array.isArray(data.result)
          ? data.result
          : Array.isArray(data.items)
            ? data.items
            : [];
      setItems(arr);
    } catch {
      setError(`Failed to load ${tab.label.toLowerCase()}. Check your connection and try again.`);
    } finally {
      setLoading(false);
    }
  }, [isAuthenticated, isSetupOk, tab.endpoint, tab.label]);

  useEffect(() => { void load(); }, [load]);

  // ── Derived list ────────────────────────────────────────────────────────────
  const filtered = items.filter(item => {
    const label = (item.name ?? item.title ?? "").toLowerCase();
    if (search && !label.includes(search.toLowerCase())) return false;
    return true;
  });

  const newRoute = tab.newRoute;

  // ── Styles ──────────────────────────────────────────────────────────────────

  const page: React.CSSProperties = {
    maxWidth: 1200,
    margin: "0 auto",
    padding: "32px 40px",
  };

  const headerRow: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    gap: 12,
    marginBottom: 24,
  };

  const titleStyle: React.CSSProperties = {
    margin: 0,
    fontSize: 22,
    fontWeight: 600,
    color: "var(--text-primary)",
    flex: 1,
  };

  const searchInput: React.CSSProperties = {
    width: 200,
    height: 34,
    padding: "0 10px",
    fontSize: 13,
    border: "1px solid var(--border)",
    borderRadius: 6,
    background: "var(--bg-surface)",
    color: "var(--text-primary)",
    outline: "none",
  };

  const newBtn: React.CSSProperties = {
    height: 34,
    padding: "0 14px",
    fontSize: 13,
    fontWeight: 600,
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    borderRadius: 6,
    cursor: "pointer",
    display: "flex",
    alignItems: "center",
    gap: 6,
    whiteSpace: "nowrap",
  };

  const tabBar: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    borderBottom: "1px solid var(--border)",
    marginBottom: 20,
    gap: 0,
  };

  const tabBtnBase: React.CSSProperties = {
    padding: "10px 16px",
    fontSize: 13,
    fontWeight: 500,
    border: "none",
    borderBottom: "2px solid transparent",
    background: "none",
    cursor: "pointer",
    color: "var(--text-secondary)",
    marginBottom: -1,
    transition: "color 0.15s, border-color 0.15s",
  };

  const tabBtnActive: React.CSSProperties = {
    ...tabBtnBase,
    color: "#2563eb",
    borderBottomColor: "#2563eb",
    fontWeight: 600,
  };

  const scopeToggle: React.CSSProperties = {
    marginLeft: "auto",
    display: "flex",
    border: "1px solid var(--border)",
    borderRadius: 6,
    overflow: "hidden",
  };

  const scopeBtn = (active: boolean): React.CSSProperties => ({
    padding: "5px 12px",
    fontSize: 12,
    fontWeight: active ? 600 : 400,
    background: active ? "var(--accent)" : "var(--bg-surface)",
    color: active ? "#fff" : "var(--text-secondary)",
    border: "none",
    cursor: "pointer",
  });

  const tableStyle: React.CSSProperties = {
    width: "100%",
    borderCollapse: "collapse",
    fontSize: 13,
    color: "var(--text-primary)",
  };

  const thStyle: React.CSSProperties = {
    textAlign: "left",
    padding: "8px 12px",
    fontSize: 11,
    fontWeight: 600,
    textTransform: "uppercase",
    letterSpacing: "0.05em",
    color: "var(--text-muted)",
    borderBottom: "1px solid var(--border)",
    background: "var(--bg-surface)",
  };

  const tdStyle: React.CSSProperties = {
    padding: "11px 12px",
    borderBottom: "1px solid var(--border)",
    verticalAlign: "middle",
  };

  const tdMuted: React.CSSProperties = {
    ...tdStyle,
    color: "var(--text-secondary)",
  };

  const centered: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    padding: "60px 0",
    color: "var(--text-secondary)",
    fontSize: 14,
  };

  // ── Render ──────────────────────────────────────────────────────────────────

  return (
    <div style={page}>
      {/* Header */}
      <div style={headerRow}>
        <h1 style={titleStyle}>Workspace</h1>
        <input
          style={searchInput}
          placeholder="Search…"
          value={search}
          onChange={e => setSearch(e.target.value)}
        />
        <button
          type="button"
          style={newBtn}
          onClick={() => { window.location.href = newRoute; }}
        >
          <i className="fas fa-plus" style={{ fontSize: 11 }} />
          New
        </button>
      </div>

      {/* Tab bar */}
      <div style={tabBar}>
        {TABS.map(t => (
          <button
            key={t.key}
            type="button"
            style={activeTab === t.key ? tabBtnActive : tabBtnBase}
            onClick={() => switchTab(t.key)}
          >
            {t.label}
          </button>
        ))}
        <div style={scopeToggle}>
          {(["mine", "all"] as const).map(s => (
            <button key={s} type="button" style={scopeBtn(scope === s)} onClick={() => setScope(s)}>
              {s === "mine" ? "Mine" : "All"}
            </button>
          ))}
        </div>
      </div>

      {/* Content */}
      {loading && (
        <div style={centered}>Loading…</div>
      )}

      {!loading && error && (
        <div style={{ ...centered, flexDirection: "column", gap: 8, color: "var(--error)" }}>
          <i className="fas fa-exclamation-circle" style={{ fontSize: 20 }} />
          <span>{error}</span>
        </div>
      )}

      {!loading && !error && filtered.length === 0 && (
        <div style={{ ...centered, flexDirection: "column", gap: 6 }}>
          <span style={{ color: "var(--text-muted)", fontSize: 15 }}>
            No {tab.label.toLowerCase()} found. Create one to get started.
          </span>
          <button
            type="button"
            style={{ ...newBtn, marginTop: 8 }}
            onClick={() => { window.location.href = newRoute; }}
          >
            <i className="fas fa-plus" style={{ fontSize: 11 }} />
            New {tab.label.slice(0, -1) === tab.label ? tab.label : tab.label.slice(0, -1)}
          </button>
        </div>
      )}

      {!loading && !error && filtered.length > 0 && (
        <table style={tableStyle}>
          <thead>
            <tr>
              <th style={thStyle}>Name</th>
              <th style={thStyle}>Owner</th>
              <th style={thStyle}>Updated</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map(item => {
              const label = item.name ?? item.title ?? "(untitled)";
              const href  = itemNav(activeTab, item.id);
              const ts    = item.updated_at ?? item.created_at;
              return (
                <tr
                  key={item.id}
                  onClick={() => { window.location.href = href; }}
                  style={{ cursor: "pointer" }}
                  onMouseEnter={e => { (e.currentTarget as HTMLTableRowElement).style.background = "var(--bg-hover)"; }}
                  onMouseLeave={e => { (e.currentTarget as HTMLTableRowElement).style.background = ""; }}
                >
                  <td style={tdStyle}>
                    <strong style={{ fontWeight: 500 }}>{label}</strong>
                  </td>
                  <td style={tdMuted}>{ownerLabel(item.created_by)}</td>
                  <td style={tdMuted}>{fmtDate(ts)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
};

export default WorkspacePage;
