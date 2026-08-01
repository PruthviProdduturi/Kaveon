import { formatDatasetDate, formatSavedQueryDate } from "../utils/date";
import { useEffect, useState } from "react";

import { API_BASE } from "../config";
import { LoadingOverlay } from "../components/LoadingOverlay";
import { msalFetch } from "../utils/msalFetch";
import { useAuth } from "../auth/useAuth";

// using same-origin relative API calls



interface DashboardSummary {
  id: number;
  name: string;
  created_by?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  changed_on?: string | null;
  favorite?: boolean;
}

interface ChartSummary {
  id: number;
  name: string;
  chart_type?: string;
  created_by?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  changed_on?: string | null;
  favorite?: boolean;
}

interface DatasetSummary {
  id: number;
  name: string;
  description?: string;
  created_by?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  favorite?: boolean;
}

interface SavedQuerySummary {
  id: number;
  name?: string;
  tab_name?: string;
  query_name?: string;
  title?: string;
  created_by?: string | null;
  updated_at?: string;
  created_at?: string;
  saved_at?: string;
  inserted_at?: string;
  created_on?: string;
  updated_on?: string;
  [key: string]: unknown;
}

interface LabTableSummary {
  schema: string;
  name: string;
  display_name?: string;
}
function pickDateValue(query: SavedQuerySummary): string | null {
  const dateFields = [
    "updated_at",
    "created_at",
    "saved_at",
    "inserted_at",
    "created_on",
    "updated_on",
  ] as const;
  for (const key of dateFields) {
    const value = query[key];
    if (value) {
      return String(value);
    }
  }
  return null;
}


function pickDisplayName(query: SavedQuerySummary): string {
  const nameFallbacks = ["tab_name", "name", "query_name", "title"] as const;
  for (const key of nameFallbacks) {
    const value = query[key];
    if (value && String(value).trim()) {
      return String(value).trim();
    }
  }
  if (query.id) {
    return `Query ${query.id}`;
  }
  return "Saved query";
}


function getTimeOfDayGreeting(): string {
  const hour = new Date().getHours();
  if (hour < 12) return "Good morning";
  if (hour < 17) return "Good afternoon";
  return "Good evening";
}

