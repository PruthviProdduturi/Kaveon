"use client";

import { useEffect, useState } from "react";
import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { Button } from "../../components/Button";
import { ListPageShell } from "../../components/ListPageShell";
import { Pagination } from "../../components/Pagination";

interface DataSource {
  id: number;
  name: string;
  type: string;
  connection_string: string;
  database_name?: string;
  region: "WW" | "EU";
  description?: string;
  created_by?: string;
  created_at?: string;
  updated_at?: string;
  is_active: boolean;
  is_favorite?: number; // 1 if favorite, 0 if not
  table_count?: number; // Number of tables in this data source
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
      if (!response.ok) throw new Error("Failed to load data sources");
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
      setError(err instanceof Error ? err.message : "Failed to load data sources");
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

    const isFavorite = ds.is_favorite === 1;
    const newFavoriteValue = isFavorite ? 0 : 1;

    // Optimistic update - update UI immediately
    setDataSources(prevDataSources =>
      prevDataSources.map(source => {
        if (source.id === ds.id) {
          return { ...source, is_favorite: newFavoriteValue };
        }
        // If setting a new favorite, clear other favorites (only one favorite allowed)
        if (newFavoriteValue === 1 && source.is_favorite === 1) {
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

  const filtered = dataSources.filter(ds =>
    !search || ds.name.toLowerCase().includes(search.toLowerCase()) ||
    (ds.description ?? "").toLowerCase().includes(search.toLowerCase()) ||
    (ds.connection_string ?? "").toLowerCase().includes(search.toLowerCase())
  );
  const handleSearch = (q: string) => { setSearch(q); setPage(1); };
  const PAGE_SIZE = 20;
  const paged = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const activeCount = dataSources.filter(ds => ds.is_active).length;
  const userEmail = account?.email || account?.username || null;
  const myCount = userEmail ? dataSources.filter(ds => ds.created_by === userEmail).length : 0;
  const othersCount = dataSources.length - myCount;

  return (
    <>
      <ListPageShell
        icon="fa-server"
        title="Data Sources"
        subtitle="Manage database connections and switch between data sources."
        pills={!loading && !error ? [
          { label: `${dataSources.length} Total`, icon: "fa-server" },
          ...(myCount > 0 ? [{ label: `${myCount} Mine`, icon: "fa-user" }] : []),
          ...(othersCount > 0 ? [{ label: `${othersCount} Others`, icon: "fa-users", bg: "#f1f5f9", border: "#e2e8f0", color: "#64748b" }] : []),
          ...(activeCount > 0 ? [{ label: `${activeCount} Active`, icon: "fa-check-circle", bg: "#d1fae5", border: "#6ee7b7", color: "#065f46" }] : []),
        ] : []}
        action={
          <Button onClick={() => setShowAddModal(true)}>
            <i className="fas fa-plus" /> New Data Source
          </Button>
        }
        loading={loading}
        loadingMessage="Loading data sources"
        error={error}
        empty={!loading && !error && dataSources.length === 0}
        emptyTitle="No data sources yet"
        emptyBody="Add your first database connection to get started."
        emptyAction={<Button onClick={() => setShowAddModal(true)}><i className="fas fa-plus" /> Add Data Source</Button>}
        search={search}
        onSearch={handleSearch}
        resultCount={search ? filtered.length : undefined}
      >
        <div className="card">
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
                        {ds.is_favorite === 1 && <i className="fas fa-star" style={{ color: "#f59e0b", fontSize: "0.85rem" }} title="Favorite data source" />}
                      </div>
                      {ds.description && <div className="muted" style={{ marginTop: 4, fontSize: 13 }}>{ds.description}</div>}
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>{ds.type}</td>
                    <td className="muted" style={{ fontSize: 12, fontFamily: "monospace", maxWidth: 300, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{ds.connection_string}</td>
                    <td className="muted" style={{ fontSize: 13 }}>{ds.database_name || "—"}</td>
                    <td>
                      <span style={{ padding: "0.2rem 0.5rem", borderRadius: 6, fontSize: "0.75rem", fontWeight: 600, background: ds.region === "WW" ? "#dbeafe" : "#fef3c7", color: ds.region === "WW" ? "#1e40af" : "#92400e" }}>
                        {ds.region}
                      </span>
                    </td>
                    <td>
                      <span style={{ padding: "0.2rem 0.5rem", borderRadius: 6, fontSize: "0.75rem", fontWeight: 600, background: ds.is_active ? "#d1fae5" : "#f3f4f6", color: ds.is_active ? "#065f46" : "#6b7280" }}>
                        {ds.is_active ? "Active" : "Inactive"}
                      </span>
                    </td>
                    <td className="muted" style={{ fontSize: 13 }}>
                      {tableCounts[ds.id] !== undefined ? (
                        tableCounts[ds.id] !== null ? <span style={{ fontWeight: 600, color: "#059669" }}>{tableCounts[ds.id]} tables</span> : <span style={{ color: "#9ca3af" }}>—</span>
                      ) : <i className="fas fa-spinner fa-spin" style={{ fontSize: 11, color: "var(--loomx-primary)" }} />}
                    </td>
                    <td className="actions-cell">
                      <div className="row-actions">
                        <button type="button" className="action-icon-btn" title={ds.is_favorite === 1 ? "Remove favorite" : "Set as favorite"} onClick={e => toggleFavorite(ds, e)} style={{ color: ds.is_favorite === 1 ? "#f59e0b" : undefined }}>
                          <i className={ds.is_favorite === 1 ? "fas fa-star" : "far fa-star"} />
                        </button>
                        <button type="button" className="action-icon-btn" title={ds.is_active ? "Deactivate" : "Activate"} onClick={e => toggleDataSource(ds, e)}>
                          <i className={`fas fa-${ds.is_active ? "pause" : "play"}-circle`} />
                        </button>
                        <button type="button" className="action-icon-btn" title="Copy data source" onClick={e => { e.stopPropagation(); setCopyingDataSource(ds); }}>
                          <i className="fas fa-copy" />
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
      </ListPageShell>

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
  const [formData, setFormData] = useState({
    name: isCopying ? `${dataSource?.name} (Copy)` : (dataSource?.name || ""),
    type: dataSource?.type || "Fabric SQL AEP",
    connection_string: dataSource?.connection_string || "",
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

      const response = await msalFetch(url, {
        method: isEditing ? "PATCH" : "POST",
        headers: {
          "Content-Type": "application/json",
          "x-user-email": account?.email || account?.username || "",
        },
        body: JSON.stringify(formData),
      });

      if (!response.ok) {
        const errorData = await response.json();
        throw new Error(errorData.message || `Failed to ${isEditing ? 'update' : 'create'} data source`);
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
          background: 'rgba(0, 0, 0, 0.5)',
          zIndex: 1000,
        }}
        onClick={onClose}
      />
      <div
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          background: 'white',
          borderRadius: '12px',
          boxShadow: '0 20px 60px rgba(0, 0, 0, 0.3)',
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
            borderBottom: '1px solid #e5e7eb',
          }}
        >
          <h2 style={{ margin: 0, fontSize: '1.5rem', fontWeight: 600 }}>
            {isEditing ? 'Edit Data Source' : isCopying ? 'Copy Data Source' : 'Add Data Source'}
          </h2>
          <button
            onClick={onClose}
            style={{
              background: 'none',
              border: 'none',
              fontSize: '1.5rem',
              cursor: 'pointer',
              color: '#6b7280',
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

        <form onSubmit={handleSubmit}>
          <div style={{ padding: '1.5rem', overflowY: 'auto', flex: 1 }}>
            {error && (
              <div
                style={{
                  padding: '1rem',
                  borderRadius: '8px',
                  marginBottom: '1.5rem',
                  background: '#fef2f2',
                  color: '#991b1b',
                  border: '1px solid #fecaca',
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
              <select
                className="chart-builder-select"
                value={formData.type}
                onChange={(e) => setFormData({ ...formData, type: e.target.value })}
                required
              >
                <option value="Fabric SQL AEP">Fabric SQL AEP</option>
                <option value="Fabric SQL DW">Fabric SQL DW</option>
                <option value="Azure SQL">Azure SQL</option>
                <option value="PostgreSQL">PostgreSQL</option>
              </select>
            </div>

            <div className="chart-builder-field">
              <label className="chart-builder-label">
                <span>SQL Endpoint *</span>
              </label>
              <input
                type="text"
                className="chart-builder-input"
                style={{ fontFamily: 'monospace' }}
                placeholder="e.g., your-workspace.datawarehouse.fabric.microsoft.com"
                value={formData.connection_string}
                onChange={(e) => setFormData({ ...formData, connection_string: e.target.value })}
                required
              />
            </div>

            <div className="chart-builder-field">
              <label className="chart-builder-label">
                <span>Database Name {formData.type.includes('Fabric SQL') ? '*' : ''}</span>
              </label>
              <input
                type="text"
                className="chart-builder-input"
                style={{ fontFamily: 'monospace' }}
                placeholder="e.g., IDEASServingStoreLH"
                value={formData.database_name}
                onChange={(e) => setFormData({ ...formData, database_name: e.target.value })}
                required={formData.type.includes('Fabric SQL')}
              />
            </div>

            <div className="chart-builder-field">
              <label className="chart-builder-label">
                <span>Region *</span>
              </label>
              <div style={{ display: 'flex', gap: '1.5rem' }}>
                <label style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', cursor: 'pointer', fontSize: '0.85rem' }}>
                  <input
                    type="radio"
                    name="region"
                    value="WW"
                    checked={formData.region === "WW"}
                    onChange={(e) => setFormData({ ...formData, region: e.target.value as "WW" | "EU" })}
                    style={{ cursor: 'pointer' }}
                  />
                  <span>Worldwide (WW)</span>
                </label>
                <label style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', cursor: 'pointer', fontSize: '0.85rem' }}>
                  <input
                    type="radio"
                    name="region"
                    value="EU"
                    checked={formData.region === "EU"}
                    onChange={(e) => setFormData({ ...formData, region: e.target.value as "WW" | "EU" })}
                    style={{ cursor: 'pointer' }}
                  />
                  <span>Europe (EU)</span>
                </label>
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
              borderTop: '1px solid #e5e7eb',
              background: '#f9fafb',
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
