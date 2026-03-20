/**
 * Dashboard Divider Component
 *
 * A clean horizontal rule that separates dashboard sections.
 * Superset-style: no header bar, just the line.
 * In edit mode a hover overlay shows delete + line-style controls.
 */

import React, { useState } from 'react';
import type { DashboardComponentProps } from '../../../types/dashboard';
import { ConfirmModal } from '../../ConfirmModal';

const LINE_STYLES = ['solid', 'dashed', 'dotted'] as const;
type LineStyle = typeof LINE_STYLES[number];

const DashboardDividerComponent: React.FC<DashboardComponentProps> = ({
  item,
  isEditMode,
  onRemove,
  onConfigChange,
}) => {
  const cfg       = item.dividerConfig;
  const lineStyle = (cfg?.style as LineStyle) || 'solid';
  const color     = cfg?.color     || '#e2e8f0';
  const thickness = cfg?.thickness ?? 1;

  const [hovered,     setHovered]     = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);

  const set = (key: string, value: any) =>
    onConfigChange?.(item.i, { dividerConfig: { ...cfg, [key]: value } as any });

  return (
    <>
      <div
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{
          position: 'relative',
          width: '100%',
          height: '100%',
          display: 'flex',
          alignItems: 'center',
          padding: '0 8px',
          boxSizing: 'border-box',
          outline: isEditMode && hovered ? '1px dashed #cbd5e1' : 'none',
          borderRadius: 4,
          transition: 'outline 0.1s',
        }}
      >
        {/* The line itself */}
        <div
          style={{
            width: '100%',
            height: 0,
            borderTop: `${thickness}px ${lineStyle} ${color}`,
          }}
        />

        {/* Edit-mode hover toolbar */}
        {isEditMode && hovered && (
          <div
            style={{
              position: 'absolute',
              top: '50%',
              right: 8,
              transform: 'translateY(-50%)',
              display: 'flex',
              alignItems: 'center',
              gap: 4,
              background: '#fff',
              border: '1px solid #e2e8f0',
              borderRadius: 6,
              padding: '3px 6px',
              boxShadow: '0 2px 8px rgba(0,0,0,0.1)',
              zIndex: 10,
            }}
          >
            {/* Line-style buttons */}
            {LINE_STYLES.map((s) => {
              const active = lineStyle === s;
              return (
                <button
                  key={s}
                  title={s.charAt(0).toUpperCase() + s.slice(1)}
                  onClick={() => set('style', s)}
                  style={{
                    width: 28,
                    height: 22,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    background: active ? '#eff6ff' : 'transparent',
                    border: active ? '1px solid #bfdbfe' : '1px solid transparent',
                    borderRadius: 4,
                    cursor: 'pointer',
                    padding: 0,
                  }}
                >
                  <div style={{
                    width: 16,
                    height: 0,
                    borderTop: `2px ${s} ${active ? '#2563eb' : '#94a3b8'}`,
                  }} />
                </button>
              );
            })}

            <div style={{ width: 1, height: 14, background: '#e2e8f0', margin: '0 2px' }} />

            {/* Delete button */}
            <button
              title="Remove divider"
              onClick={() => setConfirmOpen(true)}
              style={{
                width: 24,
                height: 22,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                background: 'transparent',
                border: '1px solid transparent',
                borderRadius: 4,
                cursor: 'pointer',
                color: '#ef4444',
                fontSize: 11,
                padding: 0,
              }}
              onMouseOver={(e) => {
                e.currentTarget.style.background = '#fef2f2';
                e.currentTarget.style.borderColor = '#fecaca';
              }}
              onMouseOut={(e) => {
                e.currentTarget.style.background = 'transparent';
                e.currentTarget.style.borderColor = 'transparent';
              }}
            >
              <i className="fas fa-trash" />
            </button>
          </div>
        )}
      </div>

      <ConfirmModal
        isOpen={confirmOpen}
        title="Remove divider"
        message="This divider will be removed from the dashboard."
        confirmLabel="Remove"
        onConfirm={() => { setConfirmOpen(false); onRemove?.(item.i); }}
        onCancel={() => setConfirmOpen(false)}
      />
    </>
  );
};

export default DashboardDividerComponent;
