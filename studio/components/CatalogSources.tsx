"use client";

import React, { useEffect, useRef, useState, useCallback } from "react";
import { API_BASE } from "../config";
import { msalFetch } from "../utils/msalFetch";
import { refusalMessage } from "../utils/demoQuota";
import { useAuth } from "../auth/useAuth";
import { useTheme } from "../contexts/ThemeContext";
import { Button } from "./Button";
import { Pagination } from "./Pagination";
import { LastKnown, UnknownPanel, reasonFromStatus, type ReadReason } from "./Reading";

interface CatalogSource {
  id: string;
  name: string;
  engine_catalog: string;
  storage_type: "local" | "adls_gen2" | "s3";
  storage_config: string;
  data_format: "parquet" | "delta" | "iceberg";
  credential_kind: string | null;
  credential_ref: string | null;
  adapter_type: string;
  adapter_config: string;
  lifecycle: string;
  description: string | null;
  created_by: string;
  modified_by: string | null;
  created_at: string;
  modified_at: string;
}

/**
 * Two readings of every hue, because these chips had only one.
 *
 * The badges and the modal's selection chips were a single light-theme
 * palette — a dark ink on a pastel wash — and Studio defaults to the dark
 * theme. On that ground the wash collapsed into the surface while the ink
 * stayed dark: #065f46 over #065f4614 measures 1.91:1, so "Local" and
 * "Managed Identity" were legible by position rather than by reading. Two of
 * them missed the 4.5:1 body-text threshold on the light theme as well —
 * #0078D4 on #eff6ff at 4.16:1 and #0891b2 on #ecfeff at 3.54:1 — so the
 * palette was not a dark-theme oversight but an unmeasured palette.
 *
 * Each hue therefore carries a reading per ground, lifted for the dark one the
 * way globals.css lifted the status tokens rather than inherited from the light
 * one and left to fail. Measured against the surface each actually sits on,
 * the weakest pair is now 5.15:1. The hues stay literals because they are
 * identity — which storage, which format, which credential — and not chrome;
 * NEUTRAL carries no hue at all and defers to the text and surface tokens, so
 * it follows the theme by itself.
 */
interface Swatch { ink: string; wash: string; edge: string }
interface Paint { light: Swatch; dark: Swatch }

const NEUTRAL: Paint = {
  light: { ink: "var(--text-secondary)", wash: "var(--bg-hover)", edge: "var(--border)" },
  dark:  { ink: "var(--text-secondary)", wash: "var(--bg-hover)", edge: "var(--border)" },
};
const GREEN: Paint = {
  light: { ink: "#065f46", wash: "#d1fae5", edge: "#6ee7b7" },   // 6.78:1
  dark:  { ink: "#6ee7b7", wash: "#103029", edge: "#2f6b58" },   // 9.33:1
};
const BLUE: Paint = {
  light: { ink: "#0b63ad", wash: "#eff6ff", edge: "#93c5fd" },   // 5.67:1
  dark:  { ink: "#8ecdf7", wash: "#10283a", edge: "#2a5d80" },   // 8.82:1
};
const ORANGE: Paint = {
  light: { ink: "#c2410c", wash: "#fff7ed", edge: "#fdba74" },   // 4.88:1
  dark:  { ink: "#fdba74", wash: "#35230f", edge: "#7a4f22" },   // 8.90:1
};
const PURPLE: Paint = {
  light: { ink: "#7c3aed", wash: "#f5f3ff", edge: "#c4b5fd" },   // 5.20:1
  dark:  { ink: "#c4b5fd", wash: "#262040", edge: "#4b3f7a" },   // 8.35:1
};
const CYAN: Paint = {
  light: { ink: "#0e7490", wash: "#ecfeff", edge: "#a5f3fc" },   // 5.15:1
  dark:  { ink: "#67e8f9", wash: "#0e2b33", edge: "#1f5f6e" },   // 10.26:1
};
const INDIGO: Paint = {
  light: { ink: "#4338ca", wash: "#eef2ff", edge: "#c7d2fe" },   // 7.07:1
  dark:  { ink: "#a5b4fc", wash: "#1f2347", edge: "#3b3f7a" },   // 7.58:1
};
const AMBER: Paint = {
  light: { ink: "#92400e", wash: "#fef3c7", edge: "#fde68a" },   // 6.37:1
  dark:  { ink: "#fbbf24", wash: "#342611", edge: "#6e5320" },   // 8.79:1
};
const RED: Paint = {
  light: { ink: "#991b1b", wash: "#fee2e2", edge: "#fecaca" },   // 6.80:1
  dark:  { ink: "#f87171", wash: "#3a1d1d", edge: "#7a3a3a" },   // 5.53:1
};

type Theme = "light" | "dark";
const paint = (p: Paint, theme: Theme): Swatch => (theme === "dark" ? p.dark : p.light);

const STORAGE_META: Record<string, { label: string; paint: Paint }> = {
  local:    { label: "Local",     paint: GREEN },
  adls_gen2:{ label: "ADLS Gen2", paint: BLUE },
  s3:       { label: "S3",        paint: ORANGE },
};

const FORMAT_META: Record<string, { label: string; paint: Paint }> = {
  parquet: { label: "Parquet", paint: PURPLE },
  delta:   { label: "Delta",   paint: CYAN },
  iceberg: { label: "Iceberg", paint: INDIGO },
};

