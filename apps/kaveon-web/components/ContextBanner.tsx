"use client";

import { useEffect, useState } from "react";
import { msalFetch } from "../utils/msalFetch";

/**
 * Fixed "available context" banner — shows which datasets have a compiled DLM,
 * the date range each covers, row counts, and how many filter values are
 * indexed. Lets users see what they can ask about before they start testing.
 *
 * Reads GET /api/v1/dlm/coverage (no-LLM compiled artifacts). Renders nothing
 * until data resolves; shows a hint when no DLM has been generated yet.
 */

interface DateRange {
  column?: string | null;
  min?: string | null;
  max?: string | null;
  approx?: boolean;
}

interface DimInfo { column?: string | null; values?: string[]; }

interface CoverageItem {
  dataset_id: string;
  name?: string | null;
  date_column?: string | null;
  date_range?: DateRange | null;
  row_count?: number | null;
  values_indexed?: number;
  columns_count?: number;
  dimensions?: DimInfo[];
  metrics?: string[];
  status?: string;
  built_at?: string;
}

function compact(n: number | null | undefined): string {
  if (n == null) return "—";
  const a = Math.abs(n);
  if (a >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + "B";
  if (a >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (a >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
}

function shortDate(s?: string | null): string | null {
  if (!s) return null;
  // trust the first 10 chars for ISO-ish dates; otherwise show as-is (trimmed)
  const m = /^(\d{4}-\d{2}-\d{2})/.exec(String(s).trim());
  return m ? m[1] : String(s).trim().slice(0, 12);
}

export function ContextBanner() {
  const [items, setItems] = useState<CoverageItem[] | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [hover, setHover] = useState<{ item: CoverageItem; x: number; y: number } | null>(null);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const res = await msalFetch("/api/v1/dlm/coverage");
        if (!res.ok) { if (alive) setItems([]); return; }
        const data = await res.json();
        if (alive) setItems(Array.isArray(data?.datasets) ? data.datasets : []);
      } catch {
        if (alive) setItems([]);
      }
    })();
    return () => { alive = false; };
  }, []);

  if (items === null || dismissed) return null;

  const ready = items.filter((d) => d.status === "ready" || (d.values_indexed ?? 0) > 0 || d.date_range);

  const wrap: React.CSSProperties = {
    position: "sticky", top: 0, zIndex: 20,
    display: "flex", flexWrap: "wrap", alignItems: "center", justifyContent: "center",
    gap: 8,
    padding: "8px 44px",
    width: "100%", maxWidth: "100%", boxSizing: "border-box",
    background: "var(--bg-surface)",
    borderBottom: "1px solid var(--border)",
    fontSize: 12.5, color: "var(--text-secondary)",
  };
  const label: React.CSSProperties = {
    display: "flex", alignItems: "center", gap: 6,
    fontWeight: 600, color: "var(--text-primary)", flexShrink: 0,
  };
  const pill: React.CSSProperties = {
    display: "inline-flex", alignItems: "center", gap: 8,
    padding: "3px 10px", borderRadius: 999,
    border: "1px solid var(--border)", background: "rgba(var(--accent-rgb),0.06)",
    flexShrink: 0, whiteSpace: "nowrap", maxWidth: "100%",
  };
  const dot = (ok: boolean): React.CSSProperties => ({
    width: 6, height: 6, borderRadius: 999,
    background: ok ? "#22c55e" : "#f59e0b", flexShrink: 0,
  });
  const closeBtn: React.CSSProperties = {
    position: "absolute", right: 10, top: 8,
    flexShrink: 0,
    border: "none", background: "transparent", cursor: "pointer",
    color: "var(--text-secondary)", fontSize: 13, padding: 4,
  };

  if (ready.length === 0) {
    return (
      <div style={wrap}>
        <span style={label}><i className="fas fa-database" /> Context</span>
        <span>No context compiled yet — open a dataset and <b>Generate DLM</b> to make it queryable here.</span>
        <button style={closeBtn} title="Dismiss" onClick={() => setDismissed(true)}>
          <i className="fas fa-times" />
        </button>
      </div>
    );
  }

  return (
    <div style={wrap}>
      <span style={label}>
        <i className="fas fa-database" /> Context ready
      </span>
      {ready.map((d) => {
        const min = shortDate(d.date_range?.min);
        const max = shortDate(d.date_range?.max);
        return (
          <span
            key={d.dataset_id}
            style={{ ...pill, cursor: "help" }}
            onMouseEnter={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              setHover({ item: d, x: r.left, y: r.bottom + 6 });
            }}
            onMouseLeave={() => setHover(null)}
          >
            <span style={dot(d.status === "ready")} />
            <b style={{ color: "var(--text-primary)" }}>{d.name ?? `#${d.dataset_id}`}</b>
            {min && max && (
              <span>{min} → {max}{d.date_range?.approx ? "~" : ""}</span>
            )}
            <span>· {compact(d.row_count)} rows</span>
            {(d.values_indexed ?? 0) > 0 && <span>· {compact(d.values_indexed)} values</span>}
          </span>
        );
      })}
      <button style={closeBtn} title="Dismiss" onClick={() => setDismissed(true)}>
        <i className="fas fa-times" />
      </button>

      {hover && <HoverCard item={hover.item} x={hover.x} y={hover.y} />}
    </div>
  );
}

