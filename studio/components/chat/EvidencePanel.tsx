"use client";

import { useState } from "react";
import { msalFetch } from "../../utils/msalFetch";
import { rateLimitNotice, refusalMessage } from "../../utils/demoQuota";

/** What a DLM answer carries about itself: the statement that ran (or would
 *  have), the dataset and its source, the source version the answer reflects,
 *  the lane it took, the Engine's execution record when there is one, and the
 *  block that reproduces it as a live read. Mirrors `evidence` on
 *  `POST /api/v1/dlm/ask` (docs/reference/api.md). */
export interface Evidence {
  sql: string;
  dataset: { id: string; name: string | null };
  source:
    | { kind: "engine"; table_id: string; catalog: string; schema: string; table: string }
    | { kind: "warehouse"; database: string; schema: string; table: string };
  source_version: Record<string, unknown> | null;
  lane: "context" | "cache" | "live";
  execution: { mode?: string; detail?: string; approximate?: { function: string; argument: string; sketch: string; error: number }[] } | null;
  settings: Record<string, unknown> | null;
  principal?: string;
  query_id: string | null;
  elapsed_ms: number | null;
  rows: number | null;
  reproduce: { sql: string; database: string; schema: string; engine: boolean; settings: Record<string, unknown> | null };
}

interface LiveRun {
  /** `quota`: the Engine refused the run on the demo's live-query quota. */
  state: "idle" | "running" | "done" | "failed" | "quota";
  headline?: number | string | null;
  elapsedMs?: number | null;
  rows?: number | null;
  detail?: string | null;
  message?: string;
}

const LANE_LABEL: Record<Evidence["lane"], string> = {
  context: "From context · no scan",
  cache: "From cache",
  live: "Live query",
};

function versionLabel(version: Record<string, unknown> | null): string {
  if (!version) return "unknown";
  const kind = String(version.kind ?? "");
  if (kind === "postgresql_change_counter") {
    const mods = version.mods_since_analyze;
    const rows = version.row_count;
    return `change counter · ${mods ?? "?"} modifications since analyze${rows != null ? ` · ${rows} rows` : ""}`;
  }
  if (kind === "unavailable") return "no change counter on this source";
  const identity = String(version.identity_sha256 ?? "");
  const extra = version.version != null ? ` v${version.version}` : version.snapshot_id != null ? ` snapshot ${version.snapshot_id}` : version.files != null ? ` · ${version.files} files` : "";
  return `${kind}${extra}${identity ? ` (${identity.slice(0, 12)})` : ""}`;
}

function sourceLabel(source: Evidence["source"]): string {
  if (source.kind === "engine") return `Engine · ${source.catalog}.${source.schema}.${source.table}`;
  return `Warehouse · ${source.database}.${source.schema}.${source.table}`;
}

export type Row = (string | number | null)[];

/** The answer's headline row: the one row of a KPI, or the top row of a
 *  ranking. Its last column is the number shown; the columns before it are
 *  the key the live run is matched on, since a reproduced grouped statement
 *  carries no ordering. */
export function headlineOf(rows: Row[] | undefined): Row | null {
  const first = rows?.[0];
  return first && first.length > 0 ? first : null;
}

function keyOf(row: Row): string {
  return row.slice(0, -1).map(v => String(v ?? "")).join("\u0000");
}

/** The live run's value for the headline's key — the same group when the
 *  answer is grouped, the single row when it is not. */
function matchLive(headline: Row | null, rows: Row[] | undefined): number | string | null {
  if (!rows || rows.length === 0) return null;
  if (!headline || headline.length <= 1) return rows[0][rows[0].length - 1] ?? null;
  const wanted = keyOf(headline);
  const match = rows.find(r => r.length === headline.length && keyOf(r) === wanted) ?? null;
  return match ? match[match.length - 1] : null;
}

function formatDelta(before: number | string | null | undefined, after: number | string | null | undefined): string {
  if (before == null || after == null) return "";
  const a = Number(before), b = Number(after);
  if (!Number.isFinite(a) || !Number.isFinite(b)) return String(before) === String(after) ? "identical" : "differs";
  if (a === b) return "identical";
  const delta = b - a;
  const pct = a !== 0 ? ` (${(delta / Math.abs(a) * 100).toFixed(2)}%)` : "";
  return `${delta > 0 ? "+" : ""}${delta.toLocaleString()}${pct}`;
}

