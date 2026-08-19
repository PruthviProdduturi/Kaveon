"use client";

import { useCallback, useEffect, useState } from "react";
import { msalFetch } from "../utils/msalFetch";

/**
 * Dataset-page "Context" panel — the compiled DLM for this dataset: when it was
 * last generated, how long that took (and why), what got indexed, and a button
 * to regenerate. Transparent about the one-time generation cost so the instant
 * answer-from-context tradeoff is clear.
 */

interface DLM {
  status?: string;
  built_at?: string;
  values_indexed?: number;
  manifest?: { columns?: { name?: string; is_dimension?: boolean; is_metric?: boolean }[]; metrics?: { name?: string }[] };
  stats_rollup?: {
    generation?: { duration_ms?: number; built_at?: string; answers_precomputed?: number; values_indexed?: number; rows_scanned?: number; scans?: number };
    date_range?: { min?: string; max?: string };
  };
}

function fmtMs(ms?: number): string {
  if (ms == null) return "—";
  return ms >= 1000 ? (ms / 1000).toFixed(1) + "s" : ms + "ms";
}
function compact(n?: number | null): string {
  if (n == null) return "—";
  const a = Math.abs(n);
  if (a >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (a >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
}
function ago(iso?: string): string {
  if (!iso) return "never";
  const t = Date.parse(iso.endsWith("Z") ? iso : iso + "Z");
  if (Number.isNaN(t)) return iso;
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function DatasetContextPanel({ datasetId }: { datasetId?: string }) {
  const [dlm, setDlm] = useState<DLM | null>(null);
  const [state, setState] = useState<"loading" | "none" | "ready" | "generating">("loading");

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

  const card: React.CSSProperties = { flexShrink: 0, border: "1px solid var(--border)", borderRadius: 12, padding: 20 };
  const h3: React.CSSProperties = { fontSize: 13, fontWeight: 700, color: "var(--text-muted)", margin: 0, textTransform: "uppercase", letterSpacing: "0.05em", display: "flex", alignItems: "center", gap: 8 };
  const chip: React.CSSProperties = { display: "inline-block", padding: "2px 9px", borderRadius: 999, marginRight: 5, marginBottom: 5, background: "rgba(var(--accent-rgb),0.08)", color: "var(--text-primary)", fontSize: 12 };
  const stat = (label: string, value: React.ReactNode) => (
    <div><div style={{ fontSize: 11, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.04em" }}>{label}</div><div style={{ fontSize: 15, fontWeight: 700, color: "var(--text-primary)", marginTop: 2 }}>{value}</div></div>
  );

  const genBtn = (
    <button
      onClick={generate}
      disabled={state === "generating" || state === "loading"}
      className="card"
      style={{ marginLeft: "auto", padding: "6px 14px", background: "var(--accent)", color: "#fff", border: "none", borderRadius: 8, cursor: state === "generating" ? "default" : "pointer", fontSize: 12.5, fontWeight: 600, opacity: state === "generating" ? 0.7 : 1, display: "flex", alignItems: "center", gap: 6 }}
    >
      <i className={`fas ${state === "generating" ? "fa-spinner fa-spin" : "fa-bolt"}`} style={{ fontSize: 11 }} />
      {state === "generating" ? "Generating…" : dlm ? "Regenerate context" : "Generate context"}
    </button>
  );

  return (
    <div className="card" style={card}>
      <div style={{ display: "flex", alignItems: "center", marginBottom: 14 }}>
        <h3 style={h3}><i className="fas fa-bolt" style={{ fontSize: 12, color: "var(--accent)", opacity: 0.8 }} /> Context (DLM)</h3>
        {genBtn}
      </div>

      {state === "loading" && <div style={{ fontSize: 13, color: "var(--text-muted)" }}>Loading…</div>}

      {(state === "none") && (
        <div style={{ fontSize: 13, color: "var(--text-muted)" }}>
          No context compiled yet. Generate it to answer questions about this dataset instantly, with no database scan.
        </div>
      )}

      {(state === "ready" || state === "generating") && dlm && (
        <>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(4, 1fr)", gap: 14, marginBottom: 14 }}>
            {stat("Last generated", ago(gen?.built_at || dlm.built_at))}
            {stat("Took", fmtMs(gen?.duration_ms))}
            {stat("Indexed values", compact(dlm.values_indexed))}
            {stat("Precomputed answers", compact(gen?.answers_precomputed))}
          </div>

          <div style={{ fontSize: 12.5, color: "var(--text-secondary)", background: "rgba(var(--accent-rgb),0.05)", border: "1px solid var(--border)", borderRadius: 8, padding: "9px 12px", marginBottom: 14 }}>
            <i className="fas fa-circle-info" style={{ color: "var(--accent)", marginRight: 7 }} />
            Generation scanned {compact(gen?.rows_scanned)} rows across {gen?.scans ?? "—"} passes ({fmtMs(gen?.duration_ms)}).
            That&apos;s a <b>one-time cost</b> — every matching question afterwards is answered <b>instantly from context, with no database scan</b>.
            It runs longer here because of the shared Postgres tier; a dedicated context tier makes it fast.
          </div>

          {dims.length > 0 && (
            <div style={{ marginBottom: 10 }}>
              <div style={{ fontSize: 11, fontWeight: 700, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.04em", marginBottom: 6 }}>Indexed dimensions ({dims.length})</div>
              <div>{dims.map((d) => <span key={d} style={chip}>{d}</span>)}</div>
            </div>
          )}
          {metrics.length > 0 && (
            <div>
              <div style={{ fontSize: 11, fontWeight: 700, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.04em", marginBottom: 6 }}>Precomputed metrics ({metrics.length})</div>
              <div>{metrics.map((m) => <span key={m} style={chip}>{m}</span>)}</div>
            </div>
          )}
        </>
      )}
    </div>
  );
}
