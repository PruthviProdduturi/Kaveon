"use client";

import React, { useCallback } from "react";
import { useRouter } from "next/navigation";
import { DashboardProvider, DashboardConfig } from "../../../components/dashboards/DashboardContext";
import DashboardBuilder from "../../../components/dashboards/DashboardBuilder";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import { API_BASE } from "../../../config";

const DashboardNewPage: React.FC = () => {
  const router = useRouter();
  // POST /dashboards is `require_min_role("Analyst")`. Below that the builder
  // would only ever end in a refusal, so it is not opened at all — the Library
  // hides its New control too, and this covers arriving by URL.
  const { canCreate, whyNoCreate } = useRole();

  /**
   * Save dashboard to the API
   */
  const handleSave = useCallback(async (config: DashboardConfig) => {
    try {
      const payload = {
        name: config.name,
        description: config.description || "",
        theme: config.theme || "default",
        layout: JSON.stringify(config.layout),
        charts: JSON.stringify(config.chartIds || []),
        filters: JSON.stringify(config.filters || []),
        is_published: false,
        is_archived: false,
      };

      const response = await msalFetch(`${API_BASE}/api/v1/dashboards/`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify(payload),
      });

      if (!response.ok) {
        const errorData = await response.json().catch(() => ({}));
        throw new Error(errorData.detail || `Failed to create dashboard: ${response.status}`);
      }

      const savedDashboard = await response.json();

      // Navigate to the dashboard view page
      // `replace`, not `push`: the creation step is finished and must not stay
      // in history. With a push, Back from the saved dashboard returned INTO the
      // creation flow -- "after a chart is saved, Back should go to the
      // library" -- and on this page that flow would have created a second
      // dashboard. Replacing it leaves [wherever they came from, the dashboard], so
      // the page's own Back button, which returns where the reader came from,
      // reaches the library it was opened from.
      router.replace(`/dashboards/${savedDashboard.id}/view`);
    } catch (error) {
      console.error("Error saving dashboard:", error);
      throw error; // Re-throw to let the context handle the error state
    }
  }, [router]);

  if (!canCreate) {
    return (
      <div className="page-shell page-shell-wide">
        <div className="card page-empty-card">
          <p className="page-empty-title">You cannot create a dashboard</p>
          <p className="page-empty-body">{whyNoCreate("dashboard")}</p>
          <button
            type="button"
            onClick={() => router.push("/workspace?tab=dashboards")}
            style={{
              marginTop: 16, padding: "9px 18px", fontSize: 13, fontWeight: 500,
              background: "var(--accent)", color: "#fff", border: "none",
              borderRadius: 10, cursor: "pointer",
            }}
          >
            Browse dashboards
          </button>
        </div>
      </div>
    );
  }

  return (
    // DashboardBuilder renders its own full-height shell — wrapping it in a
    // second page shell doubled the padding and, worse, put an auto-height box
    // between the builder and the viewport so its canvas had no height to fill.
    <DashboardProvider onSave={handleSave}>
      <DashboardBuilder />
    </DashboardProvider>
  );
};

export default DashboardNewPage;
