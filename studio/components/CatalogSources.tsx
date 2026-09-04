"use client";

import React, { useEffect, useState, useCallback } from "react";
import { API_BASE } from "../config";
import { msalFetch } from "../utils/msalFetch";
import { useAuth } from "../auth/useAuth";
import { Button } from "./Button";
import { Pagination } from "./Pagination";

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

const STORAGE_META: Record<string, { label: string; color: string; bg: string; border: string }> = {
  local:    { label: "Local",     color: "#065f46", bg: "#d1fae5", border: "#6ee7b7" },
  adls_gen2:{ label: "ADLS Gen2", color: "#0078D4", bg: "#eff6ff", border: "#93c5fd" },
  s3:       { label: "S3",        color: "#c2410c", bg: "#fff7ed", border: "#fdba74" },
};

const FORMAT_META: Record<string, { label: string; color: string; bg: string }> = {
  parquet: { label: "Parquet", color: "#7c3aed", bg: "#f5f3ff" },
  delta:   { label: "Delta",  color: "#0891b2", bg: "#ecfeff" },
  iceberg: { label: "Iceberg",color: "#4338ca", bg: "#eef2ff" },
};

const ADAPTER_META: Record<string, { label: string }> = {
  native:          { label: "Native" },
  hive_metastore:  { label: "Hive Metastore" },
  aws_glue:        { label: "AWS Glue" },
  unity_catalog:   { label: "Unity Catalog" },
  iceberg_rest:    { label: "Iceberg REST" },
};

const LIFECYCLE_META: Record<string, { label: string; color: string; bg: string }> = {
  draft:     { label: "Draft",     color: "#6b7280", bg: "var(--bg-hover)" },
  active:    { label: "Active",    color: "#065f46", bg: "#d1fae5" },
  suspended: { label: "Suspended", color: "#92400e", bg: "#fef3c7" },
  deleting:  { label: "Deleting",  color: "#991b1b", bg: "#fee2e2" },
  deleted:   { label: "Deleted",   color: "#6b7280", bg: "var(--bg-hover)" },
};

const LIFECYCLE_ACTIONS: Record<string, { label: string; target: string; icon: string }[]> = {
  draft:     [{ label: "Activate", target: "active", icon: "fa-check-circle" }, { label: "Delete", target: "deleted", icon: "fa-trash" }],
  active:    [{ label: "Suspend", target: "suspended", icon: "fa-pause-circle" }, { label: "Decommission", target: "deleting", icon: "fa-archive" }],
  suspended: [{ label: "Reactivate", target: "active", icon: "fa-play-circle" }, { label: "Decommission", target: "deleting", icon: "fa-archive" }],
  deleting:  [{ label: "Confirm Delete", target: "deleted", icon: "fa-trash" }],
  deleted:   [],
};

function Badge({ meta }: { meta: { label: string; color: string; bg: string; border?: string } }) {
  return (
    <span style={{
      display: "inline-flex", alignItems: "center", gap: 4,
      padding: "0.15rem 0.5rem", borderRadius: 6, fontSize: 12, fontWeight: 600,
      background: meta.bg, color: meta.color,
      border: meta.border ? `1px solid ${meta.border}` : undefined,
      whiteSpace: "nowrap",
    }}>
      {meta.label}
    </span>
  );
}

function parseConfig(raw: string): Record<string, string> {
  try { return JSON.parse(raw) || {}; } catch { return {}; }
}

function storageDisplay(cs: CatalogSource): string {
  const cfg = parseConfig(cs.storage_config);
  if (cs.storage_type === "local") return cfg.base_path || "/data";
  if (cs.storage_type === "adls_gen2") return `${cfg.account}/${cfg.container}`;
  if (cs.storage_type === "s3") return `${cfg.bucket}/${cfg.prefix || ""}`;
  return "—";
}

