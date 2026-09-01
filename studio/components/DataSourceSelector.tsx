"use client";

import { useEffect, useState } from "react";
import { API_BASE } from "../config";
import { msalFetch } from "../utils/msalFetch";
import { useAuth } from "../auth/useAuth";

interface DataSource {
  id: number;
  name: string;
  type: string;
  database_name?: string;
  region: string;
  is_active: boolean;
  is_favorite?: number;
  table_count?: number;
}

interface DataSourceSelectorProps {
  value?: number | null;
  onChange: (dataSourceId: number | null) => void;
  label?: string;
  required?: boolean;
  className?: string;
}

export function DataSourceSelector({
  value,
  onChange,
  label = "Data Source",
  required = false,
  className = "chart-builder-field"
}: DataSourceSelectorProps) {
  const { account, isAuthenticated } = useAuth();
  const [dataSources, setDataSources] = useState<DataSource[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isAuthenticated) return;
    loadDataSources();
  }, [isAuthenticated]);

  const loadDataSources = async () => {
    try {
      setLoading(true);
      const userEmail = account?.email || account?.username || null;
      const response = await msalFetch(`${API_BASE}/api/v1/data-sources/active`, {
        headers: userEmail ? { 'x-user-email': userEmail } : undefined
      });
      if (!response.ok) throw new Error("Failed to load data sources");
      const data = await response.json();
      const sources = data.dataSources || [];
      setDataSources(sources);

      // Auto-select favorite if no value is set
      if (!value) {
        const favorite = sources.find((ds: DataSource) => ds.is_favorite === 1);
        if (favorite) {
          onChange(favorite.id);
        }
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load data sources");
    } finally {
      setLoading(false);
    }
  };

  if (loading) {
    return (
      <div className={className}>
        <label className="chart-builder-label">
          <span>{label} {required && '*'}</span>
        </label>
        <div style={{ padding: '0.5rem', color: '#9ca3af', fontSize: '0.85rem' }}>
          <i className="fas fa-spinner fa-spin" /> Loading data sources...
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className={className}>
        <label className="chart-builder-label">
          <span>{label} {required && '*'}</span>
        </label>
        <div style={{ padding: '0.5rem', color: '#dc2626', fontSize: '0.85rem' }}>
          <i className="fas fa-exclamation-circle" /> {error}
        </div>
      </div>
    );
  }

  return (
    <div className={className}>
      <label className="chart-builder-label">
        <span>{label} {required && '*'}</span>
      </label>
      <select
        className="chart-builder-select"
        value={value || ''}
        onChange={(e) => onChange(e.target.value ? parseInt(e.target.value) : null)}
        required={required}
      >
        {!required && <option value="">-- Select Data Source --</option>}
        {dataSources.map((ds) => (
          <option key={ds.id} value={ds.id}>
            {ds.name}
          </option>
        ))}
        {dataSources.length === 0 && (
          <option value="" disabled>No data sources available</option>
        )}
      </select>
    </div>
  );
}
