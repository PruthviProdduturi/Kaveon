"use client";

import React, { useEffect, useId, useRef, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useAuth } from "../../../auth/useAuth";
import { Button } from "../../../components/Button";
import { SettingsSection } from "../SettingsSection";
import s from "../settings.module.css";
import { Pagination } from "../../../components/Pagination";
import { SOURCE_TYPE_META, SOURCE_TYPES } from "../../../components/DataSourceIcons";
import { reasonFromStatus, unknownText } from "../../../components/Reading";

function SourceTypeBadge({ type }: { type: string }) {
  const meta = SOURCE_TYPE_META[type];
  if (!meta) return <span className="muted" style={{ fontSize: 13 }}>{type}</span>;
  return (
    <span style={{
      display: "inline-flex", alignItems: "center", gap: 5,
      padding: "0.2rem 0.55rem", borderRadius: 7, fontSize: 12, fontWeight: 500,
      background: meta.bg, border: `1px solid ${meta.border}`, color: meta.color,
      whiteSpace: "nowrap",
    }}>
      {meta.icon}
      {meta.label}
    </span>
  );
}

interface DataSource {
  id: number;
  name: string;
  type: string;
  /**
   * Never present. `api/routers/data_sources._PUBLIC_FIELDS` withholds the
   * endpoint deliberately — it may carry credentials — so this is always
   * undefined in the browser. It was typed as a required string, which is why
   * a column and a search both read it and the edit form seeded a required
   * input from it.
   */
  connection_string?: undefined;
  database_name?: string;
  region: "WW" | "EU";
  description?: string;
  created_by?: string;
  created_at?: string;
  updated_at?: string;
  is_active: boolean;
  /** 1/0 from the SQL path, true/false from the KaveonDB path — read as truthy. */
  is_favorite?: number | boolean;
  /**
   * Tables in this source, as the list endpoint reports it. It answers 0 both
   * for a source it did not count and for one holding nothing — the cutover
   * path returns 0 for every source — so 0 is not a measurement.
   */
  table_count?: number;
}

