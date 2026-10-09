"use client";

import React, { useEffect, useState, useCallback, useRef } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useAuth } from "../../auth/useAuth";
import { msalFetch } from "../../utils/msalFetch";
import { useRecents } from "../../hooks/useRecents";
import { useTheme } from "../../contexts/ThemeContext";
import { KaveonLoading } from "../../components/KaveonLoading";
import { LineageView } from "../../components/LineageView";

type TabKey = "lineage" | "dashboards" | "charts" | "datasets" | "queries";

interface WorkspaceItem {
  id: string | number;
  name?: string;
  title?: string;
  description?: string | null;
  created_by?: string | null;
  updated_at?: string | null;
  created_at?: string | null;
  thumbnail?: string | null;
  thumbnail_dark?: string | null;
  has_thumbnail?: boolean;
  has_thumbnail_dark?: boolean;
  chart_type?: string | null;
  dataset_name?: string | null;
  database_name?: string | null;
  table_name?: string | null;
  schema_name?: string | null;
  sql?: string | null;
  sql_text?: string | null;
  favorite?: boolean;
}

// Singular object_type used by the per-user favorites/pin store.
const PIN_TYPE: Record<TabKey, string> = {
  lineage: "lineage",
  dashboards: "dashboard",
  charts: "chart",
  datasets: "dataset",
  queries: "query",
};

/**
 * Library card cover.
 *
 * A preview is a binary resource: the list response says only whether one
 * exists, and the image itself is fetched from
 * /api/v1/{charts,dashboards}/{id}/thumbnail through the same-origin proxy, so
 * the browser caches it and the list stays small at seventy charts. Anything
 * that cannot be shown — no capture yet, a request that fails, a record type
 * that has no preview — degrades to the group-tinted panel and the tab glyph.
 */
const CardCover: React.FC<{
  src: string | null;
  label: string;
  accent: string;
  Glyph: React.FC<{ size?: number; color?: string }>;
  children?: React.ReactNode;
}> = ({ src, label, accent, Glyph, children }) => {
  const [failed, setFailed] = useState(false);
  useEffect(() => { setFailed(false); }, [src]);
  const showImage = !!src && !failed;
  return (
    <div style={{
      position: "relative", height: 128, flexShrink: 0,
      background: showImage ? "var(--bg-elevated)" : `${accent}14`,
      display: "flex", alignItems: "center", justifyContent: "center",
      borderBottom: "1px solid var(--border)",
    }}>
      {!showImage && <div style={{ position: "absolute", top: 0, left: 0, right: 0, height: 3, background: accent, opacity: 0.85 }} />}
      {showImage ? (
        // Top-align (objectPosition:top) so a tall dashboard shows its top, not a centre crop.
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={src as string}
          alt={label}
          loading="lazy"
          decoding="async"
          onError={() => setFailed(true)}
          style={{ width: "100%", height: "100%", objectFit: "cover", objectPosition: "top" }}
        />
      ) : (
        <Glyph size={34} color={accent} />
      )}
      {children}
    </div>
  );
};

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
function LineageIcon({ size = 16, color = "currentColor" }: { size?: number; color?: string }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><circle cx="5" cy="6" r="3"/><circle cx="19" cy="6" r="3"/><circle cx="12" cy="18" r="3"/><path d="M7.5 7.5L10.5 15.5"/><path d="M16.5 7.5L13.5 15.5"/></svg>;
}

const TABS: { key: TabKey; label: string; endpoint: string; newRoute: string; Icon: typeof DashboardIcon }[] = [
  { key: "lineage", label: "Lineage", endpoint: "", newRoute: "", Icon: LineageIcon },
  { key: "dashboards", label: "Dashboards", endpoint: "/api/v1/dashboards", newRoute: "/dashboards/new", Icon: DashboardIcon },
  { key: "charts", label: "Charts", endpoint: "/api/v1/charts", newRoute: "/charts/new", Icon: ChartIcon },
  { key: "datasets", label: "Datasets", endpoint: "/api/v1/datasets", newRoute: "/datasets/new", Icon: DatasetIcon },
  { key: "queries", label: "Saved Queries", endpoint: "/api/v1/lab/saved-queries", newRoute: "/lab", Icon: QueryIcon },
];