const ADAPTER_META: Record<string, { label: string }> = {
  native:          { label: "Native" },
  hive_metastore:  { label: "Hive Metastore" },
  aws_glue:        { label: "AWS Glue" },
  unity_catalog:   { label: "Unity Catalog" },
  iceberg_rest:    { label: "Iceberg REST" },
};

const CREDENTIAL_META: { kind: string; label: string; paint: Paint }[] = [
  { kind: "",                   label: "Host identity",     paint: NEUTRAL },
  { kind: "managed_identity",   label: "Managed Identity",  paint: GREEN },
  { kind: "workload_identity",  label: "Workload Identity", paint: BLUE },
  { kind: "secret_store",       label: "Key Vault",         paint: PURPLE },
];

const LIFECYCLE_META: Record<string, { label: string; paint: Paint }> = {
  draft:     { label: "Draft",     paint: NEUTRAL },
  active:    { label: "Active",    paint: GREEN },
  suspended: { label: "Suspended", paint: AMBER },
  deleting:  { label: "Deleting",  paint: RED },
  deleted:   { label: "Deleted",   paint: NEUTRAL },
};

// "Delete" named two different operations in this table: the draft transition
// below, which only moves the record to `deleted` and drops it out of the
// listing, and the trash button further down, which is the irreversible row
// removal. One of them had to say what it does, so the reversible one is
// "Discard" and the confirmations are worded to match.
const LIFECYCLE_ACTIONS: Record<string, { label: string; target: string; icon: string }[]> = {
  draft:     [{ label: "Activate", target: "active", icon: "fa-check-circle" }, { label: "Discard", target: "deleted", icon: "fa-ban" }],
  active:    [{ label: "Suspend", target: "suspended", icon: "fa-pause-circle" }, { label: "Decommission", target: "deleting", icon: "fa-archive" }],
  suspended: [{ label: "Reactivate", target: "active", icon: "fa-play-circle" }, { label: "Decommission", target: "deleting", icon: "fa-archive" }],
  deleting:  [{ label: "Finish decommissioning", target: "deleted", icon: "fa-archive" }],
  deleted:   [],
};

function Badge({ label, swatch }: { label: string; swatch: Swatch }) {
  return (
    <span style={{
      display: "inline-flex", alignItems: "center", gap: 4,
      padding: "0.15rem 0.5rem", borderRadius: 6, fontSize: 12, fontWeight: 600,
      background: swatch.wash, color: swatch.ink,
      border: `1px solid ${swatch.edge}`,
      whiteSpace: "nowrap",
    }}>
      {label}
    </span>
  );
}

function parseConfig(raw: string): Record<string, string> {
  try { return JSON.parse(raw) || {}; } catch { return {}; }
}

/**
 * Where the catalog reads from, as the record actually holds it.
 *
 * The root path was not shown at all, which is how an operator came to retype
 * one in the form without ever having seen the one it replaced. It is shown
 * now, and so is the fact of its being empty — a blank root path means the
 * container root, which is a real setting and not a missing one.
 *
 * A part the record does not carry reads as a gap rather than as the word
 * "undefined": this interpolated `cfg.account` and `cfg.container` unchecked,
 * so a half-written ADLS config displayed "undefined/undefined" as though
 * that were the prefix.
 */
function storageDisplay(cs: CatalogSource): string {
  const cfg = parseConfig(cs.storage_config);
  const part = (value: string | undefined) => (value && value.trim() ? value.trim() : "·");
  if (cs.storage_type === "local") return part(cfg.base_path);
  if (cs.storage_type === "adls_gen2") return `${part(cfg.account)}/${part(cfg.container)}/${part(cfg.root_path)}`;
  if (cs.storage_type === "s3") return `${part(cfg.bucket)}/${part(cfg.prefix)}`;
  return "—";
}

/** A listing read that did not answer, kept apart from a listing that was empty. */
interface ReadFailure { reason: ReadReason | null; detail: string }

