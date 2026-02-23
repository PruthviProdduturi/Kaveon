import React from "react";

export default function DashboardProperties({ title, description, onChange }: { title: string; description: string; onChange: (field: string, value: string) => void }) {
  return (
    <div className="dashboard-properties" style={{ width: 320, padding: 16, borderLeft: "1px solid #eee" }}>
      <h3>Properties</h3>
      <label>
        Title
        <input value={title} onChange={e => onChange("title", e.target.value)} style={{ width: "100%" }} />
      </label>
      <label>
        Description
        <textarea value={description} onChange={e => onChange("description", e.target.value)} style={{ width: "100%" }} />
      </label>
      {/* Add more properties: refresh interval, filters, theme, etc. */}
    </div>
  );
}