export default function Home() {
  const { isAuthenticated, account } = useAuth();

  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [dashboards, setDashboards] = useState<DashboardSummary[]>([]);
  const [charts, setCharts] = useState<ChartSummary[]>([]);
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [dashboardCount, setDashboardCount] = useState<number>(0);
  const [chartCount, setChartCount] = useState<number>(0);
  const [datasetCount, setDatasetCount] = useState<number>(0);
  const [savedQueries, setSavedQueries] = useState<SavedQuerySummary[]>([]);
  const [allSavedQueries, setAllSavedQueries] = useState<SavedQuerySummary[]>([]);
  const [labTables, setLabTables] = useState<LabTableSummary[]>([]);
  const [dashboardScope, setDashboardScope] = useState<"mine" | "all">("all");
  const [chartScope, setChartScope] = useState<"mine" | "all">("all");
  const [datasetScope, setDatasetScope] = useState<"mine" | "all">("all");
  const [queryScope, setQueryScope] = useState<"mine" | "all">("mine");
  // Workspace Activity toggle state
  const [activityScope, setActivityScope] = useState<'mine'|'all'>('all');

  useEffect(() => {
    if (!isAuthenticated) return;
    setLoading(true);
    // Fetch all essential homepage data in parallel
    Promise.all([
      msalFetch(`${API_BASE}/api/v1/metadata/summary`).then(r => r.ok ? r.json() : Promise.reject("Failed to load metadata")),
      msalFetch(`${API_BASE}/api/v1/favorites`).then(r => r.ok ? r.json() : []),
      msalFetch(`${API_BASE}/api/v1/lab/saved-queries`).then(r => r.ok ? r.json() : []),
      msalFetch(`${API_BASE}/api/v1/lab/tables`).then(r => r.ok ? r.json() : { tables: [] })
    ])
      .then(([metadata, favorites, savedQueriesData, labTablesData]) => {
        setDashboards(metadata.dashboards || []);
        setCharts(metadata.charts || []);
        setDatasets(metadata.datasets || []);
        setDashboardCount((metadata.dashboards || []).length);
        setChartCount((metadata.charts || []).length);
        setDatasetCount((metadata.datasets || []).length);
        setAllFavorites((favorites || []).map((fav: any) => ({
          ...fav,
          href:
            fav.kind === "dashboard"
              ? `/dashboards/${fav.id}/view`
              : fav.kind === "chart"
              ? `/charts`
              : fav.kind === "dataset"
              ? `/datasets/${fav.id}`
              : "#"
        })).filter((f: any) => f.updated_at || f.created_at)
          .sort((a: any, b: any) => {
            const aDate = new Date(a.updated_at || a.created_at || 0).getTime();
            const bDate = new Date(b.updated_at || b.created_at || 0).getTime();
            return bDate - aDate;
          })
          .slice(0, 5));
        // Saved queries
        const userEmail = account?.email || account?.username || null;
        const all = savedQueriesData || [];
        const filtered = userEmail
          ? all.filter((q: any) => !q.created_by || q.created_by === userEmail)
          : all;
        setAllSavedQueries(all);
        setSavedQueries(filtered);
        // Lab tables
        if (labTablesData && labTablesData.success && Array.isArray(labTablesData.tables)) {
          setLabTables(labTablesData.tables);
        } else {
          setLabTables([]);
        }
        setLoading(false);
      })
      .catch((err) => {
        setError("Failed to load homepage data");
        setLoading(false);
      });
  }, [isAuthenticated, account]);

  // ...existing code...

  const userEmail = account?.email || account?.username || null;

  const myDashboards = userEmail
    ? dashboards.filter((d) => d.created_by === userEmail)
    : dashboards;
  const myCharts = userEmail
    ? charts.filter((c) => c.created_by === userEmail)
    : charts;
  const myDatasets = userEmail
    ? datasets.filter((d) => d.created_by === userEmail)
    : datasets;

  const dashboardsForScope = dashboardScope === "mine" ? myDashboards : dashboards;
  const chartsForScope = chartScope === "mine" ? myCharts : charts;
  const datasetsForScope = datasetScope === "mine" ? myDatasets : datasets;
  const queriesForScope = queryScope === "mine" ? savedQueries : allSavedQueries;

  // Use summary counts for 'All' scope
  const dashboardCountDisplay = dashboardScope === 'all' ? dashboardCount : dashboardsForScope.length;
  const chartCountDisplay = chartScope === 'all' ? chartCount : chartsForScope.length;
  const datasetCountDisplay = datasetScope === 'all' ? datasetCount : datasetsForScope.length;
  const savedQueriesCount = queriesForScope.length;
  const allSavedQueriesCount = allSavedQueries.length || savedQueries.length;

  const labTableCount = labTables.length;

  const recentQueriesCount = allSavedQueries.filter((q) => {
    const raw = pickDateValue(q);
    if (!raw) return false;
    const dt = new Date(raw);
    if (Number.isNaN(dt.getTime())) return false;
    const now = Date.now();
    const sevenDaysMs = 7 * 24 * 60 * 60 * 1000;
    return now - dt.getTime() <= sevenDaysMs;
  }).length;

  const displayedQueryCount = queryScope === "mine" ? savedQueriesCount : allSavedQueriesCount;

  const firstName = account?.name ? account.name.split(" ")[0] : null;
  const greeting = getTimeOfDayGreeting();

  const recentSavedQueries = [...allSavedQueries]
    .map((query) => ({ query, date: pickDateValue(query) }))
    .filter((x) => x.date)
    .sort((a, b) => {
      const aTime = new Date(a.date as string).getTime();
      const bTime = new Date(b.date as string).getTime();
      return bTime - aTime;
    })
    .slice(0, 4)
    .map((x) => x.query);

  // Gather all user favorites from /api/v1/favorites
  const [allFavorites, setAllFavorites] = useState<any[]>([]);
  useEffect(() => {
    if (!isAuthenticated) return;
    msalFetch(`${API_BASE}/api/v1/favorites`)
      .then(r => r.ok ? r.json() : Promise.reject("Failed to load favorites"))
      .then((favs: any[]) => {
        // Add href for each favorite type
        const withHref = (favs || []).map(fav => ({
          ...fav,
          href:
            fav.kind === "dashboard"
              ? `/dashboards/${fav.id}/view`
              : fav.kind === "chart"
              ? `/charts` // or `/charts/${fav.id}` if you have detail pages
              : fav.kind === "dataset"
              ? `/datasets/${fav.id}`
              : "#"
        }));
        // Sort by updated_at/created_at, show up to 5
        setAllFavorites(
          withHref
            .filter(f => f.updated_at || f.created_at)
            .sort((a, b) => {
              const aDate = new Date(a.updated_at || a.created_at || 0).getTime();
              const bDate = new Date(b.updated_at || b.created_at || 0).getTime();
              return bDate - aDate;
            })
            .slice(0, 5)
        );
      })
      .catch(() => setAllFavorites([]));
  }, [isAuthenticated]);

  const recentDatasets = [...datasets]
    .map((dataset) => ({
      dataset,
      date: dataset.updated_at || dataset.created_at || null,
    }))
    .filter((x) => x.date)
    .sort((a, b) => {
      const aTime = new Date(a.date as string).getTime();
      const bTime = new Date(b.date as string).getTime();
      return bTime - aTime;
    })
    .slice(0, 4)
    .map((x) => x.dataset);

  const hasWorkspaceActivity = recentDatasets.length > 0 || recentSavedQueries.length > 0;

  type ActivityKind = "dataset" | "chart" | "dashboard" | "saved_query";

  interface RecentActivityItem {
    kind: ActivityKind;
    id: number;
    title: string;
    date: string;
  }


  // User-specific activity items for 'Your Recent Activity'
  const userActivityItems: RecentActivityItem[] = [];
  myDatasets.forEach((d) => {
    const date = d.updated_at || d.created_at;
    if (!date) return;
    userActivityItems.push({
      kind: "dataset",
      id: d.id,
      title: (d as any).dataset_name || d.name,
      date,
    });
  });
  myCharts.forEach((c) => {
    const anyChart = c as unknown as { [key: string]: unknown };
    const date =
      (anyChart.updated_at as string | undefined) ||
      (anyChart.changed_on as string | undefined) ||
      (anyChart.created_at as string | undefined) ||
      null;
    if (!date) return;
    userActivityItems.push({
      kind: "chart",
      id: c.id,
      title: c.name,
      date,
    });
  });
  myDashboards.forEach((d) => {
    const anyDash = d as unknown as { [key: string]: unknown };
    const date =
      (anyDash.updated_at as string | undefined) ||
      (anyDash.changed_on as string | undefined) ||
      (anyDash.created_at as string | undefined) ||
      null;
    // Only include if created_by matches userEmail (for 'mine')
    if (!date) return;
    if (!userEmail || d.created_by === userEmail) {
      userActivityItems.push({
        kind: "dashboard",
        id: d.id,
        title: d.name,
        date,
      });
    }
  });
  savedQueries.forEach((q) => {
    const date = pickDateValue(q);
    if (!date) return;
    userActivityItems.push({
      kind: "saved_query",
      id: q.id,
      title: pickDisplayName(q),
      date,
    });
  });

  const recentUserActivityItems = userActivityItems

    .filter((item) => {
      const ts = new Date(item.date).getTime();
      return !Number.isNaN(ts);
    })
    .sort((a, b) => new Date(b.date).getTime() - new Date(a.date).getTime())
    .slice(0, 8);

  const hasRecentUserActivity = recentUserActivityItems.length > 0;

  if (loading) {
    return <LoadingOverlay />;
  }
  return (
    <>
      {isAuthenticated && (
        <div className="homepage-container">
        <section className="welcome-section">
          <div className="welcome-content">
            <h1>
              {firstName ? `${greeting}, ${firstName}` : greeting}
            </h1>
            <p>Your comprehensive data analytics and visualization platform</p>
            <div className="homepage-search">
              <input
                type="text"
                className="homepage-search-input"
                placeholder="Search datasets, charts, dashboards, queries..."
              />
            </div>
          </div>
        </section>

        <section className="dashboard-section">
          <div className="stats-grid">
            <div
              className="stat-card"
              role="button"
              tabIndex={0}
              onClick={(e) => {
                // Only navigate if the click is not on a toggle button
                if (!(e.target instanceof HTMLElement && e.target.closest('.saved-queries-toggle button'))) {
                  window.location.href = "/dashboards";
                }
              }}
              onKeyDown={(e) => {
                if ((e.key === "Enter" || e.key === " ") && !(e.target instanceof HTMLElement && e.target.closest('.saved-queries-toggle button'))) {
                  e.preventDefault();
                  window.location.href = "/dashboards";
                }
              }}
            >
              <div className="stat-number">{dashboardCountDisplay}</div>
              <div className="stat-label">
                Dashboards
                <span className="saved-queries-toggle">
                  <button
                    type="button"
                    className={dashboardScope === "mine" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setDashboardScope("mine");
                    }}
                  >
                    Mine
                  </button>
                  <button
                    type="button"
                    className={dashboardScope === "all" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setDashboardScope("all");
                    }}
                  >
                    All
                  </button>
                </span>
              </div>
            </div>
            <div
              className="stat-card"
              role="button"
              tabIndex={0}
              onClick={(e) => {
                if (!(e.target instanceof HTMLElement && e.target.closest('.saved-queries-toggle button'))) {
                  window.location.href = "/charts";
                }
              }}
              onKeyDown={(e) => {
                if ((e.key === "Enter" || e.key === " ") && !(e.target instanceof HTMLElement && e.target.closest('.saved-queries-toggle button'))) {
                  e.preventDefault();
                  window.location.href = "/charts";
                }
              }}
            >
              <div className="stat-number">{chartCountDisplay}</div>
              <div className="stat-label">
                Charts
                <span className="saved-queries-toggle">
                  <button
                    type="button"
                    className={chartScope === "mine" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setChartScope("mine");
                    }}
                  >
                    Mine
                  </button>
                  <button
                    type="button"
                    className={chartScope === "all" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setChartScope("all");
                    }}
                  >
                    All
                  </button>
                </span>
              </div>
            </div>
            <div
              className="stat-card"
              role="button"
              tabIndex={0}
              onClick={(e) => {
                if (!(e.target instanceof HTMLElement && e.target.closest('.saved-queries-toggle button'))) {
                  window.location.href = "/datasets";
                }
              }}
              onKeyDown={(e) => {
                if ((e.key === "Enter" || e.key === " ") && !(e.target instanceof HTMLElement && e.target.closest('.saved-queries-toggle button'))) {
                  e.preventDefault();
                  window.location.href = "/datasets";
                }
              }}
            >
              <div className="stat-number">{datasetCountDisplay}</div>
              <div className="stat-label">
                Datasets
                <span className="saved-queries-toggle">
                  <button
                    type="button"
                    className={datasetScope === "mine" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setDatasetScope("mine");
                    }}
                  >
                    Mine
                  </button>
                  <button
                    type="button"
                    className={datasetScope === "all" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setDatasetScope("all");
                    }}
                  >
                    All
                  </button>
                </span>
              </div>
            </div>
            <div
              className="stat-card"
              role="button"
              tabIndex={0}
              onClick={() => {
                window.location.href = "/lab/queries";
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  window.location.href = "/lab/queries";
                }
              }}
            >
              <div className="stat-number">{displayedQueryCount}</div>
              <div className="stat-label">
                Saved Queries
                <span className="saved-queries-toggle">
                  <button
                    type="button"
                    className={queryScope === "mine" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setQueryScope("mine");
                    }}
                  >
                    Mine
                  </button>
                  <button
                    type="button"
                    className={queryScope === "all" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setQueryScope("all");
                    }}
                  >
                    All
                  </button>
                </span>
              </div>
            </div>
            <div
              className="stat-card"
              role="button"
              tabIndex={0}
              onClick={() => {
                window.location.href = "/lab";
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  window.location.href = "/lab";
                }
              }}
            >
              <div className="stat-number">{labTableCount}</div>
              <div className="stat-label">Data Tables</div>
            </div>
          </div>

          <div className="home-main-grid">
            <div className="home-column">
              <section className="home-panel" style={{ marginBottom: 18, padding: '16px 20px 12px 20px' }}>
                <h2 className="home-section-title">Quick Actions</h2>
                <div className="welcome-actions">
                  <button
                    className="welcome-btn welcome-btn-primary"
                    onClick={() => (window.location.href = "/dashboards/new")}
                  >
                    <i className="fas fa-tachometer-alt" />
                    <span className="welcome-btn-label">New Dashboard</span>
                  </button>
                  <button
                    className="welcome-btn welcome-btn-primary"
                    onClick={() => (window.location.href = "/charts/new")}
                  >
                    <i className="fas fa-chart-bar" />
                    <span className="welcome-btn-label">New Chart</span>
                  </button>
                  <button
                    className="welcome-btn welcome-btn-primary"
                    onClick={() => (window.location.href = "/datasets/new")}
                  >
                    <i className="fas fa-database" />
                    <span className="welcome-btn-label">New Dataset</span>
                  </button>
                  <button
                    className="welcome-btn welcome-btn-primary"
                    onClick={() => (window.location.href = "/lab")}
                  >
                    <i className="fas fa-flask" />
                    <span className="welcome-btn-label">SQL Lab</span>
                  </button>
                </div>
              </section>
              <section className="home-panel" style={{ marginBottom: 18, padding: '16px 20px 12px 20px' }}>
                <h2
                  className="home-section-title clickable-section-title"
                  style={{ cursor: 'pointer', display: 'flex', alignItems: 'center', marginBottom: 8 }}
                  tabIndex={0}
                  onClick={() => window.location.href = '/favorites'}
                  onKeyDown={e => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault();
                      window.location.href = '/favorites';
                    }
                  }}
                >
                  Your Favorites
                  <span style={{ marginLeft: 8, fontSize: 16 }} aria-hidden="true">→</span>
                </h2>
                <div className="muted" style={{fontSize:13, marginBottom:16, marginTop:2}}>
                  Shows <b>your</b> favorite dashboards, charts, and datasets.
                </div>
                {allFavorites.length === 0 && (
                  <p className="muted">No favorites yet. Mark dashboards, charts, or datasets as favorites to see them here.</p>
                )}
                {allFavorites.length > 0 && (
                  <ul className="home-list">
                    {allFavorites.map((fav) => {
                      const when = fav.updated_at ? formatDatasetDate(fav.updated_at) : null;
                      let icon = null;
                      let typeLabel = '';
                      if (fav.kind === "dashboard") {
                        icon = <i className="fas fa-tachometer-alt" style={{marginRight:6}}/>;
                        typeLabel = "Dashboard";
                      } else if (fav.kind === "chart") {
                        icon = <i className="fas fa-chart-bar" style={{marginRight:6}}/>;
                        typeLabel = "Chart";
                      } else if (fav.kind === "dataset") {
                        icon = <i className="fas fa-database" style={{marginRight:6}}/>;
                        typeLabel = "Dataset";
                      } else if (fav.kind === "saved_query") {
                        icon = <i className="fas fa-flask" style={{marginRight:6}}/>;
                        typeLabel = "Saved query";
                      }
                      return (
                        <li
                          key={fav.kind + '-' + fav.id}
                          className="home-list-item"
                          style={{ cursor: 'pointer' }}
                          tabIndex={0}
                          onClick={() => window.location.href = fav.href}
                          onKeyDown={e => {
                            if (e.key === 'Enter' || e.key === ' ') {
                              e.preventDefault();
                              window.location.href = fav.href;
                            }
                          }}
                        >
                          <div>
                            <div className="home-list-item-title">{icon}{fav.dataset_name || fav.name}</div>
                            <div className="home-list-item-meta">
                              {typeLabel}
                              {when && (
                                <> · Last updated {when}</>
                              )}
                            </div>
                          </div>
                          <button
                            type="button"
                            className="icon-button"
                            tabIndex={-1}
                            aria-label="Go to Favorite"
                            onClick={e => {
                              e.stopPropagation();
                              window.location.href = fav.href;
                            }}
                          >
                            <i className="fas fa-arrow-right" />
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                )}
              </section>
            </div>

            <div className="home-column">
              <section className="home-panel" style={{ marginBottom: 32, padding: '24px 24px 20px 24px' }}>
                <div style={{display:'flex',alignItems:'center',justifyContent:'space-between'}}>
                  <h2
                    className="home-section-title clickable-section-title"
                    style={{ cursor: 'pointer', display: 'flex', alignItems: 'center', marginBottom: 8 }}
                    tabIndex={0}
                    onClick={() => window.location.href = '/workspace-activity'}
                    onKeyDown={e => {
                      if (e.key === 'Enter' || e.key === ' ') {
                        e.preventDefault();
                        window.location.href = '/workspace-activity';
                      }
                    }}
                  >
                    Workspace Activity
                    <span style={{ marginLeft: 8, fontSize: 16 }} aria-hidden="true">→</span>
                  </h2>
                  <div style={{
                    display: 'inline-flex',
                    alignItems: 'center',
                    background: 'transparent',
                    borderRadius: 6,
                    padding: '0 2px',
                    gap: 0,
                  }}>
                    <button
                      type="button"
                      onClick={() => setActivityScope('mine')}
                      style={{
                        background: activityScope === 'mine' ? '#f1f5fe' : 'transparent',
                        color: activityScope === 'mine' ? '#2563eb' : '#222',
                        fontWeight: 400,
                        border: 'none',
                        outline: 'none',
                        borderRadius: 4,
                        padding: '1px 8px',
                        fontSize: 14,
                        lineHeight: '18px',
                        transition: 'all 0.18s cubic-bezier(.4,0,.2,1)',
                        cursor: 'pointer',
                        marginRight: 1,
                      }}
                    >Mine</button>
                    <button
                      type="button"
                      onClick={() => setActivityScope('all')}
                      style={{
                        background: activityScope === 'all' ? '#f1f5fe' : 'transparent',
                        color: activityScope === 'all' ? '#2563eb' : '#222',
                        fontWeight: 400,
                        border: 'none',
                        outline: 'none',
                        borderRadius: 4,
                        padding: '1px 8px',
                        fontSize: 14,
                        lineHeight: '18px',
                        transition: 'all 0.18s cubic-bezier(.4,0,.2,1)',
                        cursor: 'pointer',
                        marginLeft: 1,
                      }}
                    >All</button>
                  </div>
                </div>
                <div className="muted" style={{fontSize:13, marginBottom:16, marginTop:2}}>
                  Recent dashboards, charts, datasets and saved queries.
                </div>
                {(() => {
                  const userEmail = account?.email || account?.username || null;
                  const filterByScope = (items: any[]) => {
                    if (activityScope === 'all') return items;
                    // For 'mine', show only items where created_by matches user
                    if (!userEmail) return [];
                    return items.filter((item) => item.created_by === userEmail);
                  };
                  const dashboardItems = dashboards.map((d) => ({
                    kind: "dashboard",
                    id: d.id,
                    title: d.name,
                    date: d.updated_at || d.changed_on || d.created_at || "",
                    created_by: d.created_by || null,
                  }));
                  const chartItems = charts.map((c) => ({
                    kind: "chart",
                    id: c.id,
                    title: c.name,
                    date: c.updated_at || c.changed_on || c.created_at || "",
                    created_by: c.created_by || null,
                  }));
                  const datasetItems = datasets.map((d) => ({
                    kind: "dataset",
                    id: d.id,
                    title: (d as any).dataset_name || d.name,
                    date: d.updated_at || d.created_at || "",
                    created_by: d.created_by || null,
                  }));
                  const savedQueryItems = allSavedQueries.map((q) => ({
                    kind: "saved_query",
                    id: q.id,
                    title: pickDisplayName(q),
                    date: pickDateValue(q) || "",
                    created_by: q.created_by || null,
                  }));
                  const all = [...dashboardItems, ...chartItems, ...datasetItems, ...savedQueryItems]
                    .filter((item) => item.date && !Number.isNaN(new Date(item.date).getTime()))
                    .sort((a, b) => new Date(b.date).getTime() - new Date(a.date).getTime());
                  const filtered = filterByScope(all).slice(0, 8);
                  if (filtered.length === 0) {
                    return <p className="muted">No recent changes in this workspace.</p>;
                  }
                  return (
                    <ul className="home-list">
                      {filtered.map((item) => {
                        let href = "#";
                        let typeLabel = "";
                        let formatted = "";
                        let icon = null;
                        if (item.kind === "dashboard") {
                          href = `/dashboards/${item.id}/view`;
                          typeLabel = "Dashboard";
                          formatted = formatDatasetDate(item.date);
                          icon = <i className="fas fa-tachometer-alt" style={{marginRight:6}}/>;
                        } else if (item.kind === "chart") {
                          href = `/charts`;
                          typeLabel = "Chart";
                          formatted = formatDatasetDate(item.date);
                          icon = <i className="fas fa-chart-bar" style={{marginRight:6}}/>;
                        } else if (item.kind === "dataset") {
                          href = `/datasets/${item.id}`;
                          typeLabel = "Dataset";
                          formatted = formatDatasetDate(item.date);
                          icon = <i className="fas fa-database" style={{marginRight:6}}/>;
                        } else if (item.kind === "saved_query") {
                          href = `/lab?savedQueryId=${item.id}`;
                          typeLabel = "Saved query";
                          formatted = formatSavedQueryDate(item.date);
                          icon = <i className="fas fa-flask" style={{marginRight:6}}/>;
                        }
                        return (
                          <li
                            key={`${item.kind}-${item.id}`}
                            className="home-list-item"
                            style={{ cursor: 'pointer' }}
                            tabIndex={0}
                            onClick={() => window.location.href = href}
                            onKeyDown={e => {
                              if (e.key === 'Enter' || e.key === ' ') {
                                e.preventDefault();
                                window.location.href = href;
                              }
                            }}
                          >
                            <div>
                              <div className="home-list-item-title">{icon}{item.title}</div>
                              {formatted && (
                                <div className="home-list-item-meta">
                                  {typeLabel} · Updated {formatted}
                                </div>
                              )}
                            </div>
                            <button
                              type="button"
                              className="icon-button"
                              tabIndex={-1}
                              aria-label={`Go to ${typeLabel}`}
                              onClick={e => {
                                e.stopPropagation();
                                window.location.href = href;
                              }}
                            >
                              <i className="fas fa-arrow-right" />
                            </button>
                          </li>
                        );
                      })}
                    </ul>
                  );
                })()}
              </section>
            </div>
          </div>
        </section>
        </div>
      )}
    </>
  );
}
