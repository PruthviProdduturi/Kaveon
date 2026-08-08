"use client";

import React, { useEffect, useState, useCallback } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useAuth } from "../../auth/useAuth";
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

// SVG icons for tabs and items
function DashboardIcon({ size = 16, color = "currentColor" }: { size?: number; color?: string }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/></svg>;
}
function ChartIcon({ size = 16, color = "currentColor" }: { size?: number; color?: string }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/></svg>;
}
function DatasetIcon({ size = 16, color = "currentColor" }: { size?: number; color?: string }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>;
}
function QueryIcon({ size = 16, color = "currentColor" }: { size?: number; color?: string }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>;
}

const TABS: { key: TabKey; label: string; endpoint: string; newRoute: string; Icon: typeof DashboardIcon }[] = [
  { key: "dashboards", label: "Dashboards", endpoint: "/api/v1/dashboards", newRoute: "/dashboards/new", Icon: DashboardIcon },
  { key: "charts", label: "Charts", endpoint: "/api/v1/charts", newRoute: "/charts/new", Icon: ChartIcon },
  { key: "datasets", label: "Datasets", endpoint: "/api/v1/datasets", newRoute: "/datasets/new", Icon: DatasetIcon },
  { key: "queries", label: "Saved Queries", endpoint: "/api/v1/lab/saved-queries", newRoute: "/lab", Icon: QueryIcon },
];

function itemNav(tab: TabKey, id: string | number): string {
  switch (tab) {
    case "dashboards": return `/dashboards/${id}/view`;
    case "charts": return `/charts/${id}`;
    case "datasets": return `/datasets/${id}`;
    case "queries": return `/lab?savedQueryId=${id}`;
  }
}

function fmtDate(value?: string | null): string {
  if (!value) return "";
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return "";
  const diff = Date.now() - d.getTime();
  if (diff < 0) return "just now";
  const min = Math.floor(diff / 60_000);
  if (min < 1) return "just now";
  if (min < 60) return `${min}m`;
  const hr = Math.floor(diff / 3_600_000);
  if (hr < 24) return `${hr}h`;
  const day = Math.floor(diff / 86_400_000);
  if (day < 30) return `${day}d`;
  return d.toLocaleDateString();
}

function ownerFirst(email?: string | null): string {
  if (!email) return "";
  const at = email.indexOf("@");
  const name = at > 0 ? email.slice(0, at) : email;
  return name.split(".")[0].replace(/^\w/, c => c.toUpperCase());
}