export default function DataSourcesPage() {
  const { account, isAuthenticated } = useAuth();
  const [dataSources, setDataSources] = useState<DataSource[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showAddModal, setShowAddModal] = useState(false);
  const [editingDataSource, setEditingDataSource] = useState<DataSource | null>(null);
  const [copyingDataSource, setCopyingDataSource] = useState<DataSource | null>(null);
  const [deletingId, setDeletingId] = useState<number | null>(null);
  const [compilingContextId, setCompilingContextId] = useState<number | null>(null);
  const [contextMessage, setContextMessage] = useState<string | null>(null);
  const [tableCounts, setTableCounts] = useState<Record<number, number | null>>({});
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);

  useEffect(() => {
    if (!isAuthenticated) return;
    loadDataSources();
  }, [isAuthenticated]);

  const fetchTableCount = async (dataSourceId: number) => {
    try {
      const response = await msalFetch(`${API_BASE}/api/v1/data-sources/${dataSourceId}/table-count`);
      if (!response.ok) return null;
      const data = await response.json();
      return data.tableCount;
    } catch (err) {
      return null;
    }
  };

  const loadDataSources = async () => {
    try {
      setLoading(true);
      setError(null);
      const userEmail = account?.email || account?.username || null;
      const response = await msalFetch(`${API_BASE}/api/v1/data-sources`, {
        headers: userEmail ? { 'x-user-email': userEmail } : undefined
      });
      if (!response.ok) {
        // The status is what separates a server that was never told about a
        // data plane from one that is restarting, and "Failed to load data
        // sources" said neither. The sentence is the product's own.
        setError(unknownText({
          what: "The registered databases",
          says: "which databases are registered",
          reason: reasonFromStatus(response.status),
          detail: response.status === 403 ? "Your role does not include this." : null,
        }));
        return;
      }
      const data = await response.json();
      const sources = data.dataSources || [];
      setDataSources(sources);

      // Table counts are now included in the API response
      const counts: Record<number, number | null> = {};
      sources.forEach((ds: DataSource) => {
        counts[ds.id] = ds.table_count ?? null;
      });
      setTableCounts(counts);
    } catch (err) {
      setError(unknownText({
        what: "The registered databases",
        says: "which databases are registered",
        detail: err instanceof Error ? err.message : null,
      }));
    } finally {
      setLoading(false);
    }
  };

  const toggleFavorite = async (ds: DataSource, e: React.MouseEvent) => {
    e.stopPropagation();
    if (!isAuthenticated) return;

    const userEmail = account?.email || account?.username || null;
    if (!userEmail) {
      setError("User email not available");
      return;
    }

    // The two read paths answer 1/0 and true/false for this one fact, and a
    // strict `=== 1` was false for every KaveonDB-backed source: the star never
    // appeared and the toggle could only ever add, never remove.
    const isFavorite = !!ds.is_favorite;
    const newFavoriteValue = isFavorite ? 0 : 1;

    // Optimistic update - update UI immediately
    setDataSources(prevDataSources =>
      prevDataSources.map(source => {
        if (source.id === ds.id) {
          return { ...source, is_favorite: newFavoriteValue };
        }
        // If setting a new favorite, clear other favorites (only one favorite allowed)
        if (newFavoriteValue === 1 && source.is_favorite) {
          return { ...source, is_favorite: 0 };
        }
        return source;
      })
    );

    try {
      const method = isFavorite ? "DELETE" : "POST";

      const response = await msalFetch(`${API_BASE}/api/v1/data-sources/${ds.id}/favorite`, {
        method,
        headers: { 'x-user-email': userEmail }
      });

      if (!response.ok) {
        // Revert optimistic update on error
        setDataSources(prevDataSources =>
          prevDataSources.map(source =>
            source.id === ds.id ? { ...source, is_favorite: isFavorite ? 1 : 0 } : source
          )
        );
        throw new Error(`Failed to ${isFavorite ? 'remove' : 'set'} favorite`);
      }

      // Optionally reload to ensure consistency with server
      // await loadDataSources();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to toggle favorite");
    }
  };

  const toggleDataSource = async (ds: DataSource, e: React.MouseEvent) => {
    e.stopPropagation();

    const newActiveState = !ds.is_active;

    // Optimistic update - update UI immediately
    setDataSources(prevDataSources =>
      prevDataSources.map(source =>
        source.id === ds.id ? { ...source, is_active: newActiveState } : source
      )
    );

    try {
      const response = await msalFetch(`${API_BASE}/api/v1/data-sources/${ds.id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ is_active: newActiveState }),
      });

      if (!response.ok) {
        // Revert optimistic update on error
        setDataSources(prevDataSources =>
          prevDataSources.map(source =>
            source.id === ds.id ? { ...source, is_active: ds.is_active } : source
          )
        );
        throw new Error("Failed to update data source");
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to update data source");
    }
  };

  const deleteDataSource = async (ds: DataSource, e: React.MouseEvent) => {
    e.stopPropagation();
    if (!confirm(`Delete data source "${ds.name}"? This cannot be undone.`)) return;

    setDeletingId(ds.id);
    try {
      const response = await msalFetch(`${API_BASE}/api/v1/data-sources/${ds.id}`, {
        method: "DELETE",
      });
      if (!response.ok && response.status !== 204) throw new Error("Failed to delete data source");
      await loadDataSources();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to delete data source");
    } finally {
      setDeletingId(null);
    }
  };

  // Context is compiled per dataset by the DLM, so a source-level compile is the
  // sum of its datasets: recompile each one that reads from this connection.
  const compileContext = async (ds: DataSource, e: React.MouseEvent) => {
    e.stopPropagation();

    const database = (ds.database_name || ds.name).toLowerCase();
    setCompilingContextId(ds.id);
    setContextMessage(null);
    setError(null);
    try {
      const response = await msalFetch(`${API_BASE}/api/v1/datasets`);
      if (!response.ok) throw new Error("Failed to load the datasets for this data source");
      const data = await response.json();
      const all = Array.isArray(data) ? data : (data.datasets || data.recent || []);
      const owned = all.filter((d: any) => (d.database_name || "").toLowerCase() === database);

      if (owned.length === 0) {
        setError(`No datasets read from ${ds.name}, so there is nothing to compile. Create a dataset on this source first, then compile its context.`);
        return;
      }

      let compiled = 0;
      for (const d of owned) {
        try {
          const genRes = await msalFetch(`${API_BASE}/api/v1/datasets/${d.id}/dlm/generate?force=true`, { method: "POST" });
          if (genRes.ok) compiled += 1;
        } catch {
          // Counted as a failure below; the per-dataset page reports the reason.
        }
      }
      const failed = owned.length - compiled;

      if (compiled === 0) {
        setError(`Context compilation failed for all ${failed} dataset${failed !== 1 ? "s" : ""} on ${ds.name}. Open a dataset to see why.`);
        return;
      }
      setContextMessage(failed === 0
        ? `Context compiled for ${compiled} dataset${compiled !== 1 ? "s" : ""} on ${ds.name}.`
        : `Context compiled for ${compiled} of ${owned.length} datasets on ${ds.name}; ${failed} could not be compiled.`);
      setTimeout(() => setContextMessage(null), 4000);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to compile dataset context");
    } finally {
      setCompilingContextId(null);
    }
  };

  // Name and description only: the endpoint is not returned to the browser, so
  // a search over it could never match and the box quietly claimed otherwise.
  const filtered = dataSources.filter(ds =>
    !search || ds.name.toLowerCase().includes(search.toLowerCase()) ||
    (ds.description ?? "").toLowerCase().includes(search.toLowerCase())
  );
  const handleSearch = (q: string) => { setSearch(q); setPage(1); };
  const PAGE_SIZE = 20;
  const paged = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const activeCount = dataSources.filter(ds => ds.is_active).length;
  const userEmail = account?.email || account?.username || null;
  const myCount = userEmail ? dataSources.filter(ds => ds.created_by === userEmail).length : 0;
  const othersCount = dataSources.length - myCount;

  // Engine catalog sources used to live behind a second tab here and are the
  // same component Settings -> Storage renders, so registering a lake was two
  // places with one meaning. This page is now only the external databases;
  // Storage is the lake.
  return (
    <>
      <SettingsSection
        icon="fa-server"
        title="External databases"
        subtitle="Databases Kaveon queries over the wire, as a registered connection. The lake KaveonDB reads in place is Storage; a catalog there needs no connection of its own."
        pills={!loading && !error && dataSources.length > 0 ? [
          { label: `${dataSources.length} registered`, icon: "fa-server" },
          ...(activeCount > 0 ? [{ label: `${activeCount} active`, icon: "fa-check-circle" }] : []),
          ...(myCount > 0 ? [{ label: `${myCount} added by you`, icon: "fa-user" }] : []),
        ] : []}
        action={<Button onClick={() => setShowAddModal(true)}><i className="fas fa-plus" /> Register a database</Button>}
        loading={loading}
        loadingMessage="Reading registered databases"
        error={error}
        onRetry={() => void loadDataSources()}
      >
        {!loading && !error && dataSources.length === 0 ? (
          // The old copy here read "No data sources yet - add your first
          // database connection to get started", which told an administrator
          // the platform was unconfigured. It is not: this demo reads Delta in
          // the lake through KaveonDB, and an external database is an addition
          // to that, not a prerequisite.
          <section className={s.card}>
            <p className={s.note}>
              No external databases are registered, and nothing here is waiting on one. Kaveon
              reads this deployment&rsquo;s lake in place through KaveonDB&rsquo;s catalog
              sources &mdash; see <a href="/settings/storage">Storage</a>. Register a database
              here only to query one that lives outside the lake: Fabric SQL, Azure SQL,
              PostgreSQL, MySQL or StarRocks.
            </p>
          </section>
        ) : !loading && !error && (
          <div className="card">
            <div style={{ display: "flex", justifyContent: "flex-end", marginBottom: "1rem" }}>
              <input
                type="search"
                value={search}
                onChange={event => handleSearch(event.target.value)}
                placeholder="Search registered databases"
                aria-label="Search registered databases"
                style={{
                  height: 32, padding: "0 10px", fontSize: 13, minWidth: 240,
                  color: "var(--text-primary)", background: "var(--bg-primary)",
                  border: "1px solid var(--border)", borderRadius: 8,
                }}
              />
            </div>
            {contextMessage && (
              <div style={{
                padding: "0.75rem 1rem", borderRadius: 8, marginBottom: "1rem",
                background: "color-mix(in srgb, var(--success) 8%, transparent)", color: "var(--success)",
                border: "1px solid color-mix(in srgb, var(--success) 30%, transparent)",
                display: "flex", alignItems: "center", gap: "0.75rem", fontSize: 13,
              }}>
                <i className="fas fa-check-circle" />
                {contextMessage}
              </div>
            )}
            <div className="results-table-container">
              <table className="results-table">
                <thead>
                  <tr>
                    <th><span className="column-header-label">Name</span></th>
                    <th><span className="column-header-label">Type</span></th>
                    <th><span className="column-header-label">Endpoint</span></th>
                    <th><span className="column-header-label">Database</span></th>
                    <th><span className="column-header-label">Region</span></th>
                    <th><span className="column-header-label">Status</span></th>
                    <th><span className="column-header-label">Tables</span></th>
                    <th><span className="column-header-label">Actions</span></th>
                  </tr>
                </thead>
                <tbody>
                  {paged.map(ds => (
                    <tr key={ds.id}>
                      <td>
                        <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
                          <strong>{ds.name}</strong>
                          {!!ds.is_favorite && <i className="fas fa-star" style={{ color: "var(--warning)", fontSize: "0.85rem" }} title="Favorite data source" />}
                        </div>
                        {ds.description && <div className="muted" style={{ marginTop: 4, fontSize: 13 }}>{ds.description}</div>}
                      </td>
                      <td><SourceTypeBadge type={ds.type} /></td>
                      {/* The endpoint is stored encrypted and is never returned
                          to the browser, so this cell was blank on every row of
                          a column headed "Endpoint". It says why instead. */}
                      <td className="muted" style={{ fontSize: 12, maxWidth: 300, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
                          title="Stored encrypted on the server and never sent to the browser.">
                        Not shown
                      </td>
                      <td className="muted" style={{ fontSize: 13 }}>{ds.database_name || "—"}</td>
                      <td>
                        <span style={{ padding: "0.2rem 0.5rem", borderRadius: 6, fontSize: "0.75rem", fontWeight: 600, background: ds.region === "WW" ? "color-mix(in srgb, var(--accent) 12%, transparent)" : "var(--bg-hover)", color: ds.region === "WW" ? "var(--accent)" : "var(--text-secondary)" }}>
                          {ds.region}
                        </span>
                      </td>
                      <td>
                        <span style={{ padding: "0.2rem 0.5rem", borderRadius: 6, fontSize: "0.75rem", fontWeight: 600, background: ds.is_active ? "color-mix(in srgb, var(--success) 12%, transparent)" : "var(--bg-hover)", color: ds.is_active ? "var(--success)" : "var(--text-muted)" }}>
                          {ds.is_active ? "Active" : "Inactive"}
                        </span>
                      </td>
                      {/* A count the server did not take comes back as 0, the
                          same value a source holding nothing would give — and
                          it was written in success green as "0 tables", which
                          is a measurement this page never made. Only a figure
                          above zero is reported, and it is reported in the
                          table's own ink: a count is not a success. */}
                      <td className="muted" style={{ fontSize: 13 }}>
                        {tableCounts[ds.id] === undefined
                          ? <i className="fas fa-spinner fa-spin" style={{ fontSize: 11, color: "var(--accent)" }} aria-label="Reading" />
                          : tableCounts[ds.id] ? <span style={{ fontWeight: 600 }}>{tableCounts[ds.id]} tables</span>
                          : <span style={{ color: "var(--text-muted)" }} title="No table count was taken for this source.">—</span>}
                      </td>
                      <td className="actions-cell">
                        <div className="row-actions">
                          <button type="button" className="action-icon-btn" aria-label={ds.is_favorite ? `Remove ${ds.name} from favorites` : `Set ${ds.name} as the favorite`} title={ds.is_favorite ? "Remove favorite" : "Set as favorite"} onClick={e => toggleFavorite(ds, e)} style={{ color: ds.is_favorite ? "var(--warning)" : undefined }}>
                            <i className={ds.is_favorite ? "fas fa-star" : "far fa-star"} aria-hidden="true" />
                          </button>
                          <button type="button" className="action-icon-btn" title={ds.is_active ? "Deactivate" : "Activate"} onClick={e => toggleDataSource(ds, e)}>
                            <i className={`fas fa-${ds.is_active ? "pause" : "play"}-circle`} />
                          </button>
                          <button type="button" className="action-icon-btn" title="Copy data source" onClick={e => { e.stopPropagation(); setCopyingDataSource(ds); }}>
                            <i className="fas fa-copy" />
                          </button>
                          <button type="button" className="action-icon-btn" title="Compile DLM context for every dataset on this source" onClick={e => compileContext(ds, e)} disabled={compilingContextId === ds.id}>
                            <i className={compilingContextId === ds.id ? "fas fa-spinner fa-spin" : "fas fa-brain"} />
                          </button>
                          <button type="button" className="action-icon-btn" title="Edit data source" onClick={e => { e.stopPropagation(); setEditingDataSource(ds); }}>
                            <i className="fas fa-edit" />
                          </button>
                          <button type="button" className="action-icon-btn" title="Delete data source" onClick={e => deleteDataSource(ds, e)} disabled={deletingId === ds.id}>
                            <i className={deletingId === ds.id ? "fas fa-spinner fa-spin" : "fas fa-trash"} />
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <Pagination total={filtered.length} page={page} pageSize={PAGE_SIZE} onChange={setPage} />
          </div>
        )}
      </SettingsSection>

      {showAddModal && <AddDataSourceModal onClose={() => setShowAddModal(false)} onSuccess={() => { setShowAddModal(false); loadDataSources(); }} />}
      {editingDataSource && <AddDataSourceModal dataSource={editingDataSource} onClose={() => setEditingDataSource(null)} onSuccess={() => { setEditingDataSource(null); loadDataSources(); }} />}
      {copyingDataSource && <AddDataSourceModal dataSource={copyingDataSource} isCopying={true} onClose={() => setCopyingDataSource(null)} onSuccess={() => { setCopyingDataSource(null); loadDataSources(); }} />}
    </>
  );
}

interface AddDataSourceModalProps {
  dataSource?: DataSource;
  isCopying?: boolean;
  onClose: () => void;
  onSuccess: () => void;
}

function AddDataSourceModal({ dataSource, isCopying = false, onClose, onSuccess }: AddDataSourceModalProps) {
  const { account } = useAuth();
  const isEditing = !!dataSource && !isCopying;
  const titleId = useId();
  const dialog = useRef<HTMLDivElement>(null);
  // Focus moves into the dialog on open, so Escape reaches the handler below
  // and a keyboard reader is not left behind the overlay.
  useEffect(() => { dialog.current?.focus(); }, []);
  const [formData, setFormData] = useState({
    name: isCopying ? `${dataSource?.name} (Copy)` : (dataSource?.name || ""),
    type: dataSource?.type || "Fabric SQL AEP",
    // Always empty: the API withholds the stored endpoint. On an edit this
    // field therefore means "replace it", not "this is what it is".
    connection_string: "",
    database_name: dataSource?.database_name || "",
    region: (dataSource?.region || "WW") as "WW" | "EU",
    description: dataSource?.description || "",
  });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      setSaving(true);
      setError(null);

      const url = isEditing
        ? `${API_BASE}/api/v1/data-sources/${dataSource.id}`
        : `${API_BASE}/api/v1/data-sources`;

      // The stored endpoint never reaches the browser, so this form cannot
      // show it and must not claim to. Left blank on an edit, the field is
      // omitted and the server leaves the endpoint as it stands — it writes
      // `connection_string` only when the key is present. Sending the empty
      // string instead would have been refused outright, and sending a
      // retyped guess would re-encrypt it over credentials the operator
      // could not read and therefore could not reproduce.
      const body: Record<string, unknown> = { ...formData };
      if (isEditing && !formData.connection_string.trim()) delete body.connection_string;

      const response = await msalFetch(url, {
        method: isEditing ? "PATCH" : "POST",
        headers: {
          "Content-Type": "application/json",
          "x-user-email": account?.email || account?.username || "",
        },
        body: JSON.stringify(body),
      });

      if (!response.ok) {
        // FastAPI answers `{"detail": ...}`. Reading `message` discarded every
        // reason the server gave and showed the generic fallback instead.
        const errorBody = await response.json().catch(() => null) as { detail?: unknown; message?: unknown } | null;
        const detail = errorBody?.detail;
        const text = typeof detail === "string" ? detail
          : Array.isArray(detail)
            ? detail.map(item => (item && typeof item === "object" && typeof (item as { msg?: unknown }).msg === "string"
                ? (item as { msg: string }).msg : null)).filter(Boolean).join(" ") || null
            : typeof (detail as { message?: unknown } | undefined)?.message === "string"
              ? (detail as { message: string }).message
              : typeof errorBody?.message === "string" ? errorBody.message : null;
        throw new Error(text || `Failed to ${isEditing ? 'update' : 'create'} data source (HTTP ${response.status}).`);
      }

      onSuccess();
    } catch (err) {
      setError(err instanceof Error ? err.message : `Failed to ${isEditing ? 'update' : 'create'} data source`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <div
        style={{
          position: 'fixed',
          inset: 0,
          // A scrim, not chrome: it dims whatever is behind it, so it is the
          // same black in both themes rather than a themed surface.
          background: 'rgba(0, 0, 0, 0.5)',
          zIndex: 1000,
        }}
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={event => { if (event.key === "Escape" && !saving) { event.stopPropagation(); onClose(); } }}
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          background: "var(--bg-surface)",
          borderRadius: '12px',
          boxShadow: "var(--shadow-lg)",
          zIndex: 1001,
          width: '90%',
          maxWidth: '600px',
          maxHeight: '90vh',
          overflow: 'hidden',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
            padding: '1.5rem',
            borderBottom: "1px solid var(--border)",
          }}
        >
          <h2 id={titleId} style={{ margin: 0, fontSize: '1.5rem', fontWeight: 600 }}>
            {isEditing ? 'Edit Data Source' : isCopying ? 'Copy Data Source' : 'Add Data Source'}
          </h2>
          <button
            onClick={onClose}
            style={{
              background: 'none',
              border: 'none',
              fontSize: '1.5rem',
              cursor: 'pointer',
              color: "var(--text-muted)",
              padding: '0.25rem',
              width: 32,
              height: 32,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              borderRadius: '6px',
            }}
          >
            <i className="fas fa-times" />
          </button>
        </div>

        {/* The form is the dialog's flex child, so the body below it has a
            bounded height and actually scrolls. As a plain block child, its
            `flex: 1` resolved against nothing: the form grew past the
            dialog's 90vh and `overflow: hidden` clipped the footer, leaving
            Cancel and Update unreachable on a short viewport. */}
        <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
          <div style={{ padding: '1.5rem', overflowY: 'auto', flex: 1, minHeight: 0 }}>
            {error && (
              <div
                style={{
                  padding: '1rem',
                  borderRadius: '8px',
                  marginBottom: '1.5rem',
                  background: "color-mix(in srgb, var(--danger) 8%, transparent)",
                  color: "var(--danger)",
                  border: "1px solid color-mix(in srgb, var(--danger) 30%, transparent)",
                  display: 'flex',
                  alignItems: 'center',
                  gap: '0.75rem',
                }}
              >
                <i className="fas fa-exclamation-circle" />
                {error}
              </div>
            )}

            <div className="chart-builder-field">
              <label className="chart-builder-label">
                <span>Name *</span>
              </label>
              <input
                type="text"
                className="chart-builder-input"
                placeholder="e.g., Production Database"
                value={formData.name}
                onChange={(e) => setFormData({ ...formData, name: e.target.value })}
                required
              />
            </div>

            <div className="chart-builder-field">
              <label className="chart-builder-label">
                <span>Type *</span>
              </label>
              <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem" }}>
                {SOURCE_TYPES.map(t => {
                  const meta = SOURCE_TYPE_META[t];
                  const selected = formData.type === t;
                  return (
                    <button
                      key={t}
                      type="button"
                      onClick={() => setFormData({ ...formData, type: t })}
                      style={{
                        display: "flex", alignItems: "center", gap: "0.5rem",
                        padding: "0.5rem 0.85rem", borderRadius: 9, cursor: "pointer",
                        border: `2px solid ${selected ? meta.color : "var(--border)"}`,
                        background: selected ? meta.bg : "var(--bg-surface)",
                        color: selected ? meta.color : "var(--text-secondary)",
                        fontWeight: selected ? 600 : 400,
                        fontSize: 13, transition: "all 0.15s",
                        boxShadow: selected ? `0 0 0 2px ${meta.color}20` : "none",
                      }}
                    >
                      {meta.icon}
                      {meta.label}
                    </button>
                  );
                })}
              </div>
            </div>

            <div className="chart-builder-field">
              <label className="chart-builder-label">
                <span>
                  {SOURCE_TYPE_META[formData.type]?.endpointLabel ?? "Endpoint"}
                  {isEditing ? "" : " *"}
                </span>
              </label>
              <input
                type="text"
                className="chart-builder-input"
                style={{ fontFamily: 'monospace' }}
                placeholder={isEditing
                  ? "Unchanged"
                  : SOURCE_TYPE_META[formData.type]?.endpointPlaceholder ?? ""}
                value={formData.connection_string}
                onChange={(e) => setFormData({ ...formData, connection_string: e.target.value })}
                required={!isEditing}
                aria-describedby={isEditing ? "endpoint-note" : undefined}
              />
              {isEditing && (
                <p className="muted" id="endpoint-note" style={{ marginTop: 6, fontSize: 12 }}>
                  The stored endpoint is held encrypted and is not shown here. Leave this blank to
                  keep it; type a new one only to replace it.
                </p>
              )}
            </div>

            <div className="chart-builder-field">
              <label className="chart-builder-label">
                <span>{"Trino" === formData.type ? "Catalog Name *" : "StarRocks" === formData.type ? "Database Name (optional)" : "Database Name *"}</span>
              </label>
              <input
                type="text"
                className="chart-builder-input"
                style={{ fontFamily: 'monospace' }}
                placeholder={formData.type === "Trino" ? "e.g., hive" : formData.type === "StarRocks" ? "e.g., my_database" : "e.g., my_database"}
                value={formData.database_name}
                onChange={(e) => setFormData({ ...formData, database_name: e.target.value })}
                required={formData.type !== "StarRocks"}
              />
            </div>

            <div className="chart-builder-field">
              <label className="chart-builder-label">
                <span>Region</span>
              </label>
              <div style={{ display: 'flex', alignItems: 'center', gap: '0.75rem' }}>
                <span style={{ fontSize: 13, padding: "0.3rem 0.75rem", borderRadius: 7, background: "color-mix(in srgb, var(--accent) 12%, transparent)", color: "var(--accent)", border: "1px solid color-mix(in srgb, var(--accent) 30%, transparent)", fontWeight: 600 }}>
                  WW — Worldwide
                </span>
                <span style={{ fontSize: 12, color: "var(--text-muted)" }}>EU coming soon</span>
              </div>
            </div>

            <div className="chart-builder-field">
              <label className="chart-builder-label">
                <span>Description</span>
              </label>
              <textarea
                className="chart-builder-textarea"
                rows={3}
                placeholder="Optional description..."
                value={formData.description}
                onChange={(e) => setFormData({ ...formData, description: e.target.value })}
              />
            </div>
          </div>

          <div
            style={{
              display: 'flex',
              justifyContent: 'flex-end',
              gap: '0.75rem',
              padding: '1.5rem',
              borderTop: "1px solid var(--border)",
              background: "var(--bg-primary)",
            }}
          >
            <Button
              type="button"
              variant="secondary"
              onClick={onClose}
              disabled={saving}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              disabled={saving}
            >
              {saving ? (
                <>
                  <i className="fas fa-spinner fa-spin" /> {isEditing ? 'Updating...' : 'Creating...'}
                </>
              ) : (
                <>
                  <i className={`fas fa-${isEditing ? 'save' : 'plus'}`} /> {isEditing ? 'Update Data Source' : 'Add Data Source'}
                </>
              )}
            </Button>
          </div>
        </form>
      </div>
    </>
  );
}
