"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { msalFetch } from "../utils/msalFetch";
import { refusalMessage } from "../utils/demoQuota";
import { unknownText } from "./Reading";
import type { DlmArtifactHandle } from "../hooks/useDlmArtifact";

/**
 * Dataset-page "Customize" editor — the human-editable side of the DLM's
 * per-dataset context spec. The DLM suggests defaults at generate time (aliases,
 * additivity, which breakdowns to precompute); here the user reviews and overrides
 * them. Alias / display / default / value-alias edits go live on save; breakdown or
 * depth changes prompt a regenerate (they change what is precomputed).
 *
 * The spec is read at mount, in parallel with everything else the page reads.
 * Where the spec is still empty the defaults come from the page's single read
 * of the compiled artifact, rather than from a second fetch of it here.
 */

interface MetricSpec { display_name?: string; aliases?: string[]; additive?: boolean; default?: boolean; hidden?: boolean; }
interface DimSpec { display_name?: string; aliases?: string[]; precompute?: boolean; top_n?: number; hidden?: boolean; }
interface Spec { metrics?: Record<string, MetricSpec>; dimensions?: Record<string, DimSpec>; value_aliases?: Record<string, string>; default_metric?: string; }
/**
 * The curation record as it is stored, with the fields this form does not
 * model left opaque. `PUT .../dlm/context` **replaces** the whole record
 * (`UPDATE dlm_artifact SET curation = …` in `api/dlm/engine.py`), so anything
 * this form omits from the payload is deleted — a curated `freshness_policy`
 * and a metric's `approximate` flag were both destroyed on every save, because
 * the form reads neither. It is kept here and merged back under the form's own
 * values rather than dropped.
 */
type StoredCuration = Record<string, unknown> & {
  metrics?: Record<string, Record<string, unknown>>;
  dimensions?: Record<string, Record<string, unknown>>;
  default_metric?: string;
};
interface ContextResp { ok?: boolean; dataset_name?: string; effective?: Spec; curation?: StoredCuration; }

const sub: React.CSSProperties = { fontSize: 11, fontWeight: 700, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.05em", margin: "18px 0 8px" };
const label: React.CSSProperties = { fontSize: 11, color: "var(--text-muted)", minWidth: 70 };
const input: React.CSSProperties = { border: "1px solid var(--border)", borderRadius: 7, padding: "4px 8px", fontSize: 12.5, background: "var(--bg-surface)", color: "var(--text-primary)" };
const rowCard: React.CSSProperties = { border: "1px solid var(--border)", borderRadius: 9, padding: "10px 12px", marginBottom: 8, display: "flex", flexDirection: "column", gap: 7 };

function Toggle({ on, onClick, children }: { on?: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button type="button" onClick={onClick}
      style={{ display: "inline-flex", alignItems: "center", gap: 6, padding: "3px 9px", borderRadius: 999, cursor: "pointer",
        border: "1px solid var(--border)", fontSize: 11.5, fontWeight: 600,
        background: on ? "rgba(var(--accent-rgb),0.12)" : "transparent", color: on ? "var(--accent)" : "var(--text-muted)" }}>
      <i className={`fas ${on ? "fa-check" : "fa-minus"}`} aria-hidden="true" style={{ fontSize: 9 }} />{children}
    </button>
  );
}

function Aliases({ items, onChange }: { items: string[]; onChange: (v: string[]) => void }) {
  const [t, setT] = useState("");
  const add = () => { const v = t.trim().toLowerCase(); if (v && !items.includes(v)) onChange([...items, v]); setT(""); };
  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: 5, alignItems: "center", flex: 1 }}>
      {items.map((a) => (
        <span key={a} style={{ display: "inline-flex", alignItems: "center", gap: 5, padding: "2px 8px", borderRadius: 999, background: "rgba(var(--accent-rgb),0.08)", fontSize: 12, color: "var(--text-primary)" }}>
          {/* Removing an alias was a click target on an <i> — no tab stop, no
              key handler, nothing announced. A keyboard or screen-reader user
              could add aliases and never remove one. */}
          {a}<button type="button" onClick={() => onChange(items.filter((x) => x !== a))}
            aria-label={`Remove alias ${a}`}
            style={{ display: "inline-flex", alignItems: "center", padding: 0, border: 0, background: "transparent", cursor: "pointer", opacity: 0.55, color: "inherit", fontSize: 10, lineHeight: 1 }}>
            <i className="fas fa-xmark" aria-hidden="true" />
          </button>
        </span>
      ))}
      <input value={t} onChange={(e) => setT(e.target.value)}
        onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); add(); } }}
        placeholder="add alias"
        style={{ border: "none", outline: "none", background: "transparent", fontSize: 12, minWidth: 90, color: "var(--text-primary)" }} />
    </div>
  );
}

