"use client";

import { formatDatasetDate, formatSavedQueryDate } from "../utils/date";
import { useEffect, useState } from "react";

import { API_BASE } from "../config";
import { LoadingOverlay } from "../components/LoadingOverlay";
import { msalFetch } from "../utils/msalFetch";
import { useAuth } from "../auth/useAuth";
import { useTheme } from "../contexts/ThemeContext";

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
  const { primaryColor } = useTheme();

  const [error, setError] = useState<string | null>(null);
  const [dashboards, setDashboards] = useState<DashboardSummary[]>([]);
  const [charts, setCharts] = useState<ChartSummary[]>([]);
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [dashboardCount, setDashboardCount] = useState<number | null>(null);
  const [chartCount, setChartCount] = useState<number | null>(null);
  const [datasetCount, setDatasetCount] = useState<number | null>(null);
  const [savedQueries, setSavedQueries] = useState<SavedQuerySummary[]>([]);
  const [allSavedQueries, setAllSavedQueries] = useState<SavedQuerySummary[]>([]);
  const [savedQueriesCount, setSavedQueriesCount] = useState<number | null>(null);
  const [labTables, setLabTables] = useState<LabTableSummary[]>([]);
  const [labTableCount, setLabTableCount] = useState<number | null>(null);
  const [allLabTables, setAllLabTables] = useState<LabTableSummary[]>([]);
  const [allLabTableCount, setAllLabTableCount] = useState<number | null>(null);
  const [allTableCountFetched, setAllTableCountFetched] = useState(false);
  const [dataSourceList, setDataSourceList] = useState<any[]>([]);
  const [allFavorites, setAllFavorites] = useState<any[]>([]);
  const [tableScope, setTableScope] = useState<"fav" | "all">("fav");
  const [dashboardScope, setDashboardScope] = useState<"mine" | "all">("all");
  const [chartScope, setChartScope] = useState<"mine" | "all">("all");
  const [datasetScope, setDatasetScope] = useState<"mine" | "all">("all");
  const [queryScope, setQueryScope] = useState<"mine" | "all">("mine");
  // Workspace Activity toggle state
  const [activityScope, setActivityScope] = useState<'mine'|'all'>('all');
  const [isLoadingData, setIsLoadingData] = useState(false);

  useEffect(() => {
    if (!isAuthenticated) return;

    const userEmail = account?.email || account?.username || null;
    const hdrs = userEmail ? { 'x-user-email': userEmail } : undefined;

    setIsLoadingData(true);

    // ── Fire ALL requests at t=0, as soon as the user is authenticated ────────
    //
    // Nothing waits for anything else. Metadata DB calls and warehouse calls
    // start simultaneously. On a cold pool they all take ~10s but run in
    // PARALLEL — not sequential — so the page is fully loaded in ~10s instead
    // of ~20s (old sequential approach).
    //
    // State updates happen independently as each call resolves; cards show
    // "•••" (null) until their data arrives and then update in-place.

    // 1. Favorite data source (metadata DB)
    //    Resolved first so lab/tables can chain off it immediately.
    const favPromise = msalFetch(`${API_BASE}/api/v1/data-sources/favorite/current`, { headers: hdrs })
      .then(r => r.ok ? r.json() : null)
      .catch(() => null);

    // 2. All LoomX metadata in one call (metadata DB)
    const summaryPromise = msalFetch(`${API_BASE}/api/v1/metadata/summary`, { headers: hdrs })
      .then(r => r.ok ? r.json() : { dashboards: [], charts: [], datasets: [], favorites: [], savedQueries: [] })
      .catch(() => ({ dashboards: [], charts: [], datasets: [], favorites: [], savedQueries: [] }));

    // 3. Data source list without table counts (metadata DB)
    const listPromise = msalFetch(`${API_BASE}/api/v1/data-sources/list`, { headers: hdrs })
      .then(r => r.ok ? r.json() : { dataSources: [] })
      .catch(() => ({ dataSources: [] }));

    // 4. Lab tables — chains off favPromise so it fires the instant we know
    //    the favorite DB name, without waiting for any other call to complete.
    const tablesPromise = favPromise.then((favData: any) => {
      const favDb = favData?.dataSource?.database_name as string | undefined;
      if (!favDb) return { tables: [] };
      return msalFetch(
        `${API_BASE}/api/v1/lab/tables?database=${encodeURIComponent(favDb)}`,
        { headers: hdrs }
      )
        .then(r => r.ok ? r.json() : { tables: [] })
        .catch(() => ({ tables: [] }));
    });

    // ── Update state as each call resolves ────────────────────────────────────

    summaryPromise.then((metadata: any) => {
      const allQueries = metadata.savedQueries || [];
      const filteredQueries = userEmail
        ? allQueries.filter((q: any) => !q.created_by || q.created_by === userEmail)
        : allQueries;

      const rawFavorites = (metadata.favorites || [])
        .map((fav: any) => ({
          ...fav,
          href:
            fav.kind === "dashboard" ? `/dashboards/${fav.id}/view`
            : fav.kind === "chart"   ? `/charts`
            : fav.kind === "dataset" ? `/datasets/${fav.id}`
            : "#",
        }))
        .filter((f: any) => f.updated_at || f.created_at);

      const uniqueFavoritesMap = new Map<string, any>();
      rawFavorites.forEach((fav: any) => {
        const key = `${fav.kind}-${fav.id}`;
        if (!uniqueFavoritesMap.has(key)) uniqueFavoritesMap.set(key, fav);
      });

      const processedFavorites = Array.from(uniqueFavoritesMap.values())
        .sort((a: any, b: any) =>
          new Date(b.updated_at || b.created_at || 0).getTime() -
          new Date(a.updated_at || a.created_at || 0).getTime()
        )
        .slice(0, 6);

      setDashboards(metadata.dashboards || []);
      setCharts(metadata.charts || []);
      setDatasets(metadata.datasets || []);
      setDashboardCount((metadata.dashboards || []).length);
      setChartCount((metadata.charts || []).length);
      setDatasetCount((metadata.datasets || []).length);
      setAllFavorites(processedFavorites);
      setAllSavedQueries(allQueries);
      setSavedQueries(filteredQueries);
      setSavedQueriesCount(allQueries.length);
      setIsLoadingData(false);
    }).catch(() => {
      setDashboardCount(0);
      setChartCount(0);
      setDatasetCount(0);
      setSavedQueriesCount(0);
      setIsLoadingData(false);
    });

    listPromise.then((data: any) => {
      setDataSourceList(data.dataSources || []);
    });

    tablesPromise.then((data: any) => {
      let tables: LabTableSummary[] = [];
      if (Array.isArray(data)) tables = data;
      else if (Array.isArray(data?.tables)) tables = data.tables;
      setLabTables(tables);
      setLabTableCount(tables.length);
    }).catch(() => setLabTableCount(0));

    // Background prefetch: databases list for the Labs page
    setTimeout(() => {
      msalFetch(`${API_BASE}/api/v1/lab/databases`, { headers: hdrs }).catch(() => {});
    }, 100);
  }, [isAuthenticated, account]);

  // Lazy-load total table count across ALL data sources.
  // Only fires when the user explicitly switches to "all" scope — avoids a
  // 17s warehouse round-trip on initial page load (warehouse cold start +
  // per-source getTables calls). The result is cached in allTableCountFetched
  // so the call is made at most once per session.
  useEffect(() => {
    if (!isAuthenticated || tableScope !== "all" || allTableCountFetched) return;

    const userEmail = account?.email || account?.username || null;
    const hdrs = userEmail ? { 'x-user-email': userEmail } : undefined;

    setAllTableCountFetched(true); // prevent duplicate fetches

    msalFetch(`${API_BASE}/api/v1/data-sources/active`, { headers: hdrs })
      .then(r => r.ok ? r.json() : { dataSources: [] })
      .then((data: any) => {
        const sources: any[] = data.dataSources || [];
        const total = sources.reduce((s: number, ds: any) => s + (ds.table_count || 0), 0);
        setAllLabTableCount(total);
      })
      .catch(() => setAllLabTableCount(0));
  }, [isAuthenticated, account, tableScope, allTableCountFetched]);

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
  // When null, show loading state; when number, show actual count
  const dashboardCountDisplay = dashboardScope === 'all' ? dashboardCount : dashboardsForScope.length;
  const chartCountDisplay = chartScope === 'all' ? chartCount : chartsForScope.length;
  const datasetCountDisplay = datasetScope === 'all' ? datasetCount : datasetsForScope.length;

  // For saved queries: use null for loading state, or calculate from scope
  const mySavedQueriesCount = queriesForScope.length;
  const displayedQueryCount = queryScope === "mine"
    ? (savedQueriesCount === null ? null : mySavedQueriesCount)
    : savedQueriesCount;

  // For data tables: show count based on fav/all toggle
  const displayedTableCount = tableScope === "fav" ? labTableCount : allLabTableCount;

  const recentQueriesCount = allSavedQueries.filter((q) => {
    const raw = pickDateValue(q);
    if (!raw) return false;
    const dt = new Date(raw);
    if (Number.isNaN(dt.getTime())) return false;
    const now = Date.now();
    const sevenDaysMs = 7 * 24 * 60 * 60 * 1000;
    return now - dt.getTime() <= sevenDaysMs;
  }).length;

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

  return (
    <>
      {isAuthenticated && (
        <>
          {isLoadingData && (
            <LoadingOverlay />
          )}
          {!isLoadingData && (
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
              <div className="stat-number">
                {dashboardCountDisplay === null ? (
                  <span style={{ opacity: 0.4, fontSize: '0.8em' }}>•••</span>
                ) : (
                  dashboardCountDisplay
                )}
              </div>
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
              <div className="stat-number">
                {chartCountDisplay === null ? (
                  <span style={{ opacity: 0.4, fontSize: '0.8em' }}>•••</span>
                ) : (
                  chartCountDisplay
                )}
              </div>
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
              <div className="stat-number">
                {datasetCountDisplay === null ? (
                  <span style={{ opacity: 0.4, fontSize: '0.8em' }}>•••</span>
                ) : (
                  datasetCountDisplay
                )}
              </div>
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
              onClick={(e) => {
                if (!(e.target instanceof HTMLElement && e.target.closest('.saved-queries-toggle button'))) {
                  window.location.href = "/lab/queries";
                }
              }}
              onKeyDown={(e) => {
                if ((e.key === "Enter" || e.key === " ") && !(e.target instanceof HTMLElement && e.target.closest('.saved-queries-toggle button'))) {
                  e.preventDefault();
                  window.location.href = "/lab/queries";
                }
              }}
            >
              <div className="stat-number">
                {displayedQueryCount === null ? (
                  <span style={{ opacity: 0.4, fontSize: '0.8em' }}>•••</span>
                ) : (
                  displayedQueryCount
                )}
              </div>
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
              <div className="stat-number">
                {displayedTableCount === null ? (
                  <span style={{ opacity: 0.4, fontSize: '0.8em' }}>•••</span>
                ) : (
                  displayedTableCount
                )}
              </div>
              <div className="stat-label">
                Data Tables
                <span className="saved-queries-toggle">
                  <button
                    type="button"
                    className={tableScope === "fav" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setTableScope("fav");
                    }}
                  >
                    Fav
                  </button>
                  <button
                    type="button"
                    className={tableScope === "all" ? "active" : ""}
                    onClick={(e) => {
                      e.stopPropagation();
                      setTableScope("all");
                    }}
                  >
                    All
                  </button>
                </span>
              </div>
            </div>
          </div>

          <div className="home-main-grid">
            <div className="home-column">
              <section className="home-panel" style={{ marginBottom: 12, padding: '16px 20px 12px 20px' }}>
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
                        icon = <i className="fas fa-tachometer-alt" style={{marginRight:6, color: primaryColor}}/>;
                        typeLabel = "Dashboard";
                      } else if (fav.kind === "chart") {
                        icon = <i className="fas fa-chart-bar" style={{marginRight:6, color: primaryColor}}/>;
                        typeLabel = "Chart";
                      } else if (fav.kind === "dataset") {
                        icon = <i className="fas fa-database" style={{marginRight:6, color: primaryColor}}/>;
                        typeLabel = "Dataset";
                      } else if (fav.kind === "saved_query") {
                        icon = <i className="fas fa-flask" style={{marginRight:6, color: primaryColor}}/>;
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
                  <span className="saved-queries-toggle">
                    <button
                      type="button"
                      className={activityScope === 'mine' ? 'active' : ''}
                      onClick={() => setActivityScope('mine')}
                    >
                      Mine
                    </button>
                    <button
                      type="button"
                      className={activityScope === 'all' ? 'active' : ''}
                      onClick={() => setActivityScope('all')}
                    >
                      All
                    </button>
                  </span>
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
                          icon = <i className="fas fa-tachometer-alt" style={{marginRight:6, color: primaryColor}}/>;
                        } else if (item.kind === "chart") {
                          href = `/charts`;
                          typeLabel = "Chart";
                          formatted = formatDatasetDate(item.date);
                          icon = <i className="fas fa-chart-bar" style={{marginRight:6, color: primaryColor}}/>;
                        } else if (item.kind === "dataset") {
                          href = `/datasets/${item.id}`;
                          typeLabel = "Dataset";
                          formatted = formatDatasetDate(item.date);
                          icon = <i className="fas fa-database" style={{marginRight:6, color: primaryColor}}/>;
                        } else if (item.kind === "saved_query") {
                          href = `/lab?savedQueryId=${item.id}`;
                          typeLabel = "Saved query";
                          formatted = formatSavedQueryDate(item.date);
                          icon = <i className="fas fa-flask" style={{marginRight:6, color: primaryColor}}/>;
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
      )}
    </>
  );
}
