"use client";

import React, { useCallback } from "react";
import { useRouter } from "next/navigation";
import { DashboardProvider, DashboardConfig } from "../../../components/dashboards/DashboardContext";
import DashboardBuilder from "../../../components/dashboards/DashboardBuilder";
import { msalFetch } from "../../../utils/msalFetch";
import { API_BASE } from "../../../config";

const DashboardNewPage: React.FC = () => {
  const router = useRouter();

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
      router.push(`/dashboards/${savedDashboard.id}/view`);
    } catch (error) {
      console.error("Error saving dashboard:", error);
      throw error; // Re-throw to let the context handle the error state
    }
  }, [router]);

  return (
    <DashboardProvider onSave={handleSave}>
      <div className="page-shell page-shell-wide">
        <DashboardBuilder />
      </div>
    </DashboardProvider>
  );
};

export default DashboardNewPage;
