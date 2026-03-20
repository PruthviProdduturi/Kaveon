"use client";

import React, { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import { DashboardProvider, DashboardConfig } from "../../../../components/dashboards/DashboardContext";
import DashboardBuilder from "../../../../components/dashboards/DashboardBuilder";
import { LoadingOverlay } from "../../../../components/LoadingOverlay";
import { msalFetch } from "../../../../utils/msalFetch";
import { API_BASE } from "../../../../config";

export const dynamic = 'force-dynamic';
export const dynamicParams = true;

const DashboardEditPage: React.FC = () => {
  const params = useParams();
  const id = params?.id as string | undefined;
  const [initialConfig, setInitialConfig] = useState<DashboardConfig | undefined>(undefined);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  /**
   * Load dashboard data
   */
  useEffect(() => {
    if (!id) return;

    const loadDashboard = async () => {
      try {
        setLoading(true);
        const response = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}`);

        if (!response.ok) {
          throw new Error(`Failed to load dashboard: ${response.status}`);
        }

        const dashboard = await response.json();

        // Parse JSON fields
        const config: DashboardConfig = {
          id: dashboard.id,
          name: dashboard.name,
          description: dashboard.description || "",
          layout: (() => { const p = JSON.parse(dashboard.layout || "[]"); return Array.isArray(p) ? p : []; })(),
          filters: JSON.parse(dashboard.filters || "[]"),
          filterLogic: "AND",
          chartIds: JSON.parse(dashboard.charts || "[]"),
        };

        setInitialConfig(config);
        setError(null);
      } catch (err) {
        console.error("Error loading dashboard:", err);
        setError(err instanceof Error ? err.message : "Failed to load dashboard");
      } finally {
        setLoading(false);
      }
    };

    loadDashboard();
  }, [id]);

  /**
   * Save dashboard to the API
   */
  const handleSave = useCallback(
    async (config: DashboardConfig) => {
      if (!id) return;

      try {
        const payload = {
          name: config.name,
          description: config.description || "",
          layout: config.layout,
          charts: config.chartIds || [],
          filters: config.filters || [],
          is_published: false,
        };

        const response = await msalFetch(`${API_BASE}/api/v1/dashboards/${id}`, {
          method: "PUT",
          headers: {
            "Content-Type": "application/json",
          },
          body: JSON.stringify(payload),
        });

        if (!response.ok) {
          const errorData = await response.json().catch(() => ({}));
          const detail = Array.isArray(errorData.detail)
            ? errorData.detail.map((e: any) => e.msg || JSON.stringify(e)).join('; ')
            : errorData.detail;
          throw new Error(detail || `Failed to update dashboard: ${response.status}`);
        }
        // Stay on the edit page — context will clear hasUnsavedChanges
      } catch (error) {
        console.error("Error saving dashboard:", error);
        throw error; // Re-throw to let the context handle the error state
      }
    },
    [id]
  );

  // Show loading state
  if (loading) {
    return <LoadingOverlay />;
  }

  // Show error state
  if (error) {
    return (
      <div className="page-shell page-shell-wide">
        <div style={{ padding: 40, textAlign: "center" }}>
          <i className="fas fa-exclamation-circle" style={{ fontSize: 32, color: "#ef4444" }} />
          <div style={{ marginTop: 16, color: "#64748b" }}>{error}</div>
          <button
            onClick={() => router.push("/dashboards")}
            style={{
              marginTop: 16,
              padding: "8px 16px",
              background: "#2563eb",
              color: "#ffffff",
              border: "none",
              borderRadius: 6,
              cursor: "pointer",
            }}
          >
            Back to Dashboards
          </button>
        </div>
      </div>
    );
  }

  return (
    <DashboardProvider initialConfig={initialConfig} onSave={handleSave}>
      <div className="page-shell page-shell-wide">
        <DashboardBuilder dashboardId={id as string} />
      </div>
    </DashboardProvider>
  );
};

export default DashboardEditPage;