export function CatalogSources() {
  const { isAuthenticated } = useAuth();
  const { theme } = useTheme();
  const [sources, setSources] = useState<CatalogSource[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  /** Set only when the read itself did not answer; never when it answered with nothing. */
  const [readFailure, setReadFailure] = useState<ReadFailure | null>(null);
  const [showModal, setShowModal] = useState(false);
  const [editing, setEditing] = useState<CatalogSource | null>(null);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);
  const [syncingId, setSyncingId] = useState<string | null>(null);
  const [engineRevisions, setEngineRevisions] = useState<Record<string, number>>({});
  const PAGE_SIZE = 20;

  /**
   * A read that did not answer is held apart from one that answered with
   * nothing. Both used to end in the same place: the failure path set `error`
   * and left `sources` as it was, and the render below keyed its empty state
   * off `!error`, so a refused or unreachable listing drew the table with an
   * empty body — "no catalog sources" over however many the server holds.
   *
   * It also claimed a cause it had not established. Every status became
   * "Failed to load catalog sources", and where the server did say something
   * the message was discarded: `/catalog-sources` answers 403 for a reader
   * below Admin and this told them the load had failed.
   */
  const load = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      const res = await msalFetch(`${API_BASE}/api/v1/catalog-sources`);
      const body: unknown = await res.json().catch(() => null);
      if (!res.ok) {
        setReadFailure({
          reason: reasonFromStatus(res.status),
          detail: refusalMessage(body, `The server answered ${res.status}.`),
        });
        return;
      }
      const listing = (body as { catalogSources?: unknown } | null)?.catalogSources;
      if (!Array.isArray(listing)) {
        setReadFailure({ reason: "invalid_listing", detail: "" });
        return;
      }
      setReadFailure(null);
      setSources(listing as CatalogSource[]);
    } catch {
      setReadFailure({ reason: null, detail: "The Kaveon API could not be reached from this browser." });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (isAuthenticated) load();
  }, [isAuthenticated, load]);

  /**
   * `err.detail` was interpolated straight into the message, and this API's
   * own 404 raises `HTTPException(404, {"code": ..., "message": ...})` — an
   * object, which `new Error()` renders as "[object Object]". The three shapes
   * the API uses are read by one helper instead.
   */
  const transitionLifecycle = async (cs: CatalogSource, target: string) => {
    // Moving to `deleted` is reversible in the sense that nothing is erased,
    // but the listing excludes it, so the record leaves this page for good.
    // The confirmation used to call it a permanent delete, which it is not —
    // the trash button below is.
    if (target === "deleted" && !confirm(
      `Remove "${cs.name}" from this list? Its definition is retained and its Engine catalog name stays reserved, `
      + "but it can no longer be activated or edited here."
    )) return;
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/catalog-sources/${cs.id}/transition`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ lifecycle: target }),
      });
      if (!res.ok) {
        const body: unknown = await res.json().catch(() => null);
        throw new Error(refusalMessage(body, `The server answered ${res.status} and did not say why.`));
      }
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "The lifecycle change could not be applied.");
    }
  };

  const deletePermanently = async (cs: CatalogSource) => {
    if (!confirm(`Permanently remove "${cs.name}"? This erases its definition and cannot be undone.`)) return;
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/catalog-sources/${cs.id}`, { method: "DELETE" });
      if (!res.ok) {
        const body: unknown = await res.json().catch(() => null);
        throw new Error(refusalMessage(body, `The server answered ${res.status} and did not say why.`));
      }
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "The catalog source could not be removed.");
    }
  };

  const syncWithEngine = async (cs: CatalogSource) => {
    try {
      setSyncingId(cs.id);
      setError(null);
      const revision = engineRevisions[cs.id];
      const res = await msalFetch(`${API_BASE}/api/v1/catalog-sources/${cs.id}/engine-sync`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(revision === undefined ? {} : { expected_revision: revision }),
      });
      const payload = await res.json().catch(() => null);
      if (!res.ok) throw new Error(refusalMessage(payload, `The Engine answered ${res.status} and did not say why.`));
      const nextRevision = (payload as { catalog?: { revision?: unknown } } | null)?.catalog?.revision;
      if (typeof nextRevision === "number") {
        setEngineRevisions(current => ({ ...current, [cs.id]: nextRevision }));
      }
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Engine synchronization failed");
    } finally {
      setSyncingId(null);
    }
  };

  const filtered = sources.filter(cs =>
    !search || cs.name.toLowerCase().includes(search.toLowerCase()) ||
    cs.engine_catalog.toLowerCase().includes(search.toLowerCase()) ||
    (cs.description || "").toLowerCase().includes(search.toLowerCase())
  );
  const paged = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const activeCount = sources.filter(s => s.lifecycle === "active").length;

  if (loading) {
    return (
      <div style={{ display: "flex", justifyContent: "center", padding: "3rem 0" }}>
        <i className="fas fa-spinner fa-spin" style={{ fontSize: 20, color: "var(--text-muted)" }} />
      </div>
    );
  }

  return (
    <>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1rem" }}>
        <div style={{ display: "flex", alignItems: "center", gap: "0.75rem" }}>
          {sources.length > 0 && (
            <>
              <span style={{ fontSize: 13, color: "var(--text-muted)" }}>{sources.length} catalog{sources.length !== 1 ? "s" : ""}</span>
              {/* `--success` is an indicator colour, not an ink: measured as
                  13px text on --bg-primary it reads 2.37:1 on the light theme.
                  The green that was chosen for text carries it instead —
                  7.18:1 light, 11.76:1 dark — and the meaning is unchanged. */}
              <span style={{ fontSize: 13, color: paint(GREEN, theme).ink, fontWeight: 600 }}>{activeCount} active</span>
            </>
          )}
        </div>
        <div style={{ display: "flex", gap: "0.5rem", alignItems: "center" }}>
          {sources.length > 3 && (
            <input
              type="text"
              placeholder="Search catalogs..."
              value={search}
              onChange={e => { setSearch(e.target.value); setPage(1); }}
              style={{
                padding: "0.4rem 0.75rem", borderRadius: 6, fontSize: 13,
                border: "1px solid var(--border)", background: "var(--bg-surface)",
                color: "var(--text-primary)", outline: "none", width: 200,
              }}
            />
          )}
          <Button size="sm" onClick={() => { setEditing(null); setShowModal(true); }}>
            <i className="fas fa-plus" /> New Catalog
          </Button>
        </div>
      </div>

      {error && (
        <div role="alert" style={{
          padding: "0.75rem 1rem", borderRadius: 8, marginBottom: "1rem",
          // The literal red was the old #ef4444 at two opacities, so this
          // banner stayed the light theme's red on the dark one while
          // everything beside it followed the token.
          background: "color-mix(in srgb, var(--danger) 10%, transparent)",
          color: "var(--danger)",
          border: "1px solid color-mix(in srgb, var(--danger) 35%, transparent)",
          display: "flex", alignItems: "center", gap: "0.75rem", fontSize: 13,
        }}>
          <i className="fas fa-exclamation-circle" aria-hidden="true" />
          {error}
          <button type="button" aria-label="Dismiss this message" onClick={() => setError(null)} style={{ marginLeft: "auto", background: "none", border: "none", color: "inherit", cursor: "pointer" }}>
            <i className="fas fa-times" aria-hidden="true" />
          </button>
        </div>
      )}

      {/* A read that did not answer, with nothing held from an earlier one, is
          an unknown — not an empty catalog list. */}
      {readFailure && sources.length === 0 ? (
        <UnknownPanel
          what="The catalog source registry"
          says="which storage locations are registered"
          reason={readFailure.reason}
          detail={readFailure.detail || null}
          onRetry={load}
        />
      ) : !readFailure && sources.length === 0 ? (
        <div style={{
          textAlign: "center", padding: "3rem", color: "var(--text-muted)",
          background: "var(--bg-surface)", borderRadius: 12, border: "1px solid var(--border)",
        }}>
          <i className="fas fa-database" style={{ fontSize: 32, marginBottom: "1rem", display: "block", opacity: 0.4 }} />
          <div style={{ fontSize: 15, fontWeight: 600, color: "var(--text-primary)", marginBottom: 6 }}>No Engine catalog sources</div>
          <div style={{ fontSize: 13, marginBottom: "1.5rem" }}>Register a storage location to make data available to the Engine.</div>
          <Button onClick={() => { setEditing(null); setShowModal(true); }}>
            <i className="fas fa-plus" /> Add Catalog Source
          </Button>
        </div>
      ) : (
        <div className="card">
          {/* Rows from the last read that answered, kept rather than cleared,
              and labelled so nobody reads them as current. */}
          {readFailure && (
            <LastKnown
              what="The catalog source registry"
              detail={readFailure.detail || null}
              onRetry={load}
            />
          )}
          <div className="results-table-container">
            <table className="results-table">
              <thead>
                <tr>
                  <th><span className="column-header-label">Name</span></th>
                  <th><span className="column-header-label">Engine Catalog</span></th>
                  <th><span className="column-header-label">Storage</span></th>
                  <th><span className="column-header-label">Format</span></th>
                  <th><span className="column-header-label">Adapter</span></th>
                  <th><span className="column-header-label">Credential</span></th>
                  <th><span className="column-header-label">Status</span></th>
                  <th><span className="column-header-label">Actions</span></th>
                </tr>
              </thead>
              <tbody>
                {paged.map(cs => {
                  const lcMeta = LIFECYCLE_META[cs.lifecycle] || LIFECYCLE_META.draft;
                  const stMeta = STORAGE_META[cs.storage_type] || STORAGE_META.local;
                  const fmtMeta = FORMAT_META[cs.data_format] || FORMAT_META.parquet;
                  const actions = LIFECYCLE_ACTIONS[cs.lifecycle] || [];
                  return (
                    <tr key={cs.id}>
                      <td>
                        <strong>{cs.name}</strong>
                        {cs.description && <div className="muted" style={{ marginTop: 4, fontSize: 12 }}>{cs.description}</div>}
                      </td>
                      <td style={{ fontFamily: "monospace", fontSize: 12, color: "var(--text-secondary)" }}>{cs.engine_catalog}</td>
                      <td>
                        <Badge label={stMeta.label} swatch={paint(stMeta.paint, theme)} />
                        <div className="muted" title={storageDisplay(cs)} style={{ marginTop: 3, fontSize: 11, fontFamily: "monospace", maxWidth: 200, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                          {storageDisplay(cs)}
                        </div>
                      </td>
                      <td><Badge label={fmtMeta.label} swatch={paint(fmtMeta.paint, theme)} /></td>
                      <td style={{ fontSize: 12, color: "var(--text-secondary)" }}>
                        {ADAPTER_META[cs.adapter_type]?.label || cs.adapter_type}
                      </td>
                      <td>
                        {cs.credential_kind ? (
                          // Same reason as the active count above: `--success`
                          // is 2.47:1 as 12px text on the light card.
                          <span style={{ fontSize: 12, color: paint(GREEN, theme).ink }}>
                            <i className="fas fa-shield-alt" style={{ marginRight: 4 }} aria-hidden="true" />
                            {cs.credential_kind === "secret_store" ? "Key Vault" : cs.credential_kind.replace(/_/g, " ")}
                          </span>
                        ) : (
                          <span style={{ fontSize: 12, color: "var(--text-muted)" }} title="No credential is configured on this source, so the Engine authenticates with the host's own identity — on Azure, the VM's managed identity through the default credential chain.">Host identity</span>
                        )}
                      </td>
                      <td><Badge label={lcMeta.label} swatch={paint(lcMeta.paint, theme)} /></td>
                      <td className="actions-cell">
                        <div className="row-actions">
                          {actions.map(a => (
                            <button
                              key={a.target}
                              type="button"
                              className="action-icon-btn"
                              title={a.label}
                              onClick={() => transitionLifecycle(cs, a.target)}
                            >
                              <i className={`fas ${a.icon}`} />
                            </button>
                          ))}
                          {cs.adapter_type === "native" && cs.lifecycle !== "deleted" && (
                            <button
                              type="button"
                              className="action-icon-btn"
                              title="Sync this catalog definition with Kaveon Engine"
                              onClick={() => syncWithEngine(cs)}
                              disabled={syncingId === cs.id}
                            >
                              <i className={syncingId === cs.id ? "fas fa-spinner fa-spin" : "fas fa-arrows-rotate"} />
                            </button>
                          )}
                          <button type="button" className="action-icon-btn" title="Edit" onClick={() => { setEditing(cs); setShowModal(true); }}>
                            <i className="fas fa-edit" />
                          </button>
                          {/* The route accepts a permanent removal for `draft`
                              or `deleted`; this offered it for `draft` alone.
                              A source taken active → deleting → deleted could
                              therefore never be erased, and because the name
                              and the Engine catalog name stay unique across
                              every row, its Engine catalog name stayed
                              reserved for good — registering a replacement
                              under the same name answered 409 with no control
                              anywhere to release it. The listing excludes
                              `deleted` rows server-side, so this matches the
                              route rather than fully closing the gap; see the
                              note filed with this change. */}
                          {(cs.lifecycle === "draft" || cs.lifecycle === "deleted") && (
                            <button type="button" className="action-icon-btn" title="Remove permanently" onClick={() => deletePermanently(cs)}>
                              <i className="fas fa-trash" aria-hidden="true" />
                            </button>
                          )}
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <Pagination total={filtered.length} page={page} pageSize={PAGE_SIZE} onChange={setPage} />
        </div>
      )}

      {showModal && (
        <CatalogSourceModal
          source={editing}
          onClose={() => { setShowModal(false); setEditing(null); }}
          onSuccess={() => { setShowModal(false); setEditing(null); load(); }}
        />
      )}
    </>
  );
}


interface ModalProps {
  source: CatalogSource | null;
  onClose: () => void;
  onSuccess: () => void;
}

function CatalogSourceModal({ source, onClose, onSuccess }: ModalProps) {
  const isEditing = !!source;
  const { theme } = useTheme();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const firstFieldRef = useRef<HTMLInputElement>(null);

  /**
   * This was a fixed overlay with no dialog semantics at all: no role, so a
   * screen reader announced it as part of the page behind it; no initial
   * focus, so keyboard focus stayed on the "New Catalog" button underneath;
   * and no Escape, so the only way out was to find the close button or click
   * the backdrop. Focus moves to the first field on open and Escape closes.
   */
  useEffect(() => {
    firstFieldRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.stopPropagation(); onClose(); }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const parseStorageConfig = (s: CatalogSource | null) => {
    if (!s) return {};
    try { return JSON.parse(s.storage_config) || {}; } catch { return {}; }
  };
  const parseAdapterConfig = (s: CatalogSource | null) => {
    if (!s) return {};
    try { return JSON.parse(s.adapter_config) || {}; } catch { return {}; }
  };

  const [name, setName] = useState(source?.name || "");
  const [engineCatalog, setEngineCatalog] = useState(source?.engine_catalog || "");
  const [storageType, setStorageType] = useState<string>(source?.storage_type || "local");
  const [dataFormat, setDataFormat] = useState<string>(source?.data_format || "parquet");
  const [adapterType, setAdapterType] = useState<string>(source?.adapter_type || "native");
  const [description, setDescription] = useState(source?.description || "");
  const [credentialKind, setCredentialKind] = useState<string>(source?.credential_kind || "");
  const [credentialRef, setCredentialRef] = useState(source?.credential_ref || "");

  const initSC = parseStorageConfig(source);
  const [basePath, setBasePath] = useState(initSC.base_path || "");
  const [adlsAccount, setAdlsAccount] = useState(initSC.account || "");
  const [adlsContainer, setAdlsContainer] = useState(initSC.container || "");
  const [adlsRootPath, setAdlsRootPath] = useState(initSC.root_path || "");
  const [s3Bucket, setS3Bucket] = useState(initSC.bucket || "");
  const [s3Region, setS3Region] = useState(initSC.region || "");
  const [s3Prefix, setS3Prefix] = useState(initSC.prefix || "");

  // What the record held when the form opened. A storage change is compared
  // against this rather than against the inputs' own defaults, so reopening a
  // form and saving it unchanged is never treated as a re-point.
  const storedStorage = JSON.stringify({
    storage_type: source?.storage_type || "local",
    base_path: initSC.base_path || "",
    account: initSC.account || "",
    container: initSC.container || "",
    root_path: initSC.root_path || "",
    bucket: initSC.bucket || "",
    region: initSC.region || "",
    prefix: initSC.prefix || "",
  });
  const currentStorage = JSON.stringify({
    storage_type: storageType,
    base_path: basePath, account: adlsAccount, container: adlsContainer,
    root_path: adlsRootPath, bucket: s3Bucket, region: s3Region, prefix: s3Prefix,
  });
  const storageChanged = storedStorage !== currentStorage;
  const [storageChangeConfirmed, setStorageChangeConfirmed] = useState(false);

  const initAC = parseAdapterConfig(source);
  const [adapterEndpoint, setAdapterEndpoint] = useState(initAC.endpoint || "");
  const [adapterDatabase, setAdapterDatabase] = useState(initAC.database || "");

  const buildStorageConfig = () => {
    if (storageType === "local") return JSON.stringify({ base_path: basePath });
    if (storageType === "adls_gen2") return JSON.stringify({ account: adlsAccount, container: adlsContainer, root_path: adlsRootPath });
    if (storageType === "s3") return JSON.stringify({ bucket: s3Bucket, region: s3Region, prefix: s3Prefix });
    return "{}";
  };

  const buildAdapterConfig = () => {
    if (adapterType === "native") return "{}";
    const cfg: Record<string, string> = {};
    if (adapterEndpoint) cfg.endpoint = adapterEndpoint;
    if (adapterDatabase) cfg.database = adapterDatabase;
    return JSON.stringify(cfg);
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setSaving(true);
    setError(null);
    try {
      const body: Record<string, unknown> = {
        name: name.trim(),
        engine_catalog: engineCatalog.trim(),
        storage_type: storageType,
        storage_config: buildStorageConfig(),
        data_format: dataFormat,
        adapter_type: adapterType,
        adapter_config: buildAdapterConfig(),
        description: description.trim() || null,
      };
      // A cleared credential has to be sent as an explicit null, and it was
      // not sent at all. Guarded by `if (credentialKind)`, selecting "Host
      // identity" omitted both fields from the PATCH body — so the one
      // selection in this group that removes a credential was the one
      // selection that could not be applied, and the form reported the update
      // as successful while the stored credential stayed exactly where it was.
      //
      // Only a field that actually differs from the record is sent, because
      // the route considers a field solely when it is present: an operator
      // editing a description does not restate a credential they did not
      // touch, and a clear is sent as the null that means "unset it".
      const storedKind = source?.credential_kind || "";
      const storedRef = source?.credential_ref || "";
      if (!isEditing || credentialKind !== storedKind) body.credential_kind = credentialKind || null;
      if (!isEditing || credentialRef.trim() !== storedRef) body.credential_ref = credentialRef.trim() || null;

      // Re-pointing a catalog that already holds tables moves every one of
      // them, and the tables keep their own relative locations — so a changed
      // account, container or root path makes each of them address an object
      // that very likely does not exist. This form once offered an empty,
      // required Root Path for a catalog whose real prefix was held only in
      // the Engine: typing anything and saving would have been enough to
      // break every table in it on the next synchronization.
      //
      // So a storage change on an existing catalog has to be meant. Creating
      // one is unaffected: there is nothing to re-point.
      if (isEditing && storageChanged && !storageChangeConfirmed) {
        setError(
          `This changes where ${engineCatalog || "the catalog"} reads from. Every table registered in it `
          + "keeps its own relative location, so each will address a different object — most likely one "
          + "that does not exist. Confirm below if that is intended."
        );
        setSaving(false);
        return;
      }

      const url = isEditing
        ? `${API_BASE}/api/v1/catalog-sources/${source!.id}`
        : `${API_BASE}/api/v1/catalog-sources`;

      const res = await msalFetch(url, {
        method: isEditing ? "PATCH" : "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });

      if (!res.ok) {
        const refused: unknown = await res.json().catch(() => null);
        throw new Error(refusalMessage(
          refused,
          `The server answered ${res.status} and did not say why, so the catalog source was not ${isEditing ? "updated" : "created"}.`,
        ));
      }
      onSuccess();
    } catch (err) {
      setError(err instanceof Error ? err.message : `The catalog source was not ${isEditing ? "updated" : "created"}.`);
    } finally {
      setSaving(false);
    }
  };

  const inputStyle: React.CSSProperties = {
    width: "100%", padding: "0.5rem 0.75rem", borderRadius: 6, fontSize: 13,
    border: "1px solid var(--border)", background: "var(--bg-primary)",
    color: "var(--text-primary)", outline: "none",
  };

  const labelStyle: React.CSSProperties = {
    fontSize: 13, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 4, display: "block",
  };

  const fieldStyle: React.CSSProperties = { marginBottom: "1rem" };

  const chipStyle = (selected: boolean, swatch: Swatch): React.CSSProperties => ({
    display: "inline-flex", alignItems: "center", gap: 4,
    padding: "0.4rem 0.75rem", borderRadius: 8, fontSize: 13, cursor: "pointer",
    border: `2px solid ${selected ? swatch.ink : "var(--border)"}`,
    background: selected ? swatch.wash : "var(--bg-surface)",
    color: selected ? swatch.ink : "var(--text-secondary)",
    fontWeight: selected ? 600 : 400, transition: "all 0.15s",
  });

  return (
    <>
      <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.5)", zIndex: 1000 }} onClick={onClose} />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="catalog-source-modal-title"
        style={{
          position: "fixed", top: "50%", left: "50%", transform: "translate(-50%,-50%)",
          background: "var(--bg-surface)", borderRadius: 12, boxShadow: "var(--shadow-lg)",
          zIndex: 1001, width: "90%", maxWidth: 680, maxHeight: "90vh", overflow: "hidden",
          display: "flex", flexDirection: "column",
        }}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "1.25rem 1.5rem", borderBottom: "1px solid var(--border)", flexShrink: 0 }}>
          <h2 id="catalog-source-modal-title" style={{ margin: 0, fontSize: "1.25rem", fontWeight: 600 }}>
            {isEditing ? "Edit Catalog Source" : "New Catalog Source"}
          </h2>
          <button type="button" onClick={onClose} aria-label="Close without saving" style={{ background: "none", border: "none", fontSize: "1.25rem", cursor: "pointer", color: "var(--text-muted)", borderRadius: 6, width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center" }}>
            <i className="fas fa-times" aria-hidden="true" />
          </button>
        </div>

        {/* The body carried `flex: 1` and `overflowY: auto`, but its parent was
            this form as a plain block with no height of its own, so neither did
            anything: the body grew to its content, the form grew past the
            shell's 90vh, and `overflow: hidden` on the shell cut off whatever
            did not fit — on a short viewport that was the footer, which holds
            the only way to submit. The form is the flex column now, and
            `minHeight: 0` is what actually lets the body shrink enough to
            scroll. */}
        <form onSubmit={handleSubmit} style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
          <div style={{ padding: "1.25rem 1.5rem", overflowY: "auto", flex: 1, minHeight: 0 }}>
            {error && (
              <div role="alert" style={{ padding: "0.75rem 1rem", borderRadius: 8, marginBottom: "1rem", background: "color-mix(in srgb, var(--danger) 10%, transparent)", color: "var(--danger)", border: "1px solid color-mix(in srgb, var(--danger) 35%, transparent)", display: "flex", alignItems: "center", gap: "0.75rem", fontSize: 13 }}>
                <i className="fas fa-exclamation-circle" aria-hidden="true" />
                {error}
              </div>
            )}

            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "1rem", marginBottom: "1rem" }}>
              <div>
                <label style={labelStyle}>Display Name *</label>
                <input ref={firstFieldRef} style={inputStyle} placeholder="e.g., Production Lakehouse" value={name} onChange={e => setName(e.target.value)} required />
              </div>
              <div>
                <label style={labelStyle}>Engine Catalog Name *</label>
                <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="e.g., lakehouse" value={engineCatalog} onChange={e => setEngineCatalog(e.target.value)} required />
              </div>
            </div>

            <div style={fieldStyle}>
              <label style={labelStyle}>Storage Type *</label>
              <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
                {Object.entries(STORAGE_META).map(([key, meta]) => (
                  <button key={key} type="button" aria-pressed={storageType === key} style={chipStyle(storageType === key, paint(meta.paint, theme))} onClick={() => setStorageType(key)}>
                    {meta.label}
                  </button>
                ))}
              </div>
            </div>

            {storageType === "local" && (
              <div style={fieldStyle}>
                <label style={labelStyle}>Base Path *</label>
                <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="/data/warehouse" value={basePath} onChange={e => setBasePath(e.target.value)} required />
              </div>
            )}
            {storageType === "adls_gen2" && (
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: "0.75rem", ...fieldStyle }}>
                <div>
                  <label style={labelStyle}>Account *</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="storageaccount" value={adlsAccount} onChange={e => setAdlsAccount(e.target.value)} required />
                </div>
                <div>
                  <label style={labelStyle}>Container *</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="datalake" value={adlsContainer} onChange={e => setAdlsContainer(e.target.value)} required />
                </div>
                <div>
                  {/* Marked required, and it is not.
                      `_validate_storage_config` requires the `root_path` key
                      to be present, not to hold anything, and an empty root
                      path is a real setting: the container root. A catalog
                      stored that way opened this field blank with a required
                      marker on it and a blank placeholder, so the browser
                      refused to submit until the operator typed a prefix —
                      which re-points the catalog, and every table in it keeps
                      its own relative location and addresses an object that
                      is not there. The one value the form would not let an
                      operator keep was the one the record held.

                      It is optional here, as the S3 prefix beside it already
                      is, and what blank means is stated rather than left to
                      be guessed. The key is still always sent, so the route's
                      presence check is satisfied; blanking a root path that
                      was set is a storage change and still passes through the
                      confirmation below. */}
                  <label style={labelStyle}>Root Path</label>
                  <input
                    style={{ ...inputStyle, fontFamily: "monospace" }}
                    placeholder="warehouse/"
                    value={adlsRootPath}
                    onChange={e => setAdlsRootPath(e.target.value)}
                    aria-describedby="catalog-source-root-path-help"
                  />
                  <div id="catalog-source-root-path-help" style={{ fontSize: 11.5, color: "var(--text-muted)", marginTop: 4 }}>
                    {adlsRootPath.trim()
                      ? "Tables are addressed beneath this prefix."
                      : "Blank reads from the container root."}
                  </div>
                </div>
              </div>
            )}
            {storageType === "s3" && (
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: "0.75rem", ...fieldStyle }}>
                <div>
                  <label style={labelStyle}>Bucket *</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="my-data-bucket" value={s3Bucket} onChange={e => setS3Bucket(e.target.value)} required />
                </div>
                <div>
                  <label style={labelStyle}>Region *</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="us-east-1" value={s3Region} onChange={e => setS3Region(e.target.value)} required />
                </div>
                <div>
                  <label style={labelStyle}>Prefix</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="warehouse/" value={s3Prefix} onChange={e => setS3Prefix(e.target.value)} />
                </div>
              </div>
            )}

            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "1rem", marginBottom: "1rem" }}>
              <div>
                {/* A catalog does not have a format; its tables do, and they
                    can differ. This is the format a newly registered table is
                    assumed to be, which is why it is named for what it does
                    rather than described as a property of the source. The
                    Catalog page reports each table's real format. */}
                <label style={labelStyle}>Default format for new tables *</label>
                <div style={{ display: "flex", gap: "0.5rem" }}>
                  {Object.entries(FORMAT_META).map(([key, meta]) => (
                    <button key={key} type="button" aria-pressed={dataFormat === key} style={chipStyle(dataFormat === key, paint(meta.paint, theme))} onClick={() => setDataFormat(key)}>
                      {meta.label}
                    </button>
                  ))}
                </div>
              </div>
              <div>
                {/* Who says which tables exist. Native means Kaveon's own
                    catalog over the storage below; the rest defer to an
                    external metastore. Only native can be synchronized today
                    — engine_bridge refuses the others with "Only native
                    catalog adapters can currently be synchronized" — so they
                    are shown as unavailable rather than offered and then
                    rejected after the form is filled in. */}
                <label style={labelStyle}>Catalog Adapter</label>
                <select
                  value={adapterType}
                  onChange={e => setAdapterType(e.target.value)}
                  style={{ ...inputStyle, cursor: "pointer" }}
                >
                  {Object.entries(ADAPTER_META).map(([key, meta]) => (
                    <option key={key} value={key} disabled={key !== "native"}>
                      {key === "native" ? meta.label : `${meta.label} — not yet supported`}
                    </option>
                  ))}
                </select>
              </div>
            </div>

            {adapterType !== "native" && (
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "0.75rem", ...fieldStyle }}>
                <div>
                  <label style={labelStyle}>{ADAPTER_META[adapterType]?.label} Endpoint</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="thrift://metastore:9083" value={adapterEndpoint} onChange={e => setAdapterEndpoint(e.target.value)} />
                </div>
                <div>
                  <label style={labelStyle}>Database / Namespace</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="default" value={adapterDatabase} onChange={e => setAdapterDatabase(e.target.value)} />
                </div>
              </div>
            )}

            <div style={fieldStyle}>
              <label style={labelStyle}>Credential</label>
              <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap", marginBottom: credentialKind ? "0.75rem" : 0 }}>
                {/* "Host identity" is not "None": object storage is never read
                    anonymously. Leaving this unset means the Engine
                    authenticates as whatever the host already is — on Azure,
                    the VM's managed identity, picked up through the default
                    credential chain with nothing configured here. Naming it
                    "None" invited the reader to believe the storage was open.

                    Only Key Vault has a reference to point at, and only Key
                    Vault offers a field for one, so every other selection
                    clears it. Switching an existing Key Vault source to
                    Managed Identity used to leave `credentialRef` holding the
                    vault URI and send it with the save, storing a secret
                    reference against a kind that has no secret — a stale
                    pointer that outlived the only control that could show it.
                    Clearing on selection is why this is one loop rather than
                    four buttons with four different handlers. */}
                {CREDENTIAL_META.map(({ kind, label, paint: hue }) => (
                  <button
                    key={kind || "host"}
                    type="button"
                    aria-pressed={credentialKind === kind}
                    style={chipStyle(credentialKind === kind, paint(hue, theme))}
                    onClick={() => {
                      setCredentialKind(kind);
                      if (kind !== "secret_store") setCredentialRef("");
                    }}
                  >
                    {label}
                  </button>
                ))}
                {/* The route also accepts `environment`, which no chip above
                    offers. A source stored with it drew no selected chip at
                    all, so the form showed a credential of "nothing" over a
                    record that had one, and the first save re-sent whatever
                    the operator could see. Whatever kind the record holds is
                    shown and selectable, so it can be read and kept. */}
                {credentialKind && !CREDENTIAL_META.some(c => c.kind === credentialKind) && (
                  <span style={{ ...chipStyle(true, paint(NEUTRAL, theme)), cursor: "default" }}>
                    {credentialKind.replace(/_/g, " ")} <span style={{ fontWeight: 400 }}>· stored</span>
                  </span>
                )}
              </div>
              {credentialKind === "secret_store" && (
                <div>
                  <label style={labelStyle}>Key Vault URI *</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="https://kv-kaveon.vault.azure.net/secrets/adls-key" value={credentialRef} onChange={e => setCredentialRef(e.target.value)} required />
                </div>
              )}
              {(credentialKind === "managed_identity" || credentialKind === "workload_identity") && (
                <div style={{ fontSize: 12, color: "var(--text-muted)", fontStyle: "italic" }}>
                  {credentialKind === "managed_identity"
                    ? "Uses the platform Managed Identity — no secret stored."
                    : "Uses Kubernetes Workload Identity federation — no secret stored."}
                </div>
              )}
            </div>

            <div style={fieldStyle}>
              <label style={labelStyle}>Description</label>
              <textarea
                style={{ ...inputStyle, resize: "vertical", minHeight: 60 }}
                placeholder="Optional description..."
                value={description}
                onChange={e => setDescription(e.target.value)}
                rows={2}
              />
            </div>
          </div>

          {/* Shown only once the storage actually differs from what the record
              held, so an operator correcting a description never sees it. */}
          {isEditing && storageChanged && (
            <div style={{ margin: "0 1.5rem 1rem", padding: "0.75rem 0.875rem", borderRadius: 8, flexShrink: 0,
                          border: "1px solid var(--warning)",
                          // The literal was the light theme's amber at 8%, so
                          // the wash did not follow the token the border uses.
                          background: "color-mix(in srgb, var(--warning) 10%, transparent)", fontSize: 12.5,
                          color: "var(--text-secondary)" }}>
              <div style={{ fontWeight: 600, color: "var(--text-primary)", marginBottom: 4 }}>
                This re-points {engineCatalog || "the catalog"}
              </div>
              Every table registered in it keeps its own relative location, so each will address a
              different object once this is synchronized to the Engine. Tables whose data has not
              moved with it will stop reading.
              <label style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 8,
                              color: "var(--text-primary)", fontWeight: 600 }}>
                <input type="checkbox" checked={storageChangeConfirmed}
                       onChange={(event) => setStorageChangeConfirmed(event.target.checked)} />
                The data has moved too, or this catalog holds no tables yet
              </label>
            </div>
          )}

          <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.75rem", padding: "1.25rem 1.5rem", borderTop: "1px solid var(--border)", background: "var(--bg-primary)", flexShrink: 0 }}>
            <Button type="button" variant="secondary" onClick={onClose} disabled={saving}>Cancel</Button>
            <Button type="submit" disabled={saving}>
              {saving ? <><i className="fas fa-spinner fa-spin" /> Saving...</> : <><i className={`fas fa-${isEditing ? "save" : "plus"}`} /> {isEditing ? "Update" : "Create"}</>}
            </Button>
          </div>
        </form>
      </div>
    </>
  );
}
