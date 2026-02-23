import React from "react";

export default function DashboardSidebar({ charts, onAddChart }: { charts: any[]; onAddChart: (chart: any) => void }) {
  return (
    <div className="dashboard-sidebar" style={{ width: 240, padding: 16, borderRight: "1px solid #eee" }}>
      <h3>Charts</h3>
      {charts.map((chart) => (
        <button key={chart.id} style={{ display: "block", marginBottom: 8 }} onClick={() => onAddChart(chart)}>
          {chart.name}
        </button>
      ))}
      {/* Add more components: Text, Header, Divider, KPI, Spacer */}
    </div>
  );
}
