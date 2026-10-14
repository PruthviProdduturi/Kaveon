"use client";

import React, { useEffect, useState } from "react";

import { API_BASE } from "../../config";
import { LoadingOverlay } from "../../components/LoadingOverlay";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useSetup } from "../../components/ClientLayout";
import { useTheme } from "../../contexts/ThemeContext";
import { UnknownLine, UnknownPanel, reasonFromStatus, type ReadFailure } from "../../components/Reading";

interface WorkspaceActivityItem {
  id: string | number;
  name: string;
  kind: "dataset" | "chart" | "dashboard" | "saved_query";
  owner?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
}

const WorkspaceActivityPage: React.FC = () => {
  const { isAuthenticated } = useAuth();
  const { isSetupOk } = useSetup();
  const { primaryColor, gradientColors } = useTheme();
  const [activity, setActivity] = useState<WorkspaceActivityItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [answered, setAnswered] = useState(false);
  const [failure, setFailure] = useState<ReadFailure | null>(null);
  // Four independent reads make this page, and each one that failed used to
  // be swallowed into an empty list - so a charts family that was down
  // produced a page listing datasets and dashboards with no sign that a
  // quarter of the answer was missing. Each failure is now named.
  const [partial, setPartial] = useState<ReadFailure[]>([]);

  useEffect(() => {
    if (!isAuthenticated) return;
    // `false` is an answer — this installation reports itself as not set up —
    // and `null` only means the check has not come back. Neither is an empty
    // list, and neither may leave the page waiting for ever.
    if (isSetupOk === false) {
      setFailure({ what: "The system of record", says: "what changed recently", reason: "not_configured" });
      return;
    }
    if (isSetupOk !== true) return;
    setLoading(true);
    setFailure(null);
    const missing: ReadFailure[] = [];
    const read = async (url: string, what: string, says: string, fallback: unknown) => {
      try {
        const r = await msalFetch(url);
        if (r.ok) return await r.json();
        missing.push({ what, says, reason: reasonFromStatus(r.status) });
      } catch {
        missing.push({ what, says });
      }
      return fallback;
    };
    Promise.all([
      read(`${API_BASE}/api/v1/datasets/summary`, "Datasets", "which datasets changed", { recent: [] }),
      read(`${API_BASE}/api/v1/charts/summary`, "Charts", "which charts changed", { recent: [] }),
      read(`${API_BASE}/api/v1/dashboards/summary`, "Dashboards", "which dashboards changed", { recent: [] }),
      read(`${API_BASE}/api/v1/lab/saved-queries`, "Saved queries", "which queries changed", []),
    ]).then(([datasetsRes, chartsRes, dashboardsRes, savedQueries]) => {
      const items: WorkspaceActivityItem[] = [];
      (datasetsRes.recent || []).forEach((d: any) => {
        items.push({
          id: d.id,
          name: d.name,
          kind: "dataset",
          owner: d.created_by || d.owner || d.user_email || d.owner_email || null,
          created_at: d.created_at,
          updated_at: d.updated_at,
        });
      });
      (chartsRes.recent || []).forEach((c: any) => {
        items.push({
          id: c.id,
          name: c.name,
          kind: "chart",
          owner: c.owner || c.created_by || c.user_email || c.owner_email || null,
          created_at: c.created_at,
          updated_at: c.updated_at,
        });
      });
      (dashboardsRes.recent || []).forEach((d: any) => {
        items.push({
          id: d.id,
          name: d.name,
          kind: "dashboard",
          owner: d.owner || d.created_by || d.user_email || d.owner_email || null,
          created_at: d.created_at,
          updated_at: d.updated_at,
        });
      });
      (savedQueries || []).forEach((q: any) => {
        items.push({
          id: q.id,
          name: q.tab_name || q.name || q.query_name || q.title || `Query ${q.id}`,
          kind: "saved_query",
          owner: q.created_by || q.owner || q.user_email || q.owner_email || null,
          created_at: q.created_at || q.saved_at || q.inserted_at || q.created_on,
          updated_at: q.updated_at || q.updated_on,
        });
      });
      // Sort by updated_at desc
      items.sort((a, b) => (b.updated_at || "").localeCompare(a.updated_at || ""));
      setActivity(items);
      setPartial(missing);
      // Every one of the four failed: there is nothing to show and nothing
      // known about this workspace, so the page says that rather than
      // reporting a quiet workspace.
      setAnswered(missing.length < 4);
      if (missing.length === 4) {
        setFailure({ what: "This workspace", says: "what changed recently" });
      }
    }).catch(() => {
      setFailure({ what: "This workspace", says: "what changed recently" });
    }).finally(() => setLoading(false));
  }, [isAuthenticated, isSetupOk]);

  return (
    <div className="page-shell animate-fade-in">
      <div style={{
        display: "flex", alignItems: "center", justifyContent: "space-between",
        flexWrap: "wrap", gap: "1rem",
        marginBottom: "1.25rem",
        padding: "1.25rem 1.5rem",
        background: "var(--bg-surface)",
        borderRadius: 12,
        border: "1px solid var(--border)",
        boxShadow: "var(--shadow-sm)",
      }}>
        <div style={{ display: "flex", alignItems: "center", gap: "1.25rem", flexWrap: "wrap", flex: 1, minWidth: 0 }}>
          <div style={{
            width: 48, height: 48, borderRadius: 12, flexShrink: 0,
            background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
            display: "flex", alignItems: "center", justifyContent: "center",
            boxShadow: `0 4px 14px ${primaryColor}35`,
          }}>
            <i className="fas fa-history" style={{ color: "white", fontSize: 20 }} />
          </div>
          <div style={{ minWidth: 0 }}>
            <h1 style={{ margin: 0, fontSize: "1.15rem", fontWeight: 700, color: "var(--text-primary)" }}>Workspace Activity</h1>
            <p style={{ margin: "3px 0 0", fontSize: "0.85rem", color: "var(--text-muted)", lineHeight: 1.4 }}>
              Recent changes across dashboards, charts, datasets and queries.
            </p>
          </div>
        </div>
      </div>

      {loading && <LoadingOverlay />}
      {!loading && failure && <UnknownPanel {...failure} />}

      {!loading && !failure && partial.length > 0 && (
        <div className="card" style={{ marginBottom: 12, padding: "10px 14px", display: "flex", flexDirection: "column", gap: 6 }}>
          {partial.map((miss, i) => <UnknownLine key={i} {...miss} />)}
        </div>
      )}

      {!loading && !failure && !answered && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Checking</p>
          <p className="page-empty-body">Reading recent changes.</p>
        </div>
      )}

      {!loading && !failure && answered && activity.length === 0 && (
        <div className="card page-empty-card">
          <p className="page-empty-title">No recent changes in this workspace.</p>
        </div>
      )}

      {!loading && activity.length > 0 && (
        <div className="card" style={{ marginTop: 12 }}>
          <div className="results-table-container">
            <table className="results-table">
              <thead>
                <tr>
                  <th><span className="column-header-label">Name</span></th>
                  <th><span className="column-header-label">Type</span></th>
                  <th><span className="column-header-label">Owner</span></th>
                  <th><span className="column-header-label">Last Modified</span></th>
                  <th><span className="column-header-label">Created</span></th>
                </tr>
              </thead>
              <tbody>
                {activity.map((item) => {
                  let href = "#";
                  let typeLabel = "";
                  let lastModified = "—";
                  let created = "—";
                  if (item.kind === "dataset") {
                    href = `/datasets/${item.id}`;
                    typeLabel = "Dataset";
                    lastModified = item.updated_at || "—";
                    created = item.created_at || "—";
                  } else if (item.kind === "chart") {
                    href = `/charts/${item.id}`;
                    typeLabel = "Chart";
                    lastModified = item.updated_at || "—";
                    created = item.created_at || "—";
                  } else if (item.kind === "dashboard") {
                    href = `/dashboards/${item.id}/view`;
                    typeLabel = "Dashboard";
                    lastModified = item.updated_at || "—";
                    created = item.created_at || "—";
                  } else if (item.kind === "saved_query") {
                    href = `/lab?savedQueryId=${item.id}`;
                    typeLabel = "Saved query";
                    lastModified = item.updated_at || "—";
                    created = item.created_at || "—";
                  }
                  return (
                    <tr
                      key={`${item.kind}-${item.id}`}
                      onClick={() => { window.location.href = href; }}
                      style={{ cursor: "pointer" }}
                    >
                      <td>
                        <strong>{item.name}</strong>
                      </td>
                      <td className="muted" style={{ fontSize: 13 }}>{typeLabel}</td>
                      <td className="muted" style={{ fontSize: 13 }}>{item.owner || "—"}</td>
                      <td className="muted" style={{ fontSize: 13 }}>{lastModified}</td>
                      <td className="muted" style={{ fontSize: 13 }}>{created}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
};

export default WorkspaceActivityPage;
