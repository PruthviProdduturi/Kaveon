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

interface CoverageItem {
  dataset_id: string;
  name?: string | null;
  date_column?: string | null;
  date_range?: DateRange | null;
  row_count?: number | null;
  values_indexed?: number;
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
    display: "flex", alignItems: "center", justifyContent: "center", gap: 12,
    padding: "8px 40px",
    background: "var(--bg-surface)",
    borderBottom: "1px solid var(--border)",
    fontSize: 12.5, color: "var(--text-secondary)",
    overflowX: "auto", whiteSpace: "nowrap",
  };
  const label: React.CSSProperties = {
    display: "flex", alignItems: "center", gap: 6,
    fontWeight: 600, color: "var(--text-primary)", flexShrink: 0,
  };
  const pill: React.CSSProperties = {
    display: "inline-flex", alignItems: "center", gap: 8,
    padding: "3px 10px", borderRadius: 999,
    border: "1px solid var(--border)", background: "rgba(var(--accent-rgb),0.06)",
    flexShrink: 0,
  };
  const dot = (ok: boolean): React.CSSProperties => ({
    width: 6, height: 6, borderRadius: 999,
    background: ok ? "#22c55e" : "#f59e0b", flexShrink: 0,
  });
  const closeBtn: React.CSSProperties = {
    position: "absolute", right: 10, top: "50%", transform: "translateY(-50%)",
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
          <span key={d.dataset_id} style={pill} title={
            `${d.name ?? "dataset"}${d.metrics?.length ? " · metrics: " + d.metrics.join(", ") : ""}`
          }>
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
    </div>
  );
}
