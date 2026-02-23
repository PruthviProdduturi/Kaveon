/**
 * Dashboard Canvas Component
 *
 * Renders the dashboard as a vertical stack of rows (Apache Superset style).
 * Each root-level item is rendered in order with full width.
 * In edit mode an "+ Add Row" button appears at the bottom.
 */

import React from 'react';
import { useDashboard } from './DashboardContext';
import DashboardItem from './DashboardItem';

interface DashboardCanvasProps {
  className?: string;
}

const DashboardCanvas: React.FC<DashboardCanvasProps> = ({ className = '' }) => {
  const { layout, isEditMode, addLayoutItem } = useDashboard();

  // Only show root-level items (items without a parentId)
  const rootItems = layout.filter((item) => !item.parentId);

  // ── Empty state ──────────────────────────────────────────────────────────────
  if (rootItems.length === 0) {
    return (
      <div className={`dashboard-canvas-empty ${className}`}>
        <div className="dashboard-canvas-empty-icon">
          <i className="fas fa-th-large" />
        </div>
        <div className="dashboard-canvas-empty-title">
          {isEditMode ? 'Your dashboard is empty' : 'No content available'}
        </div>
        <div className="dashboard-canvas-empty-subtitle">
          {isEditMode
            ? 'Add a row to start building your dashboard'
            : 'This dashboard has no content to display'}
        </div>
        {isEditMode && (
          <button
            onClick={() => addLayoutItem('row')}
            style={{
              marginTop: 20,
              padding: '10px 24px',
              background: '#2563eb',
              color: '#fff',
              border: 'none',
              borderRadius: 8,
              cursor: 'pointer',
              fontSize: 14,
              fontWeight: 600,
              display: 'inline-flex',
              alignItems: 'center',
              gap: 8,
            }}
            onMouseOver={(e) => { e.currentTarget.style.background = '#1d4ed8'; }}
            onMouseOut={(e) => { e.currentTarget.style.background = '#2563eb'; }}
          >
            <i className="fas fa-plus" />
            Add Row
          </button>
        )}
      </div>
    );
  }

  // ── Populated layout ─────────────────────────────────────────────────────────
  return (
    <div
      className={`dashboard-canvas ${className}`}
      style={{ paddingBottom: 32 }}
    >
      {rootItems.map((item) => (
        <div key={item.i} style={{ marginBottom: 16 }}>
          <DashboardItem item={item} isEditMode={isEditMode} />
        </div>
      ))}

      {/* Add Row button */}
      {isEditMode && (
        <div style={{ display: 'flex', justifyContent: 'center', padding: '4px 0 8px' }}>
          <button
            onClick={() => addLayoutItem('row')}
            style={{
              padding: '8px 20px',
              background: 'transparent',
              color: '#94a3b8',
              border: '2px dashed #cbd5e1',
              borderRadius: 8,
              cursor: 'pointer',
              fontSize: 13,
              fontWeight: 600,
              display: 'inline-flex',
              alignItems: 'center',
              gap: 8,
              transition: 'all 0.15s ease',
            }}
            onMouseOver={(e) => {
              e.currentTarget.style.borderColor = '#2563eb';
              e.currentTarget.style.color = '#2563eb';
              e.currentTarget.style.background = '#eff6ff';
            }}
            onMouseOut={(e) => {
              e.currentTarget.style.borderColor = '#cbd5e1';
              e.currentTarget.style.color = '#94a3b8';
              e.currentTarget.style.background = 'transparent';
            }}
          >
            <i className="fas fa-plus" />
            Add Row
          </button>
        </div>
      )}
    </div>
  );
};

export default DashboardCanvas;
