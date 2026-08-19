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
  const hasContext = state === "ready" || state === "generating";

  const chip: React.CSSProperties = { display: "inline-block", padding: "3px 10px", borderRadius: 6, marginRight: 6, marginBottom: 6, background: "rgba(var(--accent-rgb),0.08)", color: "var(--text-primary)", fontSize: 12, fontWeight: 500 };

  return (
    <div className="card" style={{ flexShrink: 0, border: "1px solid var(--border)", borderRadius: 12, padding: "16px 20px" }}>
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <i className="fas fa-bolt" style={{ fontSize: 12, color: "var(--accent)" }} />
        <span style={{ fontSize: 13, fontWeight: 700, color: "var(--text-primary)" }}>
          Context
        </span>
        {hasContext && dlm && (
          <span style={{ fontSize: 12, color: "var(--text-muted)", fontWeight: 400 }}>
            Built {ago(gen?.built_at || dlm.built_at)} · {compact(dlm.values_indexed)} values · {compact(gen?.answers_precomputed)} precomputed answers
          </span>
        )}
        <button
          onClick={generate}
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
  );
}