export default function WorkspacePage() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const { isAuthenticated, account } = useAuth();

  const rawTab = searchParams.get("tab") as TabKey | null;
  const activeTab: TabKey = TABS.some((t) => t.key === rawTab) ? rawTab! : "dashboards";

  const [scope, setScope] = useState<"mine" | "all">("all");
  const [search, setSearch] = useState("");
  const [items, setItems] = useState<WorkspaceItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const tab = TABS.find((t) => t.key === activeTab)!;
  const TabItemIcon = tab.Icon;

  const switchTab = (key: TabKey) => {
    setItems([]);
    setError(null);
    setSearch("");
    router.push(`/workspace?tab=${key}`);
  };

  const load = useCallback(async () => {
    if (!isAuthenticated) return;
    setLoading(true);
    setError(null);
    try {
      const res = await msalFetch(tab.endpoint);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      const arr: WorkspaceItem[] = Array.isArray(data) ? data : Array.isArray(data.result) ? data.result : Array.isArray(data.items) ? data.items : [];
      setItems(arr);
    } catch {
      // Retry once on failure (Neon cold start can cause first call to timeout)
      try {
        const retry = await msalFetch(tab.endpoint);
        if (retry.ok) {
          const retryData = await retry.json();
          const retryArr: WorkspaceItem[] = Array.isArray(retryData) ? retryData : Array.isArray(retryData.result) ? retryData.result : Array.isArray(retryData.items) ? retryData.items : [];
          setItems(retryArr);
          setError(null);
          return;
        }
      } catch {}
      setError(`Failed to load ${tab.label.toLowerCase()}.`);
    } finally {
      setLoading(false);
    }
  }, [isAuthenticated, tab.endpoint, tab.label]);

  useEffect(() => { void load(); }, [load]);

  const email = account?.email ?? "";
  const filtered = items.filter((item) => {
    const label = (item.name ?? item.title ?? "").toLowerCase();
    if (search && !label.includes(search.toLowerCase())) return false;
    if (scope === "mine" && item.created_by && item.created_by !== email) return false;
    return true;
  });

  return (
    <div style={{ maxWidth: 1000, margin: "0 auto", padding: "40px 48px" }}>
      {/* Header */}
      <div style={{ display: "flex", alignItems: "center", gap: 16, marginBottom: 32 }}>
        <h1 style={{ margin: 0, fontSize: 26, fontWeight: 600, color: "var(--text-primary)", flex: 1, letterSpacing: "-0.3px" }}>
          Workspace
        </h1>
        <div style={{ position: "relative" }}>
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search..."
            style={{
              width: 200, padding: "9px 12px 9px 34px", fontSize: 13,
              border: "1px solid var(--border)", borderRadius: 10,
              background: "var(--bg-surface)", color: "var(--text-primary)",
              outline: "none",
            }}
          />
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ position: "absolute", left: 12, top: "50%", transform: "translateY(-50%)" }}>
            <circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
        </div>
        <button type="button" onClick={() => router.push(tab.newRoute)} style={{
          padding: "9px 20px", fontSize: 13, fontWeight: 500,
          background: "var(--accent)", color: "#fff", border: "none",
          borderRadius: 10, cursor: "pointer", display: "flex", alignItems: "center", gap: 6,
        }}>
          + New
        </button>
      </div>

      {/* Tabs */}
      <div style={{ display: "flex", alignItems: "center", marginBottom: 8 }}>
        {TABS.map((t) => {
          const active = activeTab === t.key;
          return (
            <button key={t.key} type="button" onClick={() => switchTab(t.key)} style={{
              padding: "10px 18px", fontSize: 14, fontWeight: active ? 600 : 400,
              color: active ? "var(--text-primary)" : "var(--text-muted)",
              background: "none", border: "none", cursor: "pointer",
              borderBottom: active ? "2px solid var(--accent)" : "2px solid transparent",
              marginBottom: -1, transition: "all 0.15s", display: "flex", alignItems: "center", gap: 8,
            }}>
              <t.Icon size={15} color={active ? "var(--accent)" : "var(--text-muted)"} />
              {t.label}
            </button>
          );
        })}
        <div style={{ flex: 1 }} />
        <div style={{ display: "flex", gap: 0, background: "rgba(255,255,255,0.04)", borderRadius: 8, border: "1px solid var(--border)", overflow: "hidden" }}>
          {(["mine", "all"] as const).map((s) => (
            <button key={s} type="button" onClick={() => setScope(s)} style={{
              padding: "6px 16px", fontSize: 12, fontWeight: scope === s ? 600 : 400,
              color: scope === s ? "var(--text-primary)" : "var(--text-muted)",
              background: scope === s ? "rgba(255,255,255,0.08)" : "transparent",
              border: "none", cursor: "pointer", textTransform: "capitalize", transition: "all 0.15s",
            }}>
              {s}
            </button>
          ))}
        </div>
      </div>

      <div style={{ height: 1, background: "var(--border)", marginBottom: 4 }} />

      {/* Loading */}
      {loading && (
        <div style={{ display: "flex", justifyContent: "center", padding: "80px 0", color: "var(--text-muted)" }}>
          <div className="spinner" style={{ width: 24, height: 24, borderWidth: 2 }} />
        </div>
      )}

      {/* Error */}
      {!loading && error && (
        <div style={{ textAlign: "center", padding: "80px 0", color: "var(--text-muted)" }}>
          <p style={{ fontSize: 14 }}>{error}</p>
          <button type="button" onClick={load} style={{
            marginTop: 12, padding: "8px 16px", fontSize: 13,
            border: "1px solid var(--border)", borderRadius: 8,
            background: "var(--bg-surface)", color: "var(--text-secondary)", cursor: "pointer",
          }}>Retry</button>
        </div>
      )}

      {/* Empty */}
      {!loading && !error && filtered.length === 0 && (
        <div style={{ textAlign: "center", padding: "80px 0", color: "var(--text-muted)" }}>
          <p style={{ fontSize: 15, marginBottom: 16 }}>No {tab.label.toLowerCase()} yet</p>
          <button type="button" onClick={() => router.push(tab.newRoute)} style={{
            padding: "9px 22px", fontSize: 13, fontWeight: 500,
            background: "var(--accent)", color: "#fff", border: "none", borderRadius: 10, cursor: "pointer",
          }}>
            Create your first {tab.label.toLowerCase().replace(/s$/, "")}
          </button>
        </div>
      )}

      {/* Items */}
      {!loading && !error && filtered.length > 0 && (
        <div>
          {filtered.map((item) => {
            const label = item.name ?? item.title ?? "Untitled";
            const ts = item.updated_at ?? item.created_at;
            const owner = ownerFirst(item.created_by);
            return (
              <div
                key={item.id}
                onClick={() => router.push(itemNav(activeTab, item.id))}
                style={{
                  display: "flex", alignItems: "center", padding: "14px 12px",
                  cursor: "pointer", transition: "background 0.1s", gap: 14,
                  borderRadius: 10, margin: "2px 0",
                }}
                onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-hover)")}
                onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
              >
                {/* Icon */}
                <div style={{
                  width: 36, height: 36, borderRadius: 10,
                  background: "rgba(var(--accent-rgb), 0.06)",
                  border: "1px solid rgba(var(--accent-rgb), 0.1)",
                  display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0,
                }}>
                  <TabItemIcon size={18} color="var(--accent)" />
                </div>

                {/* Name */}
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{
                    fontSize: 14, fontWeight: 500, color: "var(--text-primary)",
                    overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
                  }}>
                    {label}
                  </div>
                  {owner && (
                    <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 2 }}>{owner}</div>
                  )}
                </div>

                {/* Time */}
                <div style={{ fontSize: 12, color: "var(--text-muted)", flexShrink: 0, minWidth: 50, textAlign: "right" }}>
                  {fmtDate(ts)}
                </div>

                {/* Chevron */}
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--text-faint)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0 }}>
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