function itemNav(tab: TabKey, id: string | number): string {
  switch (tab) {
    case "lineage": return `/workspace?tab=lineage`;
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

// Curated, muted accent palette. Color is assigned per GROUP (dataset/source),
// not per card — so it encodes "which dataset this belongs to" instead of being
// decorative rainbow noise. One calm hue per group, cycled.
const GROUP_ACCENTS = ["#3b82f6", "#10b981", "#8b5cf6", "#f59e0b", "#ec4899", "#06b6d4"];
const NEUTRAL_ACCENT = "#64748b";

// Layout per tab: visual objects → cards; data/text objects → dense rows.
const TAB_LAYOUT: Record<TabKey, "cards" | "rows" | "custom"> = {
  lineage: "custom",
  dashboards: "cards",
  charts: "cards",
  datasets: "rows",
  queries: "rows",
};

// How the Charts tab is sectioned. Dataset is the default: every chart is built
// on exactly one dataset, so it is a true partition of the tab. Dashboard is
// offered because that is how charts are read rather than how they are built —
// it is not a partition, since a chart can sit on several dashboards or on none,
// and both of those cases are shown rather than hidden.
type ChartGrouping = "dataset" | "dashboard";

const CHART_GROUPINGS: { key: ChartGrouping; label: string }[] = [
  { key: "dataset", label: "Dataset" },
  { key: "dashboard", label: "Dashboard" },
];

// Headings for the charts a grouping cannot place. Named for what is actually
// true of them, so neither an ordinary state nor a fault lands in a catch-all.
const UNPLACED_HEADING: Record<ChartGrouping, string> = {
  dataset: "Dataset unavailable",
  dashboard: "Not on a dashboard",
};

// Chart identities a dashboard holds. The record carries them as a JSON array,
// serialized by some read paths and already parsed by others.
function dashboardChartIds(dashboard: { charts?: unknown }): string[] {
  let held: unknown = dashboard.charts;
  if (typeof held === "string") {
    try { held = JSON.parse(held); } catch { return []; }
  }
  if (!Array.isArray(held)) return [];
  return held
    .map((entry) => (entry !== null && typeof entry === "object"
      ? String((entry as { chart_id?: unknown; id?: unknown }).chart_id
        ?? (entry as { id?: unknown }).id ?? "")
      : String(entry ?? "")))
    .filter((identity) => identity !== "");
}

// Which sections a tab's items belong to. Only charts are sectioned; the rest
// render as one flat list. A chart on two dashboards belongs to both.
function groupKeysFor(
  tab: TabKey,
  item: WorkspaceItem,
  grouping: ChartGrouping,
  placements: Map<string, string[]> | null,
): string[] {
  if (tab !== "charts") return [""];
  if (grouping === "dashboard") {
    const on = placements?.get(String(item.id));
    return on && on.length > 0 ? on : [UNPLACED_HEADING.dashboard];
  }
  return [item.dataset_name?.trim() || UNPLACED_HEADING.dataset];
}

const RECENT_ICON: Record<string, string> = {
  dashboard: "fas fa-table-cells-large",
  chart: "fas fa-chart-column",
  dataset: "fas fa-database",
  query: "fas fa-code",
  chat: "fas fa-comment",
};

// Humanize a chart_type slug for display on chart cards (e.g. "time_series_line"
// -> "Time Series Line", "big_number" -> "Big Number").
function chartTypeLabel(t?: string | null): string {
  if (!t) return "";
  return t.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

export default function WorkspacePage() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const { isAuthenticated, account } = useAuth();
  const { recents } = useRecents();
  const { theme } = useTheme();
  const isDark = theme === "dark";

  const rawTab = searchParams.get("tab") as TabKey | null;
  const activeTab: TabKey = TABS.some((t) => t.key === rawTab) ? rawTab! : "dashboards";

  const [scope, setScope] = useState<"mine" | "all">("all");
  const [search, setSearch] = useState("");
  const [items, setItems] = useState<WorkspaceItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Which tab the current `items` belong to — gates rendering so the previous
  // tab's content/empty-state never flashes while the new tab is still loading.
  const [loadedTab, setLoadedTab] = useState<TabKey | null>(null);
  // Groups are collapsed by default; a set of expanded group keys.
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(new Set());
  // How the Charts tab is sectioned, and the chart→dashboard placements that
  // the dashboard sectioning needs. Placements are read once, from the same
  // dashboard list the Dashboards tab uses, and only when that grouping is
  // chosen — the default grouping costs no extra request.
  const [chartGrouping, setChartGrouping] = useState<ChartGrouping>("dataset");
  const [placements, setPlacements] = useState<Map<string, string[]> | null>(null);
  const [placementsFailed, setPlacementsFailed] = useState(false);

  const toggleGroup = useCallback((key: string) => {
    setExpandedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  }, []);

  const tab = TABS.find((t) => t.key === activeTab)!;
  const TabItemIcon = tab.Icon;

  const switchTab = (key: TabKey) => {
    if (key === activeTab) return;
    setItems([]);
    setLoadedTab(null);   // invalidate immediately so the old tab can't flash
    setLoading(true);      // show the spinner right away, before the URL updates
    setError(null);
    setSearch("");
    // expandedGroups is restored per-tab by the effect below (persisted state),
    // so leaving/returning keeps your expanded sections.
    router.push(`/workspace?tab=${key}`);
  };

  // Sections belong to a tab and, on Charts, to the chosen grouping — the two
  // groupings have different headings, so they remember their own open sections.
  const sectionScope = activeTab === "charts" ? `charts:${chartGrouping}` : activeTab;

  // Restore the chosen chart grouping before anything is sectioned by it.
  useEffect(() => {
    if (typeof window === "undefined") return;
    try {
      const stored = sessionStorage.getItem("ws-group-charts");
      if (CHART_GROUPINGS.some((option) => option.key === stored)) {
        setChartGrouping(stored as ChartGrouping);
      }
    } catch {}
  }, []);

  useEffect(() => {
    if (typeof window === "undefined") return;
    try { sessionStorage.setItem("ws-group-charts", chartGrouping); } catch {}
  }, [chartGrouping]);

  // Persist + restore expanded sections and scroll per tab, so navigating away
  // (opening a chart) and back returns you to the exact position you left.
  useEffect(() => {
    if (typeof window === "undefined") return;
    try {
      const raw = sessionStorage.getItem(`ws-exp-${sectionScope}`);
      setExpandedGroups(raw ? new Set<string>(JSON.parse(raw)) : new Set());
    } catch { setExpandedGroups(new Set()); }
  }, [sectionScope]);

  useEffect(() => {
    if (typeof window === "undefined") return;
    try { sessionStorage.setItem(`ws-exp-${sectionScope}`, JSON.stringify([...expandedGroups])); } catch {}
  }, [expandedGroups, sectionScope]);

  // Save scroll position per tab (throttled).
  useEffect(() => {
    if (typeof window === "undefined") return;
    let t: ReturnType<typeof setTimeout>;
    const onScroll = () => {
      clearTimeout(t);
      t = setTimeout(() => { try { sessionStorage.setItem(`ws-scroll-${activeTab}`, String(window.scrollY)); } catch {} }, 120);
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => { window.removeEventListener("scroll", onScroll); clearTimeout(t); };
  }, [activeTab]);

  // Restore scroll once this tab's items have loaded (so the content exists).
  const scrollRestoredRef = useRef<string>("");
  useEffect(() => {
    if (typeof window === "undefined") return;
    if (loadedTab !== activeTab || scrollRestoredRef.current === activeTab) return;
    scrollRestoredRef.current = activeTab;
    try {
      const y = Number(sessionStorage.getItem(`ws-scroll-${activeTab}`) || 0);
      if (y > 0) requestAnimationFrame(() => window.scrollTo(0, y));
    } catch {}
  }, [loadedTab, activeTab]);

  const load = useCallback(async () => {
    if (!isAuthenticated) return;
    if (tab.key === "lineage") { setLoading(false); setLoadedTab("lineage"); return; }
    const tabKey = tab.key; // the tab this load belongs to
    setLoading(true);
    setError(null);
    try {
      const res = await msalFetch(tab.endpoint);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      const arr: WorkspaceItem[] = Array.isArray(data) ? data : Array.isArray(data.result) ? data.result : Array.isArray(data.items) ? data.items : [];
      setItems(arr);
      setLoadedTab(tabKey);
    } catch {
      // Retry once on failure (cold start can cause the first call to time out)
      try {
        const retry = await msalFetch(tab.endpoint);
        if (retry.ok) {
          const retryData = await retry.json();
          const retryArr: WorkspaceItem[] = Array.isArray(retryData) ? retryData : Array.isArray(retryData.result) ? retryData.result : Array.isArray(retryData.items) ? retryData.items : [];
          setItems(retryArr);
          setLoadedTab(tabKey);
          setError(null);
          return;
        }
      } catch {}
      setError(`Failed to load ${tab.label.toLowerCase()}.`);
    } finally {
      setLoading(false);
    }
  }, [isAuthenticated, tab.endpoint, tab.label, tab.key]);

  useEffect(() => { void load(); }, [load]);

  // Chart→dashboard placements, read once from the dashboard list. The list
  // already carries each dashboard's chart identities, so sectioning seventy
  // charts by dashboard costs one request, not one per dashboard.
  useEffect(() => {
    if (!isAuthenticated) return;
    if (activeTab !== "charts" || chartGrouping !== "dashboard") return;
    if (placements || placementsFailed) return;
    let cancelled = false;
    (async () => {
      try {
        const res = await msalFetch("/api/v1/dashboards");
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();
        const list: { id?: string | number; name?: string; charts?: unknown }[] =
          Array.isArray(data) ? data : [];
        const map = new Map<string, string[]>();
        for (const dashboard of list) {
          const name = dashboard.name?.trim() || "Untitled dashboard";
          for (const chartId of dashboardChartIds(dashboard)) {
            const on = map.get(chartId);
            if (!on) map.set(chartId, [name]);
            else if (!on.includes(name)) on.push(name);
          }
        }
        if (!cancelled) setPlacements(map);
      } catch {
        if (!cancelled) setPlacementsFailed(true);
      }
    })();
    return () => { cancelled = true; };
  }, [isAuthenticated, activeTab, chartGrouping, placements, placementsFailed]);

  const [deletingId, setDeletingId] = useState<string | number | null>(null);
  const [confirmItem, setConfirmItem] = useState<WorkspaceItem | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const requestDelete = useCallback((e: React.MouseEvent, item: WorkspaceItem) => {
    e.stopPropagation();
    setDeleteError(null);
    setConfirmItem(item);
  }, []);

  // Pin/unpin — persisted per user via the generic favorites store.
  const togglePin = useCallback(async (e: React.MouseEvent, item: WorkspaceItem) => {
    e.stopPropagation();
    const next = !item.favorite;
    setItems((prev) => prev.map((i) => (i.id === item.id ? { ...i, favorite: next } : i)));
    try {
      const res = await msalFetch("/api/v1/favorites/toggle", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          object_type: PIN_TYPE[activeTab],
          object_id: String(item.id),
          object_name: item.name ?? item.title ?? "",
        }),
      });
      if (!res.ok) throw new Error(String(res.status));
    } catch {
      // Revert on failure.
      setItems((prev) => prev.map((i) => (i.id === item.id ? { ...i, favorite: !next } : i)));
    }
  }, [activeTab]);

  const confirmDelete = useCallback(async () => {
    if (!confirmItem) return;
    const item = confirmItem;
    const kind = tab.label.toLowerCase().replace(/s$/, "");
    setDeletingId(item.id);
    setDeleteError(null);
    try {
      const res = await msalFetch(`${tab.endpoint}/${item.id}`, { method: "DELETE" });
      if (!res.ok && res.status !== 204) throw new Error(`HTTP ${res.status}`);
      setItems((prev) => prev.filter((i) => i.id !== item.id));
      setConfirmItem(null);
    } catch {
      setDeleteError(`Couldn't delete that ${kind}. You may not have permission.`);
    } finally {
      setDeletingId(null);
    }
  }, [confirmItem, tab.endpoint, tab.label]);

  const email = account?.email ?? "";
  // A dashboard list that cannot be read is said so rather than silently
  // sectioning every chart as unplaced; the tab falls back to dataset sections
  // meanwhile, so the cards stay usable.
  const grouping: ChartGrouping = placementsFailed ? "dataset" : chartGrouping;
  const placementsPending = activeTab === "charts" && grouping === "dashboard" && !placements;
  // Only trust `items` once they've been loaded for the CURRENT tab.
  const ready = loadedTab === activeTab && !loading && !placementsPending;
  const filtered = (ready ? items : []).filter((item) => {
    const label = (item.name ?? item.title ?? "").toLowerCase();
    if (search && !label.includes(search.toLowerCase())) return false;
    if (scope === "mine" && item.created_by && item.created_by !== email) return false;
    return true;
  });

  // Pinned items surface in their own section at the top (and are removed from
  // the normal grouped sections so they aren't shown twice).
  const pinned = filtered.filter((i) => i.favorite)
    .sort((a, b) => (a.name ?? a.title ?? "").localeCompare(b.name ?? b.title ?? ""));
  const unpinned = filtered.filter((i) => !i.favorite);

  // Section items by their organizing unit and assign one calm accent per
  // section. Every tab but Charts returns "" → a single, unheaded group.
  const unplacedHeading = UNPLACED_HEADING[grouping];
  const grouped = (() => {
    const map = new Map<string, WorkspaceItem[]>();
    for (const it of unpinned) {
      for (const key of groupKeysFor(activeTab, it, grouping, placements)) {
        if (!map.has(key)) map.set(key, []);
        map.get(key)!.push(it);
      }
    }
    // Named sections alphabetically; the unplaced section and the unheaded
    // single group both sort last.
    const order = (key: string) => (key === "" || key === unplacedHeading ? 1 : 0);
    const entries = Array.from(map.entries()).sort((a, b) =>
      order(a[0]) - order(b[0]) || a[0].localeCompare(b[0])
    );
    return entries.map(([key, groupItems], idx) => ({
      key,
      items: [...groupItems].sort((a, b) =>
        (a.name ?? a.title ?? "").localeCompare(b.name ?? b.title ?? "")
      ),
      // The unplaced section is neutral: it is not one more dataset or
      // dashboard, so it does not take a hue from the palette.
      accent: key === "" || key === unplacedHeading
        ? NEUTRAL_ACCENT
        : GROUP_ACCENTS[idx % GROUP_ACCENTS.length],
    }));
  })();
  const useSections = grouped.length > 1 || (grouped.length === 1 && grouped[0].key !== "");

  const showRecents = recents.length > 0 && !search;

  const renderCard = (item: WorkspaceItem, accent: string) => {
    const label = item.name ?? item.title ?? "Untitled";
    const ts = item.updated_at ?? item.created_at;
    const owner = ownerFirst(item.created_by);
    // An inline data URI is still honoured when one is present, so a record read
    // from a source that carries the image whole needs no special case; otherwise
    // the cover points at the preview endpoint, which serves the theme-matching
    // capture and falls back to the other theme server-side.
    const inlineThumb = isDark
      ? (item.thumbnail_dark || item.thumbnail)
      : (item.thumbnail || item.thumbnail_dark);
    const storedPreview = Boolean(item.has_thumbnail || item.has_thumbnail_dark);
    const coverSrc = inlineThumb && String(inlineThumb).startsWith("data:")
      ? String(inlineThumb)
      : storedPreview && (activeTab === "charts" || activeTab === "dashboards")
        ? `/api/kaveon/api/v1/${activeTab}/${item.id}/thumbnail`
          + (activeTab === "dashboards" ? `?theme=${isDark ? "dark" : "light"}` : "")
        : null;
    return (
      <div
        key={item.id}
        onClick={() => router.push(itemNav(activeTab, item.id))}
        style={{
          cursor: "pointer", borderRadius: 14, overflow: "hidden",
          background: "var(--bg-surface)", border: "1px solid var(--border)",
          transition: "transform 0.12s, box-shadow 0.12s, border-color 0.12s",
          display: "flex", flexDirection: "column",
        }}
        onMouseEnter={(e) => {
          e.currentTarget.style.transform = "translateY(-3px)";
          e.currentTarget.style.boxShadow = "0 12px 28px rgba(0,0,0,0.28)";
          e.currentTarget.style.borderColor = accent;
          const del = e.currentTarget.querySelector<HTMLElement>(".workspace-card-delete");
          if (del) del.style.opacity = "1";
          const pin = e.currentTarget.querySelector<HTMLElement>(".workspace-card-pin");
          if (pin) pin.style.opacity = "1";
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.transform = "translateY(0)";
          e.currentTarget.style.boxShadow = "none";
          e.currentTarget.style.borderColor = "var(--border)";
          const del = e.currentTarget.querySelector<HTMLElement>(".workspace-card-delete");
          if (del && deletingId !== item.id) del.style.opacity = "0";
          const pin = e.currentTarget.querySelector<HTMLElement>(".workspace-card-pin");
          if (pin && !item.favorite) pin.style.opacity = "0";
        }}
      >
        {/* Cover: the captured preview when there is one, else a calm group-tinted
            panel + glyph. Colour comes from the item's GROUP accent, not a random
            per-card hue. */}
        <CardCover src={coverSrc} label={label} accent={accent} Glyph={TabItemIcon}>
          {/* Pin — always visible when pinned, else reveals on hover */}
          <button
            type="button"
            title={item.favorite ? "Unpin" : "Pin"}
            onClick={(e) => togglePin(e, item)}
            className="workspace-card-pin"
            style={{
              position: "absolute", top: 8, left: 8, width: 30, height: 30, borderRadius: 8,
              border: "none", background: "rgba(15,23,42,0.72)", backdropFilter: "blur(4px)",
              display: "flex", alignItems: "center", justifyContent: "center", cursor: "pointer",
              color: item.favorite ? "#fbbf24" : "#fff",
              opacity: item.favorite ? 1 : 0, transition: "opacity 0.12s",
            }}
          >
            <i className={item.favorite ? "fas fa-thumbtack" : "fas fa-thumbtack"} style={{ fontSize: 12 }} />
          </button>

          {/* Delete — reveals on card hover */}
          <button
            type="button"
            title={`Delete ${tab.label.toLowerCase().replace(/s$/, "")}`}
            onClick={(e) => requestDelete(e, item)}
            disabled={deletingId === item.id}
            className="workspace-card-delete"
            style={{
              position: "absolute", top: 8, right: 8, width: 30, height: 30, borderRadius: 8,
              border: "none", background: "rgba(15,23,42,0.72)", backdropFilter: "blur(4px)",
              display: "flex", alignItems: "center", justifyContent: "center",
              cursor: deletingId === item.id ? "default" : "pointer",
              color: "#fff", opacity: 0, transition: "opacity 0.12s, background 0.12s",
            }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "#dc2626"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "rgba(15,23,42,0.72)"; }}
          >
            {deletingId === item.id ? (
              <div className="spinner" style={{ width: 13, height: 13, borderWidth: 2, borderTopColor: "#fff" }} />
            ) : (
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <polyline points="3 6 5 6 21 6" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /><line x1="10" y1="11" x2="10" y2="17" /><line x1="14" y1="11" x2="14" y2="17" />
              </svg>
            )}
          </button>
        </CardCover>

        {/* Body */}
        <div style={{ padding: "12px 14px 14px", flex: 1, display: "flex", flexDirection: "column", minWidth: 0 }}>
          <div style={{
            fontSize: 14, fontWeight: 600, color: "var(--text-primary)", letterSpacing: "-0.2px",
            overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
          }}>
            {label}
          </div>
          {item.description && (
            <div style={{
              fontSize: 12, color: "var(--text-muted)", marginTop: 3, lineHeight: 1.4,
              display: "-webkit-box", WebkitLineClamp: 2, WebkitBoxOrient: "vertical", overflow: "hidden",
            }}>
              {item.description}
            </div>
          )}
          <div style={{ flex: 1 }} />
          <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 10, fontSize: 11.5, color: "var(--text-muted)" }}>
            {item.chart_type && (
              <>
                <span style={{ display: "inline-flex", alignItems: "center", gap: 5, color: "var(--text-primary)", fontWeight: 600 }}>
                  <i className="fas fa-chart-column" style={{ fontSize: 10 }} />
                  {chartTypeLabel(item.chart_type)}
                </span>
                {(owner || ts) && <span style={{ opacity: 0.5 }}>·</span>}
              </>
            )}
            {owner && <span>{owner}</span>}
            {owner && ts && <span style={{ opacity: 0.5 }}>·</span>}
            {ts && <span>{fmtDate(ts)}</span>}
          </div>
        </div>
      </div>
    );
  };

  const gridStyle: React.CSSProperties = {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))",
    gap: 18,
  };

  // Dense row for non-visual objects (datasets, saved queries). Shows the
  // metadata that actually matters instead of a fake thumbnail cover.
  const renderRow = (item: WorkspaceItem) => {
    const label = item.name ?? item.title ?? "Untitled";
    const ts = item.updated_at ?? item.created_at;
    const owner = ownerFirst(item.created_by);
    const isDataset = activeTab === "datasets";
    const sql = (item.sql || item.sql_text || "").replace(/\s+/g, " ").trim();
    const qualified = [item.schema_name, item.table_name].filter(Boolean).join(".");
    return (
      <div
        key={item.id}
        onClick={() => router.push(itemNav(activeTab, item.id))}
        style={{
          display: "flex", alignItems: "center", gap: 14, padding: "12px 14px",
          cursor: "pointer", borderRadius: 12, border: "1px solid var(--border)",
          background: "var(--bg-surface)", transition: "border-color 0.12s, background 0.12s",
        }}
        onMouseEnter={(e) => {
          e.currentTarget.style.borderColor = "var(--accent)";
          const del = e.currentTarget.querySelector<HTMLElement>(".workspace-row-delete");
          if (del) del.style.opacity = "1";
          const pin = e.currentTarget.querySelector<HTMLElement>(".workspace-row-pin");
          if (pin) pin.style.opacity = "1";
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.borderColor = "var(--border)";
          const del = e.currentTarget.querySelector<HTMLElement>(".workspace-row-delete");
          if (del && deletingId !== item.id) del.style.opacity = "0";
          const pin = e.currentTarget.querySelector<HTMLElement>(".workspace-row-pin");
          if (pin && !item.favorite) pin.style.opacity = "0";
        }}
      >
        <div style={{
          width: 38, height: 38, borderRadius: 9, flexShrink: 0,
          background: "rgba(var(--accent-rgb), 0.07)", border: "1px solid rgba(var(--accent-rgb), 0.12)",
          display: "flex", alignItems: "center", justifyContent: "center",
        }}>
          <TabItemIcon size={18} color="var(--accent)" />
        </div>

        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span style={{ fontSize: 14, fontWeight: 600, color: "var(--text-primary)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{label}</span>
            {isDataset && qualified && (
              <span style={{ fontSize: 11, fontFamily: "var(--font-mono, monospace)", color: "var(--text-muted)", background: "rgba(255,255,255,0.05)", border: "1px solid var(--border)", borderRadius: 6, padding: "1px 7px", flexShrink: 0 }}>
                {qualified}
              </span>
            )}
            {isDataset && item.database_name && (
              <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>{item.database_name}</span>
            )}
          </div>
          {isDataset ? (
            item.description && item.description !== label ? (
              <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 3, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{item.description}</div>
            ) : null
          ) : (
            sql && (
              <div style={{ fontSize: 12, fontFamily: "var(--font-mono, monospace)", color: "var(--text-muted)", marginTop: 4, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{sql}</div>
            )
          )}
        </div>

        <div style={{ display: "flex", alignItems: "center", gap: 10, flexShrink: 0, fontSize: 11.5, color: "var(--text-muted)" }}>
          {owner && <span>{owner}</span>}
          {ts && <span>{fmtDate(ts)}</span>}
        </div>

        <button
          type="button"
          title={item.favorite ? "Unpin" : "Pin"}
          onClick={(e) => togglePin(e, item)}
          className="workspace-row-pin"
          style={{
            flexShrink: 0, width: 30, height: 30, borderRadius: 8, border: "1px solid transparent",
            background: "transparent", display: "flex", alignItems: "center", justifyContent: "center", cursor: "pointer",
            color: item.favorite ? "#f59e0b" : "var(--text-faint)",
            opacity: item.favorite ? 1 : 0, transition: "opacity 0.12s, color 0.12s",
          }}
        >
          <i className="fas fa-thumbtack" style={{ fontSize: 12 }} />
        </button>

        <button
          type="button"
          title={`Delete ${tab.label.toLowerCase().replace(/s$/, "")}`}
          onClick={(e) => requestDelete(e, item)}
          disabled={deletingId === item.id}
          className="workspace-row-delete"
          style={{
            flexShrink: 0, width: 30, height: 30, borderRadius: 8, border: "1px solid transparent",
            background: "transparent", display: "flex", alignItems: "center", justifyContent: "center",
            cursor: deletingId === item.id ? "default" : "pointer",
            color: "var(--text-faint)", opacity: 0, transition: "opacity 0.12s, color 0.12s, background 0.12s",
          }}
          onMouseEnter={(e) => { e.currentTarget.style.color = "#dc2626"; e.currentTarget.style.background = "rgba(220,38,38,0.08)"; }}
          onMouseLeave={(e) => { e.currentTarget.style.color = "var(--text-faint)"; e.currentTarget.style.background = "transparent"; }}
        >
          {deletingId === item.id ? (
            <div className="spinner" style={{ width: 13, height: 13, borderWidth: 2 }} />
          ) : (
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <polyline points="3 6 5 6 21 6" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /><line x1="10" y1="11" x2="10" y2="17" /><line x1="14" y1="11" x2="14" y2="17" />
            </svg>
          )}
        </button>
      </div>
    );
  };

  return (
    <div className="workspace-root" style={{ maxWidth: 1400, margin: "0 auto", padding: "32px 40px 64px" }}>
      {/* Jump back in — recents strip */}
      {showRecents && (
        <div style={{ marginBottom: 28 }}>
          <div style={{ fontSize: 12, fontWeight: 600, letterSpacing: "0.4px", textTransform: "uppercase", color: "var(--text-muted)", marginBottom: 12 }}>
            Jump back in
          </div>
          {/* Vertical padding so the hover lift + accent border isn't clipped by overflow-x */}
          <div style={{ display: "flex", gap: 12, overflowX: "auto", overflowY: "visible", padding: "6px 2px" }}>
            {recents.slice(0, 5).map((r) => (
              <button
                key={r.id}
                type="button"
                onClick={() => router.push(r.href)}
                style={{
                  flexShrink: 0, width: 200, textAlign: "left", cursor: "pointer",
                  display: "flex", alignItems: "center", gap: 10, padding: "12px 14px",
                  background: "var(--bg-surface)", border: "1px solid var(--border)", borderRadius: 12,
                  transition: "border-color 0.12s, transform 0.12s",
                }}
                onMouseEnter={(e) => { e.currentTarget.style.borderColor = "var(--accent)"; e.currentTarget.style.transform = "translateY(-1px)"; }}
                onMouseLeave={(e) => { e.currentTarget.style.borderColor = "var(--border)"; e.currentTarget.style.transform = "translateY(0)"; }}
              >
                <div style={{
                  width: 34, height: 34, borderRadius: 9, flexShrink: 0,
                  background: "rgba(255,255,255,0.05)", border: "1px solid var(--border)",
                  display: "flex", alignItems: "center", justifyContent: "center",
                }}>
                  <i className={RECENT_ICON[r.type] || "fas fa-file"} style={{ color: "var(--text-secondary)", fontSize: 14 }} />
                </div>
                <div style={{ minWidth: 0 }}>
                  <div style={{ fontSize: 13, fontWeight: 500, color: "var(--text-primary)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{r.label}</div>
                  <div style={{ fontSize: 11, color: "var(--text-muted)", textTransform: "capitalize", marginTop: 1 }}>{r.type}</div>
                </div>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Header: tabs + search + New in one row (no redundant page title) */}
      <div style={{ display: "flex", alignItems: "center", marginBottom: 8, gap: 16, flexWrap: "wrap" }}>
        {/* The strip scrolls rather than shrinking. Its buttons have no give,
            so a narrow header used to run them underneath the controls to the
            right — "Saved Queries" sat beneath the scope toggle. */}
        <div style={{ display: "flex", alignItems: "center", flex: 1, minWidth: 0,
                      overflowX: "auto", scrollbarWidth: "none" }}>
        {TABS.map((t) => {
          const active = activeTab === t.key;
          return (
            <button key={t.key} type="button" onClick={() => switchTab(t.key)} style={{
              padding: "10px 18px", fontSize: 14, fontWeight: active ? 600 : 400,
              color: active ? "var(--text-primary)" : "var(--text-muted)",
              background: "none", border: "none", cursor: "pointer",
              borderBottom: active ? "2px solid var(--accent)" : "2px solid transparent",
              marginBottom: -1, transition: "all 0.15s", display: "flex", alignItems: "center", gap: 8,
              flexShrink: 0, whiteSpace: "nowrap",
            }}>
              <t.Icon size={15} color={active ? "var(--accent)" : "var(--text-muted)"} />
              {t.label}
              {active && ready && items.length > 0 && (
                <span style={{ fontSize: 11, fontWeight: 500, color: "var(--text-muted)", background: "rgba(255,255,255,0.06)", borderRadius: 6, padding: "1px 6px" }}>{items.length}</span>
              )}
            </button>
          );
        })}
        </div>

        {activeTab !== "lineage" && (<>
        {/* Scope toggle */}
        <div style={{ display: "flex", gap: 0, background: "rgba(255,255,255,0.04)", borderRadius: 8, border: "1px solid var(--border)", overflow: "hidden", flexShrink: 0 }}>
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

        {/* Section the Charts tab by how a chart is built (its dataset) or by
            where it is read (its dashboards). Only Charts spans both. */}
        {activeTab === "charts" && (
          <div style={{ display: "flex", alignItems: "center", gap: 8, flexShrink: 0 }}>
            <span style={{ fontSize: 12, color: "var(--text-muted)", whiteSpace: "nowrap" }}>Group by</span>
            <div style={{ display: "flex", gap: 0, background: "var(--bg-surface)", borderRadius: 8, border: "1px solid var(--border)", overflow: "hidden" }}>
              {CHART_GROUPINGS.map((option) => {
                const chosen = chartGrouping === option.key;
                return (
                  <button key={option.key} type="button" onClick={() => setChartGrouping(option.key)} style={{
                    padding: "6px 14px", fontSize: 12, fontWeight: chosen ? 600 : 400,
                    color: chosen ? "var(--text-primary)" : "var(--text-muted)",
                    background: chosen ? "var(--bg-hover)" : "transparent",
                    border: "none", cursor: "pointer", transition: "all 0.15s", whiteSpace: "nowrap",
                  }}>
                    {option.label}
                  </button>
                );
              })}
            </div>
          </div>
        )}

        {/* Search */}
        <div style={{ position: "relative", flex: 1, minWidth: 0, maxWidth: 200 }}>
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search..."
            style={{
              width: "100%", padding: "9px 12px 9px 34px", fontSize: 13,
              border: "1px solid var(--border)", borderRadius: 10,
              background: "var(--bg-surface)", color: "var(--text-primary)", outline: "none",
            }}
          />
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ position: "absolute", left: 12, top: "50%", transform: "translateY(-50%)" }}>
            <circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
        </div>

        {/* Refresh */}
        <button type="button" onClick={load} title="Refresh" style={{
          padding: "9px 10px", fontSize: 13, background: "var(--bg-surface)",
          color: "var(--text-secondary)", border: "1px solid var(--border)",
          borderRadius: 10, cursor: "pointer", display: "flex", alignItems: "center",
        }}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="23 4 23 10 17 10" /><polyline points="1 20 1 14 7 14" />
            <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" />
          </svg>
        </button>

        {/* New */}
        <button type="button" onClick={() => router.push(tab.newRoute)} style={{
          padding: "9px 20px", fontSize: 13, fontWeight: 500,
          background: "var(--accent)", color: "#fff", border: "none",
          borderRadius: 10, cursor: "pointer", display: "flex", alignItems: "center", gap: 6, whiteSpace: "nowrap",
        }}>
          + New
        </button>
        </>)}
      </div>

      <div style={{ height: 1, background: "var(--border)", marginBottom: 4 }} />

      {/* Lineage — custom full-width visualization */}
      {activeTab === "lineage" && <LineageView />}

      {/* Loading — also covers the gap right after a tab switch, before the new
          tab's data has arrived, so the previous tab never flashes through. */}
      {activeTab !== "lineage" && (loading || (!error && !ready)) && (
        <KaveonLoading message="Loading library" fullScreen={false} />
      )}

      {/* Error */}
      {activeTab !== "lineage" && !loading && error && (
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
      {activeTab !== "lineage" && ready && !error && filtered.length === 0 && (
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

      {/* Pinned — user's pinned items surface at the top (persisted per user) */}
      {ready && !error && pinned.length > 0 && (
        <div style={{ marginTop: 20, marginBottom: 8 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "12px 12px", borderBottom: "1px solid var(--border)" }}>
            <i className="fas fa-thumbtack" style={{ fontSize: 12, color: "#fbbf24" }} />
            <span style={{ fontSize: 14, fontWeight: 600, color: "var(--text-primary)" }}>Pinned</span>
            <span style={{ fontSize: 12, color: "var(--text-muted)", fontWeight: 500 }}>{pinned.length}</span>
          </div>
          {TAB_LAYOUT[activeTab] === "rows" ? (
            <div style={{ marginTop: 16, display: "flex", flexDirection: "column", gap: 8 }}>
              {pinned.map((item) => renderRow(item))}
            </div>
          ) : (
            <div style={{ ...gridStyle, marginTop: 16 }}>
              {pinned.map((item) => renderCard(item, NEUTRAL_ACCENT))}
            </div>
          )}
        </div>
      )}

      {/* The dashboard list is what makes dashboard sections possible. If it
          cannot be read, say so and show dataset sections instead. */}
      {activeTab === "charts" && chartGrouping === "dashboard" && placementsFailed && (
        <div style={{
          marginTop: 16, padding: "10px 14px", borderRadius: 10,
          border: "1px solid var(--border)", background: "var(--bg-surface)",
          display: "flex", alignItems: "center", gap: 12, flexWrap: "wrap",
          fontSize: 12.5, color: "var(--text-secondary)",
        }}>
          <span>Dashboards could not be read, so these charts are grouped by dataset.</span>
          <button type="button" onClick={() => setPlacementsFailed(false)} style={{
            padding: "5px 12px", fontSize: 12, border: "1px solid var(--border)",
            borderRadius: 8, background: "var(--bg-elevated)",
            color: "var(--text-secondary)", cursor: "pointer",
          }}>Try again</button>
        </div>
      )}

      {/* Items — dense rows for datasets/queries; card grid (grouped for charts) otherwise */}
      {ready && !error && filtered.length > 0 && (
        TAB_LAYOUT[activeTab] === "rows" ? (
          <div style={{ marginTop: 20, display: "flex", flexDirection: "column", gap: 8 }}>
            {unpinned.map((item) => renderRow(item))}
          </div>
        ) : useSections ? (
          <div style={{ marginTop: 20, display: "flex", flexDirection: "column", gap: 10 }}>
            {grouped.map((g) => {
              // Collapsed by default; searching force-expands so matches are visible.
              const open = !!search || expandedGroups.has(g.key);
              return (
                <div key={g.key || "all"}>
                  {/* Section header — click to expand/collapse */}
                  <button
                    type="button"
                    onClick={() => toggleGroup(g.key)}
                    style={{
                      width: "100%", display: "flex", alignItems: "center", gap: 10,
                      padding: "12px 12px", background: "transparent", border: "none",
                      borderBottom: "1px solid var(--border)", cursor: "pointer", textAlign: "left",
                    }}
                    onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-hover)")}
                    onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
                  >
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round"
                      style={{ flexShrink: 0, transform: open ? "rotate(90deg)" : "rotate(0deg)", transition: "transform 0.15s" }}>
                      <polyline points="9 18 15 12 9 6" />
                    </svg>
                    <span style={{ width: 9, height: 9, borderRadius: "50%", background: g.accent, flexShrink: 0 }} />
                    <span style={{ fontSize: 14, fontWeight: 600, color: "var(--text-primary)" }}>{g.key || "Ungrouped"}</span>
                    <span style={{ fontSize: 12, color: "var(--text-muted)", fontWeight: 500 }}>{g.items.length}</span>
                  </button>
                  {open && (
                    <div style={{ ...gridStyle, marginTop: 16, marginBottom: 8 }}>
                      {g.items.map((item) => renderCard(item, g.accent))}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        ) : (
          <div style={{ ...gridStyle, marginTop: 20 }}>
            {grouped[0]?.items.map((item) => renderCard(item, grouped[0].accent))}
          </div>
        )
      )}

      {/* Delete confirmation — card overlay */}
      {confirmItem && (
        <div
          onClick={() => { if (!deletingId) setConfirmItem(null); }}
          style={{
            position: "fixed", inset: 0, zIndex: 1000,
            background: "rgba(15, 23, 42, 0.45)", backdropFilter: "blur(2px)",
            display: "flex", alignItems: "center", justifyContent: "center", padding: 20,
          }}
        >
          <div
            onClick={(e) => e.stopPropagation()}
            style={{
              width: "100%", maxWidth: 400, background: "var(--bg-surface)",
              border: "1px solid var(--border)", borderRadius: 16,
              boxShadow: "0 24px 48px rgba(0,0,0,0.28)", padding: 24,
              animation: "workspaceModalIn 0.14s ease-out",
            }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 14 }}>
              <div style={{
                width: 40, height: 40, borderRadius: 10, flexShrink: 0,
                background: "rgba(220,38,38,0.1)", display: "flex", alignItems: "center", justifyContent: "center",
              }}>
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#dc2626" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <polyline points="3 6 5 6 21 6" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /><line x1="10" y1="11" x2="10" y2="17" /><line x1="14" y1="11" x2="14" y2="17" />
                </svg>
              </div>
              <h3 style={{ margin: 0, fontSize: 17, fontWeight: 600, color: "var(--text-primary)" }}>
                Delete {tab.label.toLowerCase().replace(/s$/, "")}?
              </h3>
            </div>
            <p style={{ margin: "0 0 20px", fontSize: 14, lineHeight: 1.5, color: "var(--text-secondary)" }}>
              &ldquo;<span style={{ fontWeight: 600, color: "var(--text-primary)" }}>{confirmItem.name ?? confirmItem.title ?? "Untitled"}</span>&rdquo; will be permanently removed. This can&apos;t be undone.
            </p>
            {deleteError && (
              <div style={{ marginBottom: 14, fontSize: 13, color: "#dc2626" }}>{deleteError}</div>
            )}
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
              <button
                type="button"
                onClick={() => setConfirmItem(null)}
                disabled={!!deletingId}
                style={{
                  padding: "9px 18px", fontSize: 13, fontWeight: 500, cursor: deletingId ? "default" : "pointer",
                  border: "1px solid var(--border)", borderRadius: 10,
                  background: "var(--bg-surface)", color: "var(--text-secondary)",
                }}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={confirmDelete}
                disabled={!!deletingId}
                style={{
                  padding: "9px 18px", fontSize: 13, fontWeight: 600, cursor: deletingId ? "default" : "pointer",
                  border: "none", borderRadius: 10, background: "#dc2626", color: "#fff",
                  display: "flex", alignItems: "center", gap: 8, minWidth: 88, justifyContent: "center",
                }}
              >
                {deletingId ? <div className="spinner" style={{ width: 13, height: 13, borderWidth: 2, borderTopColor: "#fff" }} /> : "Delete"}
              </button>
            </div>
          </div>
        </div>
      )}

      <style jsx>{`
        @keyframes workspaceModalIn {
          from { opacity: 0; transform: translateY(8px) scale(0.98); }
          to   { opacity: 1; transform: translateY(0)   scale(1); }
        }
        @media (max-width: 768px) {
          .workspace-root {
            padding-left: 16px !important;
            padding-right: 16px !important;
          }
        }
      `}</style>
    </div>
  );
}
