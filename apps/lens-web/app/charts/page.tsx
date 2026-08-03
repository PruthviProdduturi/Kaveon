"use client";

import { useEffect, useMemo, useState } from "react";
import { API_BASE } from "../../config";
import { TEMPLATES } from "../../components/charts/ChartBuilderContext";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useRouter } from "next/navigation";
import { Button } from "../../components/Button";
import { ListPageShell } from "../../components/ListPageShell";
import { Pagination } from "../../components/Pagination";

interface ChartSummary {
  id: number;
  name: string;
  chart_type: string;
  description?: string | null;
  owner: string | null;
  created_by?: string | null;
  modified_by?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  favorite?: boolean;
}

const PAGE_SIZE = 20;

export default function ChartsPage() {
  const { isAuthenticated, account } = useAuth();
  const router = useRouter();
  const [charts, setCharts] = useState<ChartSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<number | null>(null);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);

  const handleToggleFavorite = async (chart: ChartSummary, e: React.MouseEvent) => {
    e.stopPropagation();
    if (!isAuthenticated) return;
    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/charts/${chart.id}/favorite?is_favorite=${!chart.favorite}`, {
        method: "PUT",
        headers: userEmail ? { "x-user-email": userEmail } : undefined,
      });
      if (!res.ok) throw new Error();
      setCharts(prev => prev.map(c => c.id === chart.id ? { ...c, favorite: !c.favorite } : c));
    } catch {}
  };

  useEffect(() => {
    if (!isAuthenticated) return;
    const load = async () => {
      setLoading(true);
      try {
        const userEmail = account?.email || account?.username || null;
        const res = await msalFetch(`${API_BASE}/api/v1/charts/summary`, {
          headers: userEmail ? { "x-user-email": userEmail } : undefined,
        });
        if (!res.ok) throw new Error(`Failed to load charts: ${res.status}`);
        const data = await res.json();
        setCharts((data.recent || []).map((c: ChartSummary) => ({ ...c, favorite: c.favorite ?? false, owner: c.owner || "—", modified_by: c.modified_by || "—" })));
      } catch (e) {
        setError(e instanceof Error ? e.message : "Unknown error");
      } finally {
        setLoading(false);
      }
    };
    load();
  }, [isAuthenticated]);

  const chartsWithMeta = useMemo(() =>
    charts.map(c => {
      const tpl = TEMPLATES.find(t => t.id === c.chart_type);
      return { ...c, friendlyType: tpl?.name ?? (c.chart_type?.replace(/_/g, " ") ?? "Unknown"), category: tpl?.category ?? "" };
    }), [charts]);

  const formatDateTime = (value?: string | null) => {
    if (!value) return "—";
    try { return new Date(value).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }); }
    catch { return value; }
  };

  const handleDeleteChart = async (chart: ChartSummary) => {
    if (!window.confirm(`Delete chart "${chart.name}"? This cannot be undone.`)) return;
    setDeletingId(chart.id);
    try {
      const userEmail = account?.email || account?.username || null;
      const res = await msalFetch(`${API_BASE}/api/v1/charts/${chart.id}`, {
        method: "DELETE",
        headers: userEmail ? { "x-user-email": userEmail } : undefined,
      });
      if (!res.ok && res.status !== 204) throw new Error(`Failed (${res.status})`);
      setCharts(prev => prev.filter(c => c.id !== chart.id));
    } catch (err) {
      console.error("Failed to delete chart", err);
    } finally {
      setDeletingId(null);
    }
  };

  const filtered = chartsWithMeta.filter(c =>
    !search || c.name.toLowerCase().includes(search.toLowerCase()) ||
    c.friendlyType.toLowerCase().includes(search.toLowerCase()) ||
    (c.description ?? "").toLowerCase().includes(search.toLowerCase())
  );

  const handleSearch = (q: string) => { setSearch(q); setPage(1); };
  const paged = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const userEmail = account?.email || account?.username || null;
  const myCount = userEmail ? charts.filter(c => c.created_by === userEmail || c.owner === userEmail).length : 0;
  const othersCount = charts.length - myCount;

  return (
    <ListPageShell
      icon="fa-chart-bar"
      title="Charts"
      subtitle="Browse and manage saved charts built on top of your datasets and Lab queries."
      pills={!loading && !error ? [
        { label: `${charts.length} Total`, icon: "fa-chart-bar" },
        ...(myCount > 0 ? [{ label: `${myCount} Mine`, icon: "fa-user" }] : []),
        ...(othersCount > 0 ? [{ label: `${othersCount} Others`, icon: "fa-users", bg: "#f1f5f9", border: "#e2e8f0", color: "#64748b" }] : []),
      ] : []}
      action={
        <Button onClick={() => void router.push("/charts/new")}>
          <i className="fas fa-plus" /> New chart
        </Button>
      }
      loading={loading}
      loadingMessage="Loading charts"
      error={error}
      empty={!loading && !error && charts.length === 0}
      emptyTitle="Create your first chart"
      emptyBody="Charts turn a dataset or Lab query into a visual — KPIs, time-series, maps, tables and more. Here's the path."
      emptySteps={[
        { icon: "fa-database", title: "Pick a dataset", desc: "Choose a table, view or saved query as the chart's source." },
        { icon: "fa-sliders-h", title: "Shape the data", desc: "Add metrics, group-bys, filters and a visualization type." },
        { icon: "fa-floppy-disk", title: "Save & reuse", desc: "Save the chart to drop it onto any dashboard." },
      ]}
      emptyAction={<Button onClick={() => void router.push("/charts/new")}><i className="fas fa-plus" /> New chart</Button>}
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
                <th><span className="column-header-label">Owner</span></th>
                <th><span className="column-header-label">Modified by</span></th>
                <th><span className="column-header-label">Last modified</span></th>
                <th><span className="column-header-label">Actions</span></th>
              </tr>
            </thead>
            <tbody>
              {paged.map(c => (
                <tr key={c.id} onClick={() => void router.push(`/charts/${c.id}`)} style={{ cursor: "pointer" }}>
                  <td>
                    <strong>{c.name}</strong>
                    {c.description && <div className="muted" style={{ marginTop: 4, fontSize: 13 }}>{c.description}</div>}
                  </td>
                  <td className="muted" style={{ fontSize: 13 }}>{c.friendlyType}</td>
                  <td className="muted" style={{ fontSize: 13 }}>{c.owner || "—"}</td>
                  <td className="muted" style={{ fontSize: 13 }}>{c.modified_by || "—"}</td>
                  <td className="muted" style={{ fontSize: 13 }}>{formatDateTime(c.updated_at || c.created_at)}</td>
                  <td className="actions-cell">
                    <div className="row-actions">
                      <button type="button" className="action-icon-btn" title={c.favorite ? "Unfavorite" : "Favorite"}
                        onClick={e => handleToggleFavorite(c, e)} style={{ color: c.favorite ? "#f5c518" : undefined }}>
                        <i className={c.favorite ? "fas fa-star" : "far fa-star"} />
                      </button>
                      <button type="button" className="action-icon-btn" title="Edit chart"
                        onClick={e => { e.stopPropagation(); void router.push(`/charts/${c.id}`); }}>
                        <i className="fas fa-edit" />
                      </button>
                      <button type="button" className="action-icon-btn" title="Delete chart"
                        onClick={e => { e.stopPropagation(); void handleDeleteChart(c); }} disabled={deletingId === c.id}>
                        <i className={deletingId === c.id ? "fas fa-spinner fa-spin" : "fas fa-trash"} />
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
  );
}
