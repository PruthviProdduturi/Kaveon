"use client";

import { useCallback, useEffect, useId, useRef, useState } from "react";
import { msalFetch } from "../utils/msalFetch";
import { refusalMessage } from "../utils/demoQuota";
import type { DlmArtifactHandle } from "../hooks/useDlmArtifact";

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

/**
 * The Context card on the dataset page.
 *
 * The compiled artifact is read once for the whole page and handed in; this
 * card no longer fetches it a second time of its own.
 */
export function DatasetContextPanel({ datasetId, dlm, className }: {
  datasetId?: string;
  dlm: DlmArtifactHandle;
  className?: string;
}) {
  const [generating, setGenerating] = useState(false);
  const [showConfirm, setShowConfirm] = useState(false);
  /** What the platform said when it refused the generate. */
  const [refusal, setRefusal] = useState<string>("");

  const artifact = dlm.artifact;
  const state = generating ? "generating" : dlm.state;

  const generate = useCallback(async () => {
    if (!datasetId) return;
    setShowConfirm(false);
    setGenerating(true);
    setRefusal("");
    try {
      const r = await msalFetch(`/api/v1/datasets/${datasetId}/dlm/generate?force=true`, { method: "POST" });
      // The response was discarded. The route requires the Analyst role and
      // the demo deployment refuses every write, so a Viewer pressed
      // Generate, watched the spinner, and was returned to "Not built.
      // Generate to answer questions …" — the same sentence as before, as
      // though the build had simply produced nothing.
      if (!r.ok) {
        const body = await r.json().catch(() => null);
        setRefusal(refusalMessage(body, `The platform answered ${r.status}.`));
      }
    } catch {
      setRefusal("The platform could not be reached.");
    } finally {
      await dlm.reload();
      setGenerating(false);
    }
  }, [datasetId, dlm]);

  const dialogIds = useId();
  const dialogTitleId = `${dialogIds}-title`;
  const dialogRef = useRef<HTMLDivElement>(null);
  const dialogCancelRef = useRef<HTMLButtonElement>(null);

  /**
   * Dialog semantics for a hand-rolled overlay: it had none. Nothing told a
   * screen reader a dialog had opened, Escape did not dismiss it, focus stayed
   * on the Regenerate button behind the overlay — so Enter reopened it — and
   * Tab walked the page underneath.
   */
  useEffect(() => {
    if (!showConfirm) return;
    const opener = document.activeElement as HTMLElement | null;
    dialogCancelRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") { setShowConfirm(false); return; }
      if (e.key !== "Tab") return;
      const root = dialogRef.current;
      if (!root) return;
      const focusable = Array.from(root.querySelectorAll<HTMLElement>('button:not([disabled]), [href], [tabindex]:not([tabindex="-1"])'));
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || !root.contains(active))) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && (active === last || !root.contains(active))) { e.preventDefault(); first.focus(); }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      if (opener && document.body.contains(opener)) opener.focus();
    };
  }, [showConfirm]);

  const gen = artifact?.stats_rollup?.generation;
  const cols = artifact?.manifest?.columns ?? [];
  const dims = cols.filter((c) => c.is_dimension).map((c) => c.name).filter(Boolean) as string[];
  const metrics = (artifact?.manifest?.metrics ?? []).map((m) => m.name).filter(Boolean) as string[];
  const hasContext = (state === "ready" || state === "generating") && !!artifact;

  const rowCounts = artifact?.stats_rollup?.row_counts || {};
  const maxRows = Math.max(0, ...Object.values(rowCounts));
  const dateRange = artifact?.stats_rollup?.date_range;
  const estimatedAnswers = metrics.length * (1 + dims.length + comboCount(dims.length));

  const handleRegenClick = () => {
    if (hasContext) {
      setShowConfirm(true);
    } else {
      void generate();
    }
  };

  const chip: React.CSSProperties = { display: "inline-block", padding: "3px 10px", borderRadius: 6, background: "rgba(var(--accent-rgb),0.08)", color: "var(--text-primary)", fontSize: 12, fontWeight: 500 };
  const statCell: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 2, minWidth: 100 };
  const statLabel: React.CSSProperties = { fontSize: 11, color: "var(--text-muted)", fontWeight: 500, textTransform: "uppercase" as const, letterSpacing: "0.04em" };
  const statValue: React.CSSProperties = { fontSize: 20, fontWeight: 700, color: "var(--text-primary)" };

  return (
    <>
      <section className={className}>
        <div style={{ display: "flex", alignItems: "center", gap: 10, minHeight: 26 }}>
          <i className="fas fa-bolt" aria-hidden="true" style={{ width: 13, textAlign: "center", fontSize: 12, color: "var(--accent)" }} />
          <span style={{ fontSize: 13, fontWeight: 700, color: "var(--text-primary)", flexShrink: 0 }}>
            Context
          </span>
          {/* The card holds its place while the artifact is read, and says
              what it is reading — a mute placeholder bar here was the same
              shape as a card with nothing in it. */}
          {state === "loading" && (
            <span
              role="status"
              style={{ display: "inline-flex", alignItems: "center", gap: 7, minWidth: 0, fontSize: 12, color: "var(--text-secondary)" }}
            >
              <span aria-hidden="true" style={{ width: 6, height: 6, borderRadius: 999, background: "var(--accent)", flexShrink: 0 }} />
              Reading compiled context
            </span>
          )}
          {state !== "loading" && (
            <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 12, color: "var(--text-muted)" }}>
              {hasContext
                ? `Built ${fmtBuilt(gen?.built_at || artifact?.built_at)} · ${compact(gen?.values_indexed ?? artifact?.values_indexed)} values · ${compact(gen?.answers_precomputed)} precomputed answers`
                : "Not built. Generate to answer questions from precomputed results, with no live database query."}
            </span>
          )}
          <button
            type="button"
            onClick={handleRegenClick}
            disabled={state === "generating" || state === "loading"}
            style={{ marginLeft: "auto", flexShrink: 0, display: "inline-flex", alignItems: "center", gap: 6, height: 26, padding: "0 12px", background: "var(--accent)", color: "#fff", border: "none", borderRadius: 7, cursor: state === "generating" || state === "loading" ? "default" : "pointer", fontSize: 12, fontWeight: 600, opacity: state === "generating" || state === "loading" ? 0.6 : 1 }}
          >
            <i className={`fas ${state === "generating" ? "fa-spinner fa-spin" : "fa-bolt"}`} aria-hidden="true" style={{ fontSize: 10 }} />
            {state === "generating" ? "Generating" : hasContext ? "Regenerate" : "Generate"}
          </button>
        </div>

        {refusal && (
          <p role="alert" style={{ margin: "10px 0 0", fontSize: 12.5, lineHeight: 1.5, color: "var(--text-secondary)" }}>
            The context was not regenerated. {refusal}
          </p>
        )}

        {hasContext && (dims.length > 0 || metrics.length > 0) && (
          <div style={{ marginTop: 12, display: "flex", flexWrap: "wrap", gap: 6 }}>
            {dims.map((d) => <span key={d} style={chip}>{d}</span>)}
            {metrics.map((m) => <span key={m} style={{ ...chip, background: "rgba(var(--success-rgb, 34,197,94),0.08)", color: "var(--success)" }}>{m}</span>)}
          </div>
        )}
      </section>

      {showConfirm && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 9999, display: "flex", alignItems: "center", justifyContent: "center", background: "rgba(0,0,0,0.45)", backdropFilter: "blur(4px)" }}
          onClick={() => setShowConfirm(false)}
        >
          <div
            ref={dialogRef}
            role="dialog"
            aria-modal="true"
            aria-labelledby={dialogTitleId}
            style={{ width: 520, maxWidth: "92vw", maxHeight: "86vh", overflowY: "auto", borderRadius: 16, border: "1px solid var(--border)", background: "var(--bg-surface)", padding: "28px 32px", boxShadow: "var(--shadow-lg)" }}
            onClick={(e) => e.stopPropagation()}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 20 }}>
              <div style={{ width: 36, height: 36, borderRadius: 10, background: "rgba(var(--accent-rgb),0.12)", display: "flex", alignItems: "center", justifyContent: "center" }}>
                <i className="fas fa-bolt" aria-hidden="true" style={{ fontSize: 16, color: "var(--accent)" }} />
              </div>
              <div>
                <div id={dialogTitleId} style={{ fontSize: 16, fontWeight: 700, color: "var(--text-primary)" }}>Regenerate context</div>
                <div style={{ fontSize: 12, color: "var(--text-muted)" }}>Every precomputed answer is recomputed from live data</div>
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
                <span style={statLabel}>Values indexed</span>
                <span style={statValue}>{compact(gen?.values_indexed ?? artifact?.values_indexed)}</span>
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
            {!!gen?.skipped_breakdowns?.length && (
              <div style={{ marginTop: 8, fontSize: 12, color: "var(--text-secondary)", lineHeight: 1.5 }}>
                <div style={{ fontWeight: 600 }}>
                  {gen.skipped_breakdowns.length} breakdown{gen.skipped_breakdowns.length === 1 ? "" : "s"} not precomputed — questions on them run live
                </div>
                {gen.skipped_breakdowns.slice(0, 6).map(sk => (
                  <div key={sk.dimension || "totals"}>
                    <code style={{ fontSize: 11.5 }}>{sk.dimension || "grand totals"}</code> — {sk.reason}
                  </div>
                ))}
              </div>
            )}

            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10, marginTop: 20 }}>
              <button
                ref={dialogCancelRef}
                type="button"
                onClick={() => setShowConfirm(false)}
                style={{ padding: "8px 20px", background: "transparent", color: "var(--text-secondary)", border: "1px solid var(--border)", borderRadius: 8, cursor: "pointer", fontSize: 13, fontWeight: 600 }}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={generate}
                style={{ padding: "8px 20px", background: "var(--accent)", color: "#fff", border: "none", borderRadius: 8, cursor: "pointer", fontSize: 13, fontWeight: 600, display: "flex", alignItems: "center", gap: 6 }}
              >
                <i className="fas fa-bolt" aria-hidden="true" style={{ fontSize: 11 }} />
                Regenerate
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