function HoverCard({ item, x, y }: { item: CoverageItem; x: number; y: number }) {
  const dr = item.date_range;
  const dims = (item.dimensions ?? []).filter((d) => (d.values?.length ?? 0) > 0);
  const card: React.CSSProperties = {
    position: "fixed", left: Math.min(x, (typeof window !== "undefined" ? window.innerWidth : 1200) - 340), top: y,
    zIndex: 60, width: 320, maxWidth: "92vw",
    background: "var(--bg-elevated, #1a1a1a)", border: "1px solid var(--border)",
    borderRadius: 10, boxShadow: "0 12px 32px rgba(0,0,0,0.4)",
    padding: 12, fontSize: 12, color: "var(--text-secondary)",
    textAlign: "left", whiteSpace: "normal", cursor: "default",
  };
  const chip: React.CSSProperties = {
    display: "inline-block", padding: "1px 7px", borderRadius: 999, marginRight: 4, marginBottom: 4,
    background: "rgba(var(--accent-rgb),0.1)", color: "var(--text-primary)", fontSize: 11,
  };
  const head: React.CSSProperties = { fontSize: 10.5, fontWeight: 700, textTransform: "uppercase", letterSpacing: 0.4, color: "var(--text-faint)", margin: "10px 0 4px" };
  return (
    <div style={card}>
      <div style={{ fontWeight: 700, color: "var(--text-primary)", fontSize: 13 }}>{item.name}</div>
      <div style={{ marginTop: 3, color: "var(--text-faint)" }}>
        {compact(item.row_count)} rows · {item.columns_count ?? "—"} columns · {compact(item.values_indexed)} indexed values
        {dr?.min && dr?.max ? ` · ${shortDate(dr.min)} → ${shortDate(dr.max)}` : ""}
      </div>

      {dims.length > 0 && (
        <>
          <div style={head}>Filter by</div>
          {dims.map((d) => (
            <div key={d.column} style={{ marginBottom: 5 }}>
              <span style={{ color: "var(--text-primary)", fontWeight: 600 }}>{d.column}: </span>
              <span>{(d.values ?? []).slice(0, 6).join(", ")}{(d.values?.length ?? 0) >= 6 ? "…" : ""}</span>
            </div>
          ))}
        </>
      )}

      {(item.metrics?.length ?? 0) > 0 && (
        <>
          <div style={head}>Metrics</div>
          <div>{item.metrics!.map((m) => <span key={m} style={chip}>{m}</span>)}</div>
        </>
      )}
    </div>
  );
}
