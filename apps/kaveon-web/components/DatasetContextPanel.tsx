"use client";

import { useCallback, useEffect, useState } from "react";
import { msalFetch } from "../utils/msalFetch";

interface DLM {
  status?: string;
  built_at?: string;
  values_indexed?: number;
  manifest?: { columns?: { name?: string; is_dimension?: boolean; is_metric?: boolean }[]; metrics?: { name?: string }[] };
  stats_rollup?: {
    generation?: { duration_ms?: number; built_at?: string; answers_precomputed?: number; values_indexed?: number; rows_scanned?: number; scans?: number };
    date_range?: { min?: string; max?: string };
    row_counts?: Record<string, number>;
  };
}

function fmtMs(ms?: number): string {
  if (ms == null) return "—";
  return ms >= 1000 ? (ms / 1000).toFixed(1) + "s" : ms + "ms";
}
function compact(n?: number | null): string {
  if (n == null) return "—";
  const a = Math.abs(n);
  if (a >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + "B";
  if (a >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (a >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
}
function fmtBuilt(iso?: string): string {
  if (!iso) return "never";
  const cleaned = iso.replace(/\+00:00$/, "Z").replace(/(\.\d{3})\d*Z$/, "$1Z");
  const d = new Date(cleaned);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, { month: "short", day: "numeric", year: "numeric", hour: "numeric", minute: "2-digit" });
}
function comboCount(d: number): number {
  return d * (d - 1) / 2;
}

export function DatasetContextPanel({ datasetId }: { datasetId?: string }) {
  const [dlm, setDlm] = useState<DLM | null>(null);
  const [state, setState] = useState<"loading" | "none" | "ready" | "generating">("loading");
  const [showConfirm, setShowConfirm] = useState(false);

  const load = useCallback(async () => {
    if (!datasetId) return;
    try {
      const res = await msalFetch(`/api/v1/datasets/${datasetId}/dlm`);
      if (res.ok) { setDlm(await res.json()); setState("ready"); }
      else { setDlm(null); setState("none"); }
    } catch { setDlm(null); setState("none"); }
  }, [datasetId]);

  useEffect(() => { load(); }, [load]);

  const generate = useCallback(async () => {
    if (!datasetId) return;
    setShowConfirm(false);
    setState("generating");
    try {
      await msalFetch(`/api/v1/datasets/${datasetId}/dlm/generate?force=true`, { method: "POST" });
    } catch { /* ignore */ }
    await load();
  }, [datasetId, load]);

  const gen = dlm?.stats_rollup?.generation;
  const cols = dlm?.manifest?.columns ?? [];
  const dims = cols.filter((c) => c.is_dimension).map((c) => c.name).filter(Boolean) as string[];
  const metrics = (dlm?.manifest?.metrics ?? []).map((m) => m.name).filter(Boolean) as string[];
  const hasContext = state === "ready" || state === "generating";

  const rowCounts = dlm?.stats_rollup?.row_counts || {};
  const maxRows = Math.max(0, ...Object.values(rowCounts));
  const dateRange = dlm?.stats_rollup?.date_range;
  const estimatedAnswers = metrics.length * (1 + dims.length + comboCount(dims.length));

  const handleRegenClick = () => {
    if (hasContext) {
      setShowConfirm(true);
    } else {
      generate();
    }
  };

  const chip: React.CSSProperties = { display: "inline-block", padding: "3px 10px", borderRadius: 6, marginRight: 6, marginBottom: 6, background: "rgba(var(--accent-rgb),0.08)", color: "var(--text-primary)", fontSize: 12, fontWeight: 500 };
  const statCell: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 2, minWidth: 100 };
  const statLabel: React.CSSProperties = { fontSize: 11, color: "var(--text-muted)", fontWeight: 500, textTransform: "uppercase" as const, letterSpacing: "0.04em" };
  const statValue: React.CSSProperties = { fontSize: 18, fontWeight: 700, color: "var(--text-primary)" };

  return (
    <>
      <div className="card" style={{ flexShrink: 0, border: "1px solid var(--border)", borderRadius: 12, padding: "16px 20px" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <i className="fas fa-bolt" style={{ fontSize: 12, color: "var(--accent)" }} />
          <span style={{ fontSize: 13, fontWeight: 700, color: "var(--text-primary)" }}>
            Context
          </span>
          {hasContext && dlm && (
            <span style={{ fontSize: 12, color: "var(--text-muted)", fontWeight: 400 }}>
              Built {fmtBuilt(gen?.built_at || dlm.built_at)} · {compact(gen?.values_indexed ?? dlm.values_indexed)} values · {compact(gen?.answers_precomputed)} precomputed answers
            </span>
          )}
          <button
            onClick={handleRegenClick}
            disabled={state === "generating" || state === "loading"}
            style={{ marginLeft: "auto", padding: "5px 12px", background: "var(--accent)", color: "#fff", border: "none", borderRadius: 7, cursor: state === "generating" ? "default" : "pointer", fontSize: 12, fontWeight: 600, opacity: state === "generating" ? 0.7 : 1, display: "flex", alignItems: "center", gap: 5 }}
          >
            <i className={`fas ${state === "generating" ? "fa-spinner fa-spin" : "fa-bolt"}`} style={{ fontSize: 10 }} />
            {state === "generating" ? "Generating…" : hasContext ? "Regenerate" : "Generate"}
          </button>
        </div>

        {state === "loading" && <div style={{ fontSize: 13, color: "var(--text-muted)", marginTop: 10 }}>Loading…</div>}

        {state === "none" && (
          <div style={{ fontSize: 13, color: "var(--text-muted)", marginTop: 10 }}>
            No context yet. Generate to enable instant answers from precomputed results — no live database query needed.
          </div>
        )}

        {hasContext && dlm && (dims.length > 0 || metrics.length > 0) && (
          <div style={{ marginTop: 12, display: "flex", flexWrap: "wrap", gap: 4 }}>
            {dims.map((d) => <span key={d} style={chip}>{d}</span>)}
            {metrics.map((m) => <span key={m} style={{ ...chip, background: "rgba(var(--success-rgb, 34,197,94),0.08)", color: "var(--success)" }}>{m}</span>)}
          </div>
        )}
      </div>

      {showConfirm && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 9999, display: "flex", alignItems: "center", justifyContent: "center", background: "rgba(0,0,0,0.45)", backdropFilter: "blur(4px)" }}
          onClick={() => setShowConfirm(false)}
        >
          <div
            className="card"
            style={{ width: 520, maxWidth: "92vw", borderRadius: 16, border: "1px solid var(--border)", background: "var(--bg-surface)", padding: "28px 32px", boxShadow: "0 20px 60px rgba(0,0,0,0.3)" }}
            onClick={(e) => e.stopPropagation()}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 20 }}>
              <div style={{ width: 36, height: 36, borderRadius: 10, background: "rgba(var(--accent-rgb),0.12)", display: "flex", alignItems: "center", justifyContent: "center" }}>
                <i className="fas fa-bolt" style={{ fontSize: 16, color: "var(--accent)" }} />
              </div>
              <div>
                <div style={{ fontSize: 16, fontWeight: 700, color: "var(--text-primary)" }}>Regenerate Context</div>
                <div style={{ fontSize: 12, color: "var(--text-muted)" }}>This will recompute all precomputed answers from live data</div>
              </div>
            </div>

            <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 16, padding: "16px 0", borderTop: "1px solid var(--border)", borderBottom: "1px solid var(--border)" }}>
              <div style={statCell}>
                <span style={statLabel}>Rows</span>
                <span style={statValue}>{compact(maxRows || gen?.rows_scanned)}</span>
              </div>
              <div style={statCell}>
                <span style={statLabel}>Columns</span>
                <span style={statValue}>{cols.length}</span>
              </div>
              <div style={statCell}>
                <span style={statLabel}>Dimensions</span>
                <span style={statValue}>{dims.length}</span>
              </div>
              <div style={statCell}>
                <span style={statLabel}>Metrics</span>
                <span style={statValue}>{metrics.length}</span>
              </div>
              <div style={statCell}>
                <span style={statLabel}>Values Indexed</span>
                <span style={statValue}>{compact(gen?.values_indexed ?? dlm?.values_indexed)}</span>
              </div>
              <div style={statCell}>
                <span style={statLabel}>Answers</span>
                <span style={statValue}>{compact(estimatedAnswers)}</span>
              </div>
            </div>

            {dims.length > 0 && (
              <div style={{ marginTop: 14 }}>
                <div style={{ fontSize: 11, fontWeight: 600, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.04em", marginBottom: 6 }}>Breakdown</div>
                <div style={{ fontSize: 12, color: "var(--text-secondary)", lineHeight: 1.6 }}>
                  <span style={{ color: "var(--text-primary)", fontWeight: 600 }}>{metrics.length}</span> grand totals + <span style={{ color: "var(--text-primary)", fontWeight: 600 }}>{dims.length * metrics.length}</span> single-dim + <span style={{ color: "var(--text-primary)", fontWeight: 600 }}>{comboCount(dims.length) * metrics.length}</span> two-dim combos
                </div>
              </div>
            )}

            {dateRange?.min && (
              <div style={{ marginTop: 10, fontSize: 12, color: "var(--text-muted)" }}>
                Date range: {dateRange.min} — {dateRange.max}
              </div>
            )}

            {gen?.duration_ms != null && (
              <div style={{ marginTop: 6, fontSize: 12, color: "var(--text-muted)" }}>
                Last generation took {fmtMs(gen.duration_ms)}
              </div>
            )}

            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10, marginTop: 20 }}>
              <button
                onClick={() => setShowConfirm(false)}
                style={{ padding: "8px 20px", background: "transparent", color: "var(--text-secondary)", border: "1px solid var(--border)", borderRadius: 8, cursor: "pointer", fontSize: 13, fontWeight: 600 }}
              >
                Cancel
              </button>
              <button
                onClick={generate}
                style={{ padding: "8px 20px", background: "var(--accent)", color: "#fff", border: "none", borderRadius: 8, cursor: "pointer", fontSize: 13, fontWeight: 600, display: "flex", alignItems: "center", gap: 6 }}
              >
                <i className="fas fa-bolt" style={{ fontSize: 11 }} />
                Regenerate
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