export function DatasetContextEditor({ datasetId, dlm, className }: {
  datasetId?: string;
  dlm: DlmArtifactHandle;
  className?: string;
}) {
  const [name, setName] = useState<string>("");
  const [metrics, setMetrics] = useState<Record<string, MetricSpec>>({});
  const [dims, setDims] = useState<Record<string, DimSpec>>({});
  const [valiases, setValiases] = useState<Array<[string, string]>>([]);
  const [dflt, setDflt] = useState<string>("");
  const [ready, setReady] = useState(false);
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState<string>("");
  const [needsRegen, setNeedsRegen] = useState(false);
  /** A read that did not answer, as distinct from a dataset with nothing to curate. */
  const [readFailure, setReadFailure] = useState<string>("");
  const stored = useRef<StoredCuration>({});

  const load = useCallback(async () => {
    if (!datasetId) return;
    try {
      const r = await msalFetch(`/api/v1/datasets/${datasetId}/dlm/context`);
      if (!r.ok) {
        setReady(false);
        // 404 is the route's answer for "no artifact has been compiled yet"
        // (`reason: no_artifact`) — there is genuinely nothing to curate, and
        // the Context card above already says so. Any other status is a read
        // that did not answer, and hiding the whole control for it left the
        // Customize section simply missing with nothing said.
        setReadFailure(r.status === 404 ? "" : unknownText({
          what: "The context spec",
          says: "how this dataset's metrics and dimensions are curated",
          detail: `The platform answered ${r.status}.`,
        }));
        return;
      }
      const j: ContextResp = await r.json();
      const eff = j.effective || {};
      stored.current = j.curation || {};
      setName(j.dataset_name || "");
      setMetrics({ ...(eff.metrics || {}) });
      setDims({ ...(eff.dimensions || {}) });
      setValiases(Object.entries(eff.value_aliases || {}));
      // The effective spec always names a default metric — `_merge_spec` falls
      // back to the first one — so reading it from there and sending it back
      // froze a derived choice into explicit curation. Only a default the
      // record actually carries is a default the user chose.
      setDflt(stored.current.default_metric || "");
      setReadFailure("");
      setReady(true);
    } catch {
      setReady(false);
      setReadFailure(unknownText({
        what: "The context spec",
        says: "how this dataset's metrics and dimensions are curated",
      }));
    }
  }, [datasetId]);
  useEffect(() => { void load(); }, [load]);

  // Where the saved spec names no metrics or no dimensions, suggest them from
  // the compiled artifact the page already read.
  const artifact = dlm.artifact;
  useEffect(() => {
    if (!ready || !artifact) return;
    const manifest = artifact.manifest || {};
    setMetrics((current) => {
      if (Object.keys(current).length > 0 || !manifest.metrics) return current;
      const next: Record<string, MetricSpec> = {};
      for (const m of manifest.metrics) {
        const n = m.name || m.metric_name;
        if (!n) continue;
        const expr = (m.expression || "").toLowerCase();
        // COUNT DISTINCT / AVG / MAX / MIN are not additive.
        const isAdditive = !/(count_distinct|count\s*\(\s*distinct|avg\(|max\(|min\(|active|unique|distinct)/i.test(`${expr} ${n}`);
        next[n] = { display_name: n, aliases: [], additive: isAdditive, default: false, hidden: false };
      }
      return Object.keys(next).length > 0 ? next : current;
    });
    setDims((current) => {
      if (Object.keys(current).length > 0 || !manifest.columns) return current;
      const next: Record<string, DimSpec> = {};
      for (const c of manifest.columns) {
        if (c.is_dimension && c.name) {
          next[c.name] = { display_name: c.name, aliases: [], precompute: true, top_n: 500, hidden: false };
        }
      }
      return Object.keys(next).length > 0 ? next : current;
    });
  }, [ready, artifact]);

  const patchMetric = (k: string, p: Partial<MetricSpec>) => setMetrics((m) => ({ ...m, [k]: { ...m[k], ...p } }));
  const patchDim = (k: string, p: Partial<DimSpec>) => setDims((d) => ({ ...d, [k]: { ...d[k], ...p } }));

  const save = useCallback(async () => {
    if (!datasetId) return;
    setSaving(true); setMsg(""); setNeedsRegen(false);
    // The PUT replaces the stored record, so the payload starts from it and the
    // form's values go over the top — per entry as well as at the top level,
    // so a metric's `approximate` and the dataset's `freshness_policy` survive
    // a save made by a form that does not show them.
    const mergeEntries = (kind: "metrics" | "dimensions", edited: Record<string, MetricSpec | DimSpec>): Record<string, unknown> => {
      const base = stored.current[kind] || {};
      const out: Record<string, unknown> = {};
      for (const [key, value] of Object.entries(edited)) {
        const entry: Record<string, unknown> = { ...(base[key] || {}), ...value };
        // A cleared display name is not a display name. It was persisted as
        // "" and applied over the suggestion, leaving the dimension nameless
        // everywhere the assistant quoted it.
        if (typeof entry.display_name === "string" && !entry.display_name.trim()) delete entry.display_name;
        out[key] = entry;
      }
      return out;
    };
    const payload = {
      ...stored.current,
      metrics: mergeEntries("metrics", metrics),
      dimensions: mergeEntries("dimensions", dims),
      // A half-filled row maps a phrase to nothing. The key was already
      // guarded; the value was not, so an unfinished row saved an alias that
      // resolved every question about that phrase to an empty value.
      value_aliases: Object.fromEntries(valiases.filter(([k, v]) => k.trim() && v.trim())),
      default_metric: dflt || null,
    };
    try {
      const r = await msalFetch(`/api/v1/datasets/${datasetId}/dlm/context`, {
        method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(payload),
      });
      const j = await r.json().catch(() => null);
      if (r.ok) {
        setNeedsRegen(!!j?.needs_regenerate);
        setMsg(j?.needs_regenerate ? "Saved — regenerate to apply the breakdown and depth changes." : "Saved. Aliases, defaults and value aliases are live now.");
        // The record this form merges into is now what was just written.
        stored.current = { ...payload } as StoredCuration;
      } else {
        // The route refuses this PUT below the Analyst role, and the demo
        // deployment refuses every write — both say so in `detail`. A fixed
        // "Save failed." threw that away and left the reader to guess whether
        // to retry something that will never be allowed.
        setMsg(refusalMessage(j, `Save failed — the platform answered ${r.status}.`));
      }
    } catch { setMsg("Save failed — the platform could not be reached."); }
    setSaving(false);
  }, [datasetId, metrics, dims, valiases, dflt]);

  const regenerate = useCallback(async () => {
    if (!datasetId) return;
    setSaving(true); setMsg("Regenerating context");
    try {
      const r = await msalFetch(`/api/v1/datasets/${datasetId}/dlm/generate?force=true`, { method: "POST" });
      // The response was never read, so a refused regenerate — 403 below
      // Analyst, 403 on the read-only demo, 404 for a dataset with no
      // artifact — reported "Context regenerated." and cleared the prompt.
      if (!r.ok) {
        const body = await r.json().catch(() => null);
        setMsg(refusalMessage(body, `Regenerate failed — the platform answered ${r.status}.`));
      } else {
        setMsg("Context regenerated."); setNeedsRegen(false);
      }
      await dlm.reload();
      await load();
    }
    catch { setMsg("Regenerate failed — the platform could not be reached."); }
    setSaving(false);
  }, [datasetId, dlm, load]);

  if (!ready) {
    if (!readFailure) return null;
    return (
      <section className={className}>
        <div style={{ display: "flex", alignItems: "flex-start", gap: 10 }}>
          <i className="fas fa-sliders" aria-hidden="true" style={{ marginTop: 3, width: 13, textAlign: "center", fontSize: 12, color: "var(--text-muted)" }} />
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 13, fontWeight: 700, color: "var(--text-primary)" }}>Customize</div>
            <p role="status" style={{ margin: "3px 0 0", fontSize: 12.5, lineHeight: 1.5, color: "var(--text-secondary)" }}>{readFailure}</p>
            <button type="button" onClick={() => void load()}
              style={{ marginTop: 8, padding: "5px 12px", border: "1px solid var(--border)", borderRadius: 8, background: "var(--bg-elevated)", color: "var(--text-secondary)", cursor: "pointer", fontSize: 12, fontWeight: 500 }}>
              Try again
            </button>
          </div>
        </div>
      </section>
    );
  }

  return (
    <section className={className}>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        style={{ display: "flex", alignItems: "center", gap: 10, minHeight: 26, width: "100%", padding: 0, border: 0, background: "transparent", textAlign: "left", cursor: "pointer", color: "var(--text-primary)" }}
      >
        <i className="fas fa-sliders" aria-hidden="true" style={{ width: 13, textAlign: "center", fontSize: 12, color: "var(--accent)" }} />
        <span style={{ fontSize: 13, fontWeight: 700, flexShrink: 0 }}>Customize</span>
        <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 12, fontWeight: 400, color: "var(--text-muted)" }}>
          metric aliases, breakdowns, value mappings
        </span>
        <i
          className="fas fa-chevron-down"
          aria-hidden="true"
          style={{ marginLeft: "auto", flexShrink: 0, fontSize: 10, color: "var(--text-muted)", transition: "transform .18s ease", transform: open ? "rotate(180deg)" : undefined }}
        />
      </button>

      {open && (
        <>
          <div style={{ fontSize: 13, color: "var(--text-muted)", marginTop: 10, marginBottom: 4 }}>
            Teach the query engine how to read questions about <b>{name || "this dataset"}</b>.
          </div>

          {/* Metrics and dimensions side by side */}
          <div style={{ display: "grid", gridTemplateColumns: "minmax(0, 1fr) minmax(0, 1fr)", gap: 32, marginTop: 12 }}>
          <div style={{ minWidth: 0 }}>
          <div style={sub}>Metrics ({Object.keys(metrics).length})</div>
          {Object.keys(metrics).map((k) => {
            const m = metrics[k];
            return (
              <div key={k} style={rowCard}>
                <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
                  <b style={{ fontSize: 13, flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{k}</b>
                  <Toggle on={m.additive !== false} onClick={() => patchMetric(k, { additive: !(m.additive !== false) })}>additive</Toggle>
                  <Toggle on={!m.hidden} onClick={() => patchMetric(k, { hidden: !m.hidden })}>{m.hidden ? "hidden" : "visible"}</Toggle>
                </div>
                <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                  <span style={label}>aliases</span>
                  <Aliases items={m.aliases || []} onChange={(v) => patchMetric(k, { aliases: v })} />
                </div>
              </div>
            );
          })}
          </div>

          <div style={{ minWidth: 0 }}>
          <div style={sub}>Dimensions ({Object.keys(dims).length})</div>
          {Object.keys(dims).map((k) => {
            const d = dims[k];
            return (
              <div key={k} style={rowCard}>
                <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
                  <b style={{ fontSize: 13 }}>{k}</b>
                  <input style={{ ...input, width: 150 }} value={d.display_name ?? k} onChange={(e) => patchDim(k, { display_name: e.target.value })} placeholder="display name" />
                  <Toggle on={d.precompute !== false} onClick={() => patchDim(k, { precompute: !(d.precompute !== false) })}>precompute</Toggle>
                  <span style={{ display: "inline-flex", alignItems: "center", gap: 5, fontSize: 11.5, color: "var(--text-muted)" }}>
                    depth
                    <input type="number" min={1} max={5000} style={{ ...input, width: 72 }} value={d.top_n ?? 500} onChange={(e) => patchDim(k, { top_n: Math.max(1, Math.min(5000, Number(e.target.value) || 500)) })} />
                  </span>
                  <Toggle on={!d.hidden} onClick={() => patchDim(k, { hidden: !d.hidden })}>{d.hidden ? "hidden" : "visible"}</Toggle>
                </div>
                <div style={{ display: "flex", gap: 8, alignItems: "flex-start" }}>
                  <span style={label}>aliases</span>
                  <Aliases items={d.aliases || []} onChange={(v) => patchDim(k, { aliases: v })} />
                </div>
              </div>
            );
          })}
          </div>
          </div>

          {/* Value aliases */}
          <div style={sub}>Value aliases <span style={{ textTransform: "none", fontWeight: 400 }}>— map a phrase to a real value (for example <code>smb</code> to <code>Team</code>)</span></div>
          {valiases.map(([kk, vv], i) => (
            <div key={i} style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 6 }}>
              <input style={{ ...input, width: 150 }} value={kk} placeholder="phrase" onChange={(e) => setValiases((a) => a.map((p, j) => j === i ? [e.target.value, p[1]] : p))} />
              <i className="fas fa-arrow-right" aria-hidden="true" style={{ fontSize: 10, color: "var(--text-muted)" }} />
              <input style={{ ...input, width: 150 }} value={vv} placeholder="actual value" onChange={(e) => setValiases((a) => a.map((p, j) => j === i ? [p[0], e.target.value] : p))} />
              <button type="button" onClick={() => setValiases((a) => a.filter((_, j) => j !== i))}
                aria-label={kk.trim() ? `Remove value alias ${kk}` : "Remove empty value alias row"}
                style={{ display: "inline-flex", alignItems: "center", padding: 2, border: 0, background: "transparent", cursor: "pointer", opacity: 0.55, color: "var(--text-primary)" }}>
                <i className="fas fa-xmark" aria-hidden="true" />
              </button>
            </div>
          ))}
          <button type="button" onClick={() => setValiases((a) => [...a, ["", ""]])} style={{ background: "none", border: "none", color: "var(--accent)", cursor: "pointer", fontSize: 12, fontWeight: 600, padding: 0, marginTop: 2 }}>
            <i className="fas fa-plus" aria-hidden="true" style={{ fontSize: 10, marginRight: 5 }} />add value alias
          </button>

          {/* Actions */}
          <div style={{ display: "flex", alignItems: "center", gap: 12, marginTop: 18 }}>
            <button type="button" onClick={save} disabled={saving}
              style={{ padding: "7px 16px", background: "var(--accent)", color: "#fff", border: "none", borderRadius: 8, cursor: saving ? "default" : "pointer", fontSize: 13, fontWeight: 600, opacity: saving ? 0.7 : 1 }}>
              <i className={`fas ${saving ? "fa-spinner fa-spin" : "fa-floppy-disk"}`} aria-hidden="true" style={{ fontSize: 11, marginRight: 6 }} />Save context
            </button>
            {needsRegen && (
              <button type="button" onClick={regenerate} disabled={saving}
                style={{ padding: "7px 16px", background: "transparent", color: "var(--accent)", border: "1px solid var(--accent)", borderRadius: 8, cursor: "pointer", fontSize: 13, fontWeight: 600 }}>
                <i className="fas fa-bolt" aria-hidden="true" style={{ fontSize: 11, marginRight: 6 }} />Regenerate now
              </button>
            )}
            {msg && <span style={{ fontSize: 13, color: "var(--text-secondary)" }}>{msg}</span>}
          </div>
        </>
      )}
    </section>
  );
}
