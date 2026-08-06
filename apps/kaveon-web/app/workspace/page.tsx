"use client";

import React, { useEffect, useState, useCallback } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { API_BASE } from "../../config";
import { useAuth } from "../../auth/useAuth";
import { useSetup } from "../../components/ClientLayout";
import { msalFetch } from "../../utils/msalFetch";

type TabKey = "dashboards" | "charts" | "datasets" | "queries";

interface WorkspaceItem {
  id: string | number;
  name?: string;
  title?: string;
  created_by?: string | null;
  updated_at?: string | null;
  created_at?: string | null;
}

const TABS: { key: TabKey; label: string; endpoint: string; newRoute: string; icon: string }[] = [
  { key: "dashboards", label: "Dashboards",    endpoint: "/api/v1/dashboards",          newRoute: "/dashboards/new", icon: "📊" },
  { key: "charts",     label: "Charts",        endpoint: "/api/v1/charts",              newRoute: "/charts/new",     icon: "📈" },
  { key: "datasets",   label: "Datasets",      endpoint: "/api/v1/datasets",            newRoute: "/datasets/new",   icon: "🗂" },
  { key: "queries",    label: "Saved Queries",  endpoint: "/api/v1/lab/saved-queries",   newRoute: "/lab",            icon: "⌨️" },
];

const TAB_TYPE_MAP: Record<TabKey, "dashboard" | "chart" | "dataset" | "query"> = {
  dashboards: "dashboard",
  charts: "chart",
  datasets: "dataset",
  queries: "query",
};

function itemNav(tab: TabKey, id: string | number): string {
  switch (tab) {
    case "dashboards": return `/dashboards/${id}/view`;
    case "charts":     return `/charts/${id}`;
    case "datasets":   return `/datasets/${id}`;
    case "queries":    return `/lab?savedQueryId=${id}`;
  }
}

function fmtDate(value?: string | null): string {
  if (!value) return "—";
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return "—";
  const diff = Date.now() - d.getTime();
  if (diff < 0) return "just now";
  const min = Math.floor(diff / 60_000);
  if (min < 1) return "just now";
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(diff / 3_600_000);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(diff / 86_400_000);
  if (day < 30) return `${day}d ago`;
  return d.toLocaleDateString();
}

function ownerLabel(email?: string | null): string {
  if (!email) return "—";
  const at = email.indexOf("@");
  return at > 0 ? email.slice(0, at) : email;
}