function fmtMs(ms: number | null | undefined): string {
  if (ms == null) return "—";
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms)}ms`;
}

/** FastAPI's third `detail` shape: a list of `{msg, loc}` validation items.
 *  `refusalMessage` reads the string and the object shapes and falls through to
 *  its fallback on a list, so a refused body naming the field it refused came
 *  out as a generic line. The server's own message is composed here instead. */
function validationDetail(body: unknown): string | null {
  const detail = (body as { detail?: unknown } | null)?.detail;
  if (!Array.isArray(detail) || detail.length === 0) return null;
  const parts = detail.flatMap(item => {
    if (!item || typeof item !== "object") return [];
    const msg = (item as { msg?: unknown }).msg;
    if (typeof msg !== "string" || msg.length === 0) return [];
    const loc = (item as { loc?: unknown }).loc;
    const field = Array.isArray(loc)
      ? [...loc].reverse().find((part): part is string => typeof part === "string")
      : undefined;
    return [field ? `${field}: ${msg}` : msg];
  });
  return parts.length > 0 ? parts.join("; ") : null;
}

const row: React.CSSProperties = { display: "flex", gap: 10, padding: "3px 0", alignItems: "baseline" };
// Muted, not faint: these label the value beside them, so they have to be read
// rather than merely sensed. On the panel ground `--text-faint` measured
// 3.02:1 in light and 3.00:1 in dark, under the 4.5:1 body-text threshold;
// `--text-muted` measures 4.52:1 and 4.56:1 on the same two grounds.
const key: React.CSSProperties = { width: 84, flexShrink: 0, color: "var(--text-muted)", fontSize: 10.5, textTransform: "uppercase", letterSpacing: "0.06em" };
const val: React.CSSProperties = { color: "var(--text-secondary)", fontSize: 12, wordBreak: "break-word" };
const button: React.CSSProperties = { background: "none", border: "1px solid var(--border)", borderRadius: 6, cursor: "pointer", fontSize: 11, color: "var(--text-secondary)", padding: "3px 9px" };

export function EvidencePanel({ evidence, headline, canRunLive }: {
  evidence: Evidence;
  headline: Row | null;
  canRunLive: boolean;
}) {
  const headlineValue = headline ? headline[headline.length - 1] : null;
  const headlineKey = headline && headline.length > 1 ? headline.slice(0, -1).map(v => String(v ?? "")).join(", ") : null;
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const [live, setLive] = useState<LiveRun>({ state: "idle" });

  const copy = async () => {
    try { await navigator.clipboard.writeText(evidence.sql); setCopied(true); setTimeout(() => setCopied(false), 1400); } catch { /* clipboard unavailable */ }
  };

  const runLive = async () => {
    setLive({ state: "running" });
    try {
      const res = await msalFetch("/api/v1/dlm/reproduce", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ dataset_id: evidence.dataset.id, sql: evidence.reproduce.sql }),
      });
      const body = await res.json().catch(() => ({}));
      if (!res.ok) {
        const notice = rateLimitNotice(res.status, body);
        if (notice) {
          setLive({ state: "quota", message: notice });
          return;
        }
        // The server's own account first, in whichever of FastAPI's three
        // `detail` shapes it arrived in. Where it gave none, the status is
        // carried rather than a cause this page has not established.
        setLive({
          state: "failed",
          message: validationDetail(body)
            ?? refusalMessage(body, `The live run was refused (HTTP ${res.status}).`),
        });
        return;
      }
      setLive({
        state: "done",
        headline: matchLive(headline, body.rows),
        elapsedMs: body.evidence?.elapsed_ms ?? body.duration_ms,
        rows: body.evidence?.rows ?? (Array.isArray(body.rows) ? body.rows.length : null),
        detail: body.evidence?.execution?.detail ?? null,
      });
    } catch {
      setLive({ state: "failed", message: "The live run did not complete." });
    }
  };

  const approx = evidence.execution?.approximate;

  return (
    <div style={{ marginTop: 6 }}>
      <button
        type="button"
        onClick={() => setOpen(v => !v)}
        aria-expanded={open}
        // The only affordance that opens the panel, so it is read as a control:
        // `--text-faint` measured 2.95:1 on the message ground in light and
        // 3.27:1 in dark, both under the 4.5:1 threshold for text this size.
        // `--text-secondary` measures 7.34:1 and 6.38:1 there.
        style={{ background: "none", border: "none", cursor: "pointer", fontSize: 11, color: "var(--text-secondary)", padding: 0, display: "flex", alignItems: "center", gap: 4 }}
      >
        <span>{open ? "▾" : "▸"}</span>
        <span>Evidence</span>
      </button>
      {open && (
        <div style={{ marginTop: 6, padding: "10px 12px", borderRadius: 8, border: "1px solid var(--border)", background: "var(--bg-elevated)" }}>
          <div style={row}><span style={key}>Lane</span><span style={val}>{LANE_LABEL[evidence.lane]}{evidence.execution?.detail ? ` · ${evidence.execution.detail}` : ""}</span></div>
          <div style={row}><span style={key}>Source</span><span style={val}>{sourceLabel(evidence.source)}</span></div>
          <div style={row}><span style={key}>Version</span><span style={val}>{versionLabel(evidence.source_version)}</span></div>
          <div style={row}>
            <span style={key}>Run</span>
            <span style={val}>
              {fmtMs(evidence.elapsed_ms)}{evidence.rows != null ? ` · ${evidence.rows} ${evidence.rows === 1 ? "row" : "rows"}` : ""}
              {evidence.query_id ? ` · query ${evidence.query_id.slice(0, 8)}` : ""}
              {evidence.principal ? ` · as ${evidence.principal}` : ""}
            </span>
          </div>
          {approx && approx.length > 0 && (
            <div style={row}>
              <span style={key}>Estimate</span>
              <span style={val}>{approx.map(a => `${a.function}(${a.argument}) from ${a.sketch}, ±${(a.error * 100).toFixed(2)}%`).join("; ")}</span>
            </div>
          )}
          <div style={{ ...row, alignItems: "flex-start" }}>
            <span style={key}>SQL</span>
            <div style={{ flex: 1, minWidth: 0 }}>
              <pre style={{ margin: 0, whiteSpace: "pre-wrap", wordBreak: "break-word", fontSize: 11.5, lineHeight: 1.5, fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace", color: "var(--text-primary)" }}>{evidence.sql}</pre>
              <div style={{ display: "flex", gap: 8, marginTop: 8, alignItems: "center", flexWrap: "wrap" }}>
                <button type="button" onClick={copy} style={button}>{copied ? "Copied" : "Copy SQL"}</button>
                {canRunLive && (
                  <button type="button" onClick={runLive} disabled={live.state === "running"} style={{ ...button, opacity: live.state === "running" ? 0.6 : 1 }}
                    title={evidence.reproduce.engine ? "Re-run on the Engine with use_statistics = false and result_cache = false" : "Re-run on the warehouse"}>
                    {live.state === "running" ? "Running live…" : "Run live"}
                  </button>
                )}
                {live.state === "done" && (
                  <span style={{ fontSize: 11.5, color: "var(--text-secondary)" }}>
                    {/* A reproduced grouped statement carries no ordering, so the
                        live rows are matched on the headline's key. No match is
                        not a value of zero and not an empty answer — it is a
                        comparison that could not be made, and an em dash beside
                        "vs 1,234 here" read as the live run disagreeing. */}
                    {live.headline == null ? (
                      <>The live run returned no row matching {headlineKey ? headlineKey : "this answer"}, so there is nothing to compare.</>
                    ) : (
                      <>
                        Live{headlineKey ? ` for ${headlineKey}` : ""}: <strong style={{ color: "var(--text-primary)" }}>{String(live.headline)}</strong>
                        {headlineValue != null && <span> · vs {String(headlineValue)} here · {formatDelta(headlineValue, live.headline)}</span>}
                      </>
                    )}
                    <span> · {fmtMs(live.elapsedMs)}{live.rows != null ? ` · ${live.rows} ${live.rows === 1 ? "row" : "rows"}` : ""}{live.detail ? ` · ${live.detail}` : ""}</span>
                  </span>
                )}
                {live.state === "quota" && <span style={{ fontSize: 11.5, color: "var(--text-secondary)" }}>{live.message}</span>}
                {/* `--danger` rather than a literal: #ef4444 measured 3.76:1 on
                    the panel in light and 4.07:1 in dark, both under 4.5:1,
                    and followed neither theme. The token measures 4.83:1 and
                    5.54:1 on the same two grounds. */}
                {live.state === "failed" && <span style={{ fontSize: 11.5, color: "var(--danger)" }}>{live.message}</span>}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