export function CatalogSources() {
  const { isAuthenticated } = useAuth();
  const [sources, setSources] = useState<CatalogSource[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showModal, setShowModal] = useState(false);
  const [editing, setEditing] = useState<CatalogSource | null>(null);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);
  const PAGE_SIZE = 20;

  const load = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      const res = await msalFetch(`${API_BASE}/api/v1/catalog-sources`);
      if (!res.ok) throw new Error("Failed to load catalog sources");
      const data = await res.json();
      setSources(data.catalogSources || []);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (isAuthenticated) load();
  }, [isAuthenticated, load]);

  const transitionLifecycle = async (cs: CatalogSource, target: string) => {
    if (target === "deleted" && !confirm(`Permanently delete "${cs.name}"?`)) return;
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/catalog-sources/${cs.id}/transition`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ lifecycle: target }),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        throw new Error(err.detail || "Transition failed");
      }
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Transition failed");
    }
  };

  const deletePermanently = async (cs: CatalogSource) => {
    if (!confirm(`Permanently remove "${cs.name}"? This cannot be undone.`)) return;
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/catalog-sources/${cs.id}`, { method: "DELETE" });
      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        throw new Error(err.detail || "Delete failed");
      }
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed");
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
              <span style={{ fontSize: 13, color: "var(--success)", fontWeight: 600 }}>{activeCount} active</span>
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
        <div style={{
          padding: "0.75rem 1rem", borderRadius: 8, marginBottom: "1rem",
          background: "rgba(239,68,68,0.08)", color: "var(--error)",
          border: "1px solid rgba(239,68,68,0.3)",
          display: "flex", alignItems: "center", gap: "0.75rem", fontSize: 13,
        }}>
          <i className="fas fa-exclamation-circle" />
          {error}
          <button onClick={() => setError(null)} style={{ marginLeft: "auto", background: "none", border: "none", color: "inherit", cursor: "pointer" }}>
            <i className="fas fa-times" />
          </button>
        </div>
      )}

      {sources.length === 0 ? (
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
                        <Badge meta={stMeta} />
                        <div className="muted" style={{ marginTop: 3, fontSize: 11, fontFamily: "monospace", maxWidth: 200, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                          {storageDisplay(cs)}
                        </div>
                      </td>
                      <td><Badge meta={fmtMeta} /></td>
                      <td style={{ fontSize: 12, color: "var(--text-secondary)" }}>
                        {ADAPTER_META[cs.adapter_type]?.label || cs.adapter_type}
                      </td>
                      <td>
                        {cs.credential_kind ? (
                          <span style={{ fontSize: 12, color: "var(--success)" }}>
                            <i className="fas fa-shield-alt" style={{ marginRight: 4 }} />
                            {cs.credential_kind === "secret_store" ? "Key Vault" : cs.credential_kind.replace(/_/g, " ")}
                          </span>
                        ) : (
                          <span style={{ fontSize: 12, color: "var(--text-muted)" }}>None</span>
                        )}
                      </td>
                      <td><Badge meta={lcMeta} /></td>
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
                          <button type="button" className="action-icon-btn" title="Edit" onClick={() => { setEditing(cs); setShowModal(true); }}>
                            <i className="fas fa-edit" />
                          </button>
                          {cs.lifecycle === "draft" && (
                            <button type="button" className="action-icon-btn" title="Delete" onClick={() => deletePermanently(cs)}>
                              <i className="fas fa-trash" />
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
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
      if (credentialKind) {
        body.credential_kind = credentialKind;
        body.credential_ref = credentialRef.trim() || null;
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
        const err = await res.json().catch(() => ({}));
        throw new Error(err.detail || `Failed to ${isEditing ? "update" : "create"}`);
      }
      onSuccess();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Operation failed");
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

  const chipStyle = (selected: boolean, color: string): React.CSSProperties => ({
    display: "inline-flex", alignItems: "center", gap: 4,
    padding: "0.4rem 0.75rem", borderRadius: 8, fontSize: 13, cursor: "pointer",
    border: `2px solid ${selected ? color : "var(--border)"}`,
    background: selected ? `${color}14` : "var(--bg-surface)",
    color: selected ? color : "var(--text-secondary)",
    fontWeight: selected ? 600 : 400, transition: "all 0.15s",
  });

  return (
    <>
      <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.5)", zIndex: 1000 }} onClick={onClose} />
      <div style={{
        position: "fixed", top: "50%", left: "50%", transform: "translate(-50%,-50%)",
        background: "var(--bg-surface)", borderRadius: 12, boxShadow: "var(--shadow-lg)",
        zIndex: 1001, width: "90%", maxWidth: 680, maxHeight: "90vh", overflow: "hidden",
        display: "flex", flexDirection: "column",
      }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "1.25rem 1.5rem", borderBottom: "1px solid var(--border)" }}>
          <h2 style={{ margin: 0, fontSize: "1.25rem", fontWeight: 600 }}>
            {isEditing ? "Edit Catalog Source" : "New Catalog Source"}
          </h2>
          <button onClick={onClose} style={{ background: "none", border: "none", fontSize: "1.25rem", cursor: "pointer", color: "var(--text-muted)", borderRadius: 6, width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center" }}>
            <i className="fas fa-times" />
          </button>
        </div>

        <form onSubmit={handleSubmit}>
          <div style={{ padding: "1.25rem 1.5rem", overflowY: "auto", flex: 1 }}>
            {error && (
              <div style={{ padding: "0.75rem 1rem", borderRadius: 8, marginBottom: "1rem", background: "rgba(239,68,68,0.08)", color: "var(--error)", border: "1px solid rgba(239,68,68,0.3)", display: "flex", alignItems: "center", gap: "0.75rem", fontSize: 13 }}>
                <i className="fas fa-exclamation-circle" />
                {error}
              </div>
            )}

            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "1rem", marginBottom: "1rem" }}>
              <div>
                <label style={labelStyle}>Display Name *</label>
                <input style={inputStyle} placeholder="e.g., Production Lakehouse" value={name} onChange={e => setName(e.target.value)} required />
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
                  <button key={key} type="button" style={chipStyle(storageType === key, meta.color)} onClick={() => setStorageType(key)}>
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
                  <label style={labelStyle}>Root Path *</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="warehouse/" value={adlsRootPath} onChange={e => setAdlsRootPath(e.target.value)} required />
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
                <label style={labelStyle}>Data Format *</label>
                <div style={{ display: "flex", gap: "0.5rem" }}>
                  {Object.entries(FORMAT_META).map(([key, meta]) => (
                    <button key={key} type="button" style={chipStyle(dataFormat === key, meta.color)} onClick={() => setDataFormat(key)}>
                      {meta.label}
                    </button>
                  ))}
                </div>
              </div>
              <div>
                <label style={labelStyle}>Catalog Adapter</label>
                <select
                  value={adapterType}
                  onChange={e => setAdapterType(e.target.value)}
                  style={{ ...inputStyle, cursor: "pointer" }}
                >
                  {Object.entries(ADAPTER_META).map(([key, meta]) => (
                    <option key={key} value={key}>{meta.label}</option>
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
                <button type="button" style={chipStyle(!credentialKind, "#6b7280")} onClick={() => { setCredentialKind(""); setCredentialRef(""); }}>
                  None
                </button>
                <button type="button" style={chipStyle(credentialKind === "managed_identity", "#065f46")} onClick={() => setCredentialKind("managed_identity")}>
                  Managed Identity
                </button>
                <button type="button" style={chipStyle(credentialKind === "workload_identity", "#0078D4")} onClick={() => setCredentialKind("workload_identity")}>
                  Workload Identity
                </button>
                <button type="button" style={chipStyle(credentialKind === "secret_store", "#7c3aed")} onClick={() => setCredentialKind("secret_store")}>
                  Key Vault
                </button>
              </div>
              {credentialKind === "secret_store" && (
                <div>
                  <label style={labelStyle}>Key Vault URI *</label>
                  <input style={{ ...inputStyle, fontFamily: "monospace" }} placeholder="https://kv-kaveon.vault.azure.net/secrets/adls-key" value={credentialRef} onChange={e => setCredentialRef(e.target.value)} required />
                </div>
              )}
              {credentialKind && credentialKind !== "secret_store" && (
                <div style={{ fontSize: 12, color: "var(--text-muted)", fontStyle: "italic" }}>
                  {credentialKind === "managed_identity" && "Uses the platform Managed Identity — no secret stored."}
                  {credentialKind === "workload_identity" && "Uses Kubernetes Workload Identity federation — no secret stored."}
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

          <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.75rem", padding: "1.25rem 1.5rem", borderTop: "1px solid var(--border)", background: "var(--bg-primary)" }}>
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