export default function WorkspacePage() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const { isAuthenticated, account } = useAuth();
  const { isSetupOk } = useSetup();
  const rawTab = searchParams.get("tab") as TabKey | null;
  const activeTab: TabKey = TABS.some((t) => t.key === rawTab) ? rawTab! : "dashboards";

  const [scope, setScope] = useState<"mine" | "all">("all");
  const [search, setSearch] = useState("");
  const [items, setItems] = useState<WorkspaceItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const tab = TABS.find((t) => t.key === activeTab)!;

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
        : Array.isArray(data.result) ? data.result
        : Array.isArray(data.items) ? data.items
        : [];
      setItems(arr);
    } catch {
      setError(`Failed to load ${tab.label.toLowerCase()}.`);
    } finally {
      setLoading(false);
    }
  }, [isAuthenticated, isSetupOk, tab.endpoint, tab.label]);

  useEffect(() => { void load(); }, [load]);

  const email = account?.email ?? "";
  const filtered = items.filter((item) => {
    const label = (item.name ?? item.title ?? "").toLowerCase();
    if (search && !label.includes(search.toLowerCase())) return false;
    if (scope === "mine" && item.created_by && item.created_by !== email) return false;
    return true;
  });

  const handleItemClick = (item: WorkspaceItem) => {
    router.push(itemNav(activeTab, item.id));
  };

  return (
    <div style={{ maxWidth: 1100, margin: "0 auto", padding: "36px 48px" }}>
      {/* Header */}
      <div style={{ display: "flex", alignItems: "center", gap: 16, marginBottom: 28 }}>
        <h1 style={{ margin: 0, fontSize: 24, fontWeight: 600, color: "var(--text-primary)", flex: 1 }}>
          Workspace
        </h1>
        <div style={{ position: "relative" }}>
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search..."
            style={{
              width: 220,
              padding: "8px 12px 8px 32px",
              fontSize: 13,
              border: "1px solid var(--border)",
              borderRadius: 8,
              background: "var(--bg-surface)",
              color: "var(--text-primary)",
              outline: "none",
              boxShadow: "var(--shadow-sm)",
            }}
          />
          <svg
            width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)"
            strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"
            style={{ position: "absolute", left: 10, top: "50%", transform: "translateY(-50%)" }}
          >
            <circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
        </div>
        <button
          type="button"
          onClick={() => router.push(tab.newRoute)}
          style={{
            padding: "8px 18px",
            fontSize: 13,
            fontWeight: 500,
            background: "linear-gradient(135deg, #4a9ee8, #2d7dd2)",
            color: "#fff",
            border: "none",
            borderRadius: 8,
            cursor: "pointer",
            display: "flex",
            alignItems: "center",
            gap: 6,
            boxShadow: "0 2px 8px rgba(74,158,232,0.25)",
          }}
        >
          + New
        </button>
      </div>

      {/* Tabs + scope */}
      <div style={{ display: "flex", alignItems: "center", borderBottom: "1px solid var(--border)", marginBottom: 0 }}>
        {TABS.map((t) => (
          <button
            key={t.key}
            type="button"
            onClick={() => switchTab(t.key)}
            style={{
              padding: "10px 18px",
              fontSize: 13,
              fontWeight: activeTab === t.key ? 600 : 400,
              color: activeTab === t.key ? "var(--accent)" : "var(--text-muted)",
              background: "none",
              border: "none",
              borderBottom: activeTab === t.key ? "2px solid var(--accent)" : "2px solid transparent",
              cursor: "pointer",
              marginBottom: -1,
              transition: "all 0.15s",
              display: "flex",
              alignItems: "center",
              gap: 6,
            }}
          >
            <span style={{ fontSize: 12 }}>{t.icon}</span>
            {t.label}
          </button>
        ))}
        <div style={{ flex: 1 }} />
        <div style={{ display: "flex", gap: 2, padding: "4px", background: "var(--bg-hover)", borderRadius: 6 }}>
          {(["mine", "all"] as const).map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => setScope(s)}
              style={{
                padding: "4px 14px",
                fontSize: 12,
                fontWeight: scope === s ? 500 : 400,
                color: scope === s ? "var(--text-primary)" : "var(--text-muted)",
                background: scope === s ? "var(--bg-surface)" : "transparent",
                border: "none",
                borderRadius: 4,
                cursor: "pointer",
                boxShadow: scope === s ? "var(--shadow-sm)" : "none",
                textTransform: "capitalize",
              }}
            >
              {s}
            </button>
          ))}
        </div>
      </div>

      {/* Content */}
      {loading && (
        <div style={{ display: "flex", justifyContent: "center", padding: "80px 0", color: "var(--text-muted)" }}>
          <div className="spinner" style={{ width: 24, height: 24, borderWidth: 2 }} />
        </div>
      )}

      {!loading && error && (
        <div style={{ textAlign: "center", padding: "80px 0", color: "var(--text-muted)" }}>
          <p style={{ fontSize: 14 }}>{error}</p>
          <button
            type="button"
            onClick={load}
            style={{
              marginTop: 12, padding: "8px 16px", fontSize: 13,
              border: "1px solid var(--border)", borderRadius: 8,
              background: "var(--bg-surface)", color: "var(--text-secondary)",
              cursor: "pointer",
            }}
          >
            Retry
          </button>
        </div>
      )}

      {!loading && !error && filtered.length === 0 && (
        <div style={{ textAlign: "center", padding: "80px 0", color: "var(--text-muted)" }}>
          <p style={{ fontSize: 15, marginBottom: 16 }}>No {tab.label.toLowerCase()} yet</p>
          <button
            type="button"
            onClick={() => router.push(tab.newRoute)}
            style={{
              padding: "8px 20px", fontSize: 13, fontWeight: 500,
              background: "linear-gradient(135deg, #4a9ee8, #2d7dd2)",
              color: "#fff", border: "none", borderRadius: 8, cursor: "pointer",
            }}
          >
            Create your first {tab.label.toLowerCase().replace(/s$/, "")}
          </button>
        </div>
      )}

      {!loading && !error && filtered.length > 0 && (
        <div style={{ marginTop: 0 }}>
          {filtered.map((item, idx) => {
            const label = item.name ?? item.title ?? "Untitled";
            const ts = item.updated_at ?? item.created_at;
            return (
              <div
                key={item.id}
                onClick={() => handleItemClick(item)}
                style={{
                  display: "flex",
                  alignItems: "center",
                  padding: "14px 16px",
                  borderBottom: "1px solid var(--border)",
                  cursor: "pointer",
                  transition: "background 0.1s",
                  gap: 14,
                }}
                onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-hover)")}
                onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
              >
                {/* Type icon */}
                <div
                  style={{
                    width: 34,
                    height: 34,
                    borderRadius: 8,
                    background: "rgba(var(--accent-rgb), 0.08)",
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    fontSize: 15,
                    flexShrink: 0,
                  }}
                >
                  {tab.icon}
                </div>

                {/* Name + owner */}
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div
                    style={{
                      fontSize: 14,
                      fontWeight: 500,
                      color: "var(--text-primary)",
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {label}
                  </div>
                  <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 2 }}>
                    {ownerLabel(item.created_by)}
                  </div>
                </div>

                {/* Updated */}
                <div style={{ fontSize: 12, color: "var(--text-muted)", flexShrink: 0, minWidth: 70, textAlign: "right" }}>
                  {fmtDate(ts)}
                </div>

                {/* Arrow */}
                <svg
                  width="14" height="14" viewBox="0 0 24 24" fill="none"
                  stroke="var(--text-faint)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"
                  style={{ flexShrink: 0 }}
                >
                  <polyline points="9 18 15 12 9 6" />
                </svg>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
