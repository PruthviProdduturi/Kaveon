import React from "react";

interface StatusCardProps {
  title: string;
  status?: string;
  details?: Record<string, unknown> | null;
}

export const StatusCard: React.FC<StatusCardProps> = ({ title, status, details }) => {
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <h2 style={{ fontSize: 18, fontWeight: 700 }}>{title}</h2>
        <span style={{ color: status === "ok" ? "#4ade80" : "#fbbf24" }}>{status ?? "pending"}</span>
      </div>
      {details && (
        <pre style={{ marginTop: 12, fontSize: 13, color: "#d1d5db" }}>
          {JSON.stringify(details, null, 2)}
        </pre>
      )}
    </div>
  );
};
