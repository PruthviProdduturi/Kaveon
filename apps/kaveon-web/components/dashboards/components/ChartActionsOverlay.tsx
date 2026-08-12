"use client";

import React, { useState, useRef, useEffect } from 'react';
import ReactDOM from 'react-dom';
import dynamic from 'next/dynamic';
import { useRouter } from 'next/navigation';
import { useChartBuilder } from '../../charts/ChartBuilderContext';

// Rendered inside the fullscreen modal — the same chart, big. Loaded lazily so it
// doesn't bloat the tile bundle.
const ChartPreview = dynamic(() => import('../../charts/ChartPreview'), { ssr: false });

interface ChartActionsOverlayProps {
  chartId: number;
  dashboardId?: string | null;
  isEditMode: boolean;
  onRefresh: () => void;
  onDuplicate?: () => void;
  onRemove?: () => void;
  exportsRef: React.MutableRefObject<{ downloadPng: () => void; downloadCsv: () => void } | null>;
}

const DIVIDER = '---';

const ChartActionsOverlay: React.FC<ChartActionsOverlayProps> = ({
  chartId, dashboardId, isEditMode, onRefresh, onDuplicate, onRemove, exportsRef,
}) => {
  const router = useRouter();
  const { sqlPreview } = useChartBuilder();
  const [open, setOpen]               = useState(false);   // menu open
  const [fullScreen, setFullScreen]   = useState(false);
  const [showQuery, setShowQuery]     = useState(false);
  const [showTable, setShowTable]     = useState(false);
  const [copied, setCopied]           = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const btnRef  = useRef<HTMLButtonElement>(null);

  // Close menu on outside click
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node) &&
          btnRef.current  && !btnRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  // Close on Escape
  useEffect(() => {
    const h = (e: KeyboardEvent) => { if (e.key === 'Escape') { setOpen(false); setFullScreen(false); setShowQuery(false); setShowTable(false); } };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, []);

  const hasData = !!(sqlPreview?.dataRows?.length);
  const queryText: string = (sqlPreview as any)?.sql || (sqlPreview as any)?.lastSql || '';

  const share = () => {
    const url = dashboardId
      ? `${window.location.origin}/dashboards/${dashboardId}/view`
      : window.location.href;
    navigator.clipboard.writeText(url).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
    setOpen(false);
  };

  const menuItems = [
    { icon: 'fas fa-arrow-up-right-from-square', label: 'View chart', action: () => { router.push(`/charts/${chartId}`); setOpen(false); } },
    { icon: 'fas fa-sync-alt',     label: 'Refresh',           action: () => { onRefresh(); setOpen(false); } },
    { icon: 'fas fa-expand',       label: 'Full screen',       action: () => { setFullScreen(true); setOpen(false); } },
    DIVIDER,
    ...(isEditMode ? [{ icon: 'fas fa-edit', label: 'Edit chart', action: () => { router.push(`/charts/${chartId}`); setOpen(false); } }] : []),
    { icon: 'fas fa-code',         label: 'View query',        action: () => { setShowQuery(true); setOpen(false); }, disabled: !queryText },
    { icon: 'fas fa-table',        label: 'View as table',     action: () => { setShowTable(true); setOpen(false); }, disabled: !hasData },
    DIVIDER,
    { icon: 'fas fa-file-csv',     label: 'Download CSV',      action: () => { exportsRef.current?.downloadCsv(); setOpen(false); }, disabled: !hasData },
    { icon: 'fas fa-image',        label: 'Download PNG',      action: () => { exportsRef.current?.downloadPng(); setOpen(false); } },
    DIVIDER,
    { icon: copied ? 'fas fa-check' : 'fas fa-share-alt', label: copied ? 'Copied!' : 'Share dashboard', action: share },
    ...(isEditMode && onDuplicate ? [DIVIDER, { icon: 'fas fa-copy', label: 'Duplicate', action: () => { onDuplicate(); setOpen(false); } }] : []),
    ...(isEditMode && onRemove    ? [{ icon: 'fas fa-trash', label: 'Remove', danger: true, action: () => { setConfirmRemove(true); setOpen(false); } }] : []),
  ];

  return (
    <>
      {/* ⋯ button — always visible */}
      <button
          ref={btnRef}
          onClick={(e) => { e.stopPropagation(); setOpen(v => !v); }}
          title="Chart options"
          style={{
            position: 'absolute',
            top: 6, right: 6,
            zIndex: 20,
            width: 28, height: 28,
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            background: open ? 'rgba(var(--accent-rgb), 0.15)' : 'var(--bg-elevated)',
            border: '1px solid var(--border)',
            borderRadius: 6,
            cursor: 'pointer',
            boxShadow: '0 1px 4px rgba(0,0,0,0.10)',
            fontSize: 14,
            color: open ? 'var(--accent)' : 'var(--text-muted)',
            pointerEvents: 'auto',
          }}
        >
          <i className="fas fa-ellipsis-h" style={{ fontSize: 12 }} />
        </button>

      {/* Dropdown menu */}
      {open && (
        <div
          ref={menuRef}
          style={{
            position: 'absolute',
            top: 36, right: 6,
            zIndex: 100,
            background: '#fff',
            border: '1px solid var(--border)',
            borderRadius: 8,
            boxShadow: '0 8px 24px rgba(0,0,0,0.4)',
            minWidth: 190,
            overflow: 'hidden',
            pointerEvents: 'auto',
          }}
        >
          {menuItems.map((mi, i) => {
            if (mi === DIVIDER) {
              return <div key={i} style={{ height: 1, background: 'var(--border)', margin: '3px 0' }} />;
            }
            const item = mi as any;
            return (
              <button
                key={i}
                onClick={item.action}
                disabled={item.disabled}
                style={{
                  width: '100%',
                  display: 'flex', alignItems: 'center', gap: 9,
                  padding: '8px 14px',
                  background: 'transparent',
                  border: 'none', cursor: item.disabled ? 'default' : 'pointer',
                  fontSize: 13,
                  color: item.danger ? '#ef4444' : item.disabled ? 'var(--text-faint)' : 'var(--text-primary)',
                  textAlign: 'left',
                  whiteSpace: 'nowrap',
                }}
                onMouseOver={(e) => { if (!item.disabled) e.currentTarget.style.background = item.danger ? 'rgba(239,68,68,0.1)' : 'var(--bg-hover)'; }}
                onMouseOut={(e) => { e.currentTarget.style.background = 'transparent'; }}
              >
                <i className={item.icon} style={{ fontSize: 12, width: 14, textAlign: 'center', color: item.danger ? '#ef4444' : item.disabled ? 'var(--text-faint)' : 'var(--text-muted)' }} />
                {item.label}
              </button>
            );
          })}
        </div>
      )}

      {/* Full-screen modal */}
      {fullScreen && typeof document !== 'undefined' && ReactDOM.createPortal(
        <div
          onClick={() => setFullScreen(false)}
          style={{
            position: 'fixed', inset: 0, zIndex: 9999,
            background: 'rgba(15,23,42,0.7)',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            padding: 24,
          }}
        >
          <div
            onClick={e => e.stopPropagation()}
            style={{
              background: 'var(--bg-surface)', borderRadius: 12,
              width: '90vw', height: '85vh',
              display: 'flex', flexDirection: 'column',
              overflow: 'hidden',
              boxShadow: '0 24px 64px rgba(0,0,0,0.25)',
            }}
          >
            <div style={{ display: 'flex', justifyContent: 'flex-end', padding: '10px 14px', borderBottom: '1px solid var(--border)' }}>
              <button
                onClick={() => setFullScreen(false)}
                style={{ background: 'transparent', border: 'none', cursor: 'pointer', color: 'var(--text-muted)', fontSize: 18, padding: '0 4px' }}
              >
                <i className="fas fa-times" />
              </button>
            </div>
            <div style={{ flex: 1, padding: 16, overflow: 'hidden', position: 'relative', display: 'flex' }}>
              {/* Render the real chart big — reads the same ChartBuilder context as
                  the tile, so it shows the exact same data/options. */}
              <ChartPreview />
            </div>
          </div>
        </div>,
        document.body
      )}

      {/* View Query modal */}
      {showQuery && typeof document !== 'undefined' && ReactDOM.createPortal(
        <div
          onClick={() => setShowQuery(false)}
          style={{ position: 'fixed', inset: 0, zIndex: 9999, background: 'rgba(15,23,42,0.55)', display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 24 }}
        >
          <div onClick={e => e.stopPropagation()} style={{ background: 'var(--bg-surface)', borderRadius: 12, width: '70vw', maxHeight: '70vh', display: 'flex', flexDirection: 'column', boxShadow: '0 24px 64px rgba(0,0,0,0.2)' }}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '14px 20px', borderBottom: '1px solid var(--border)' }}>
              <span style={{ fontWeight: 700, fontSize: 14, color: 'var(--text-primary)' }}>View Query</span>
              <button onClick={() => setShowQuery(false)} style={{ background: 'transparent', border: 'none', cursor: 'pointer', color: 'var(--text-muted)', fontSize: 16 }}><i className="fas fa-times" /></button>
            </div>
            <div style={{ flex: 1, overflow: 'auto', padding: 20 }}>
              <pre style={{ margin: 0, fontSize: 12, lineHeight: 1.6, color: 'var(--text-primary)', background: 'var(--bg-elevated)', borderRadius: 6, padding: 16, border: '1px solid var(--border)', whiteSpace: 'pre-wrap', wordBreak: 'break-all', fontFamily: 'ui-monospace, monospace' }}>
                {queryText || 'No query available.'}
              </pre>
            </div>
          </div>
        </div>,
        document.body
      )}

      {/* View as Table modal */}
      {showTable && typeof document !== 'undefined' && ReactDOM.createPortal(
        <div
          onClick={() => setShowTable(false)}
          style={{ position: 'fixed', inset: 0, zIndex: 9999, background: 'rgba(15,23,42,0.55)', display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 24 }}
        >
          <div onClick={e => e.stopPropagation()} style={{ background: 'var(--bg-surface)', borderRadius: 12, width: '80vw', maxHeight: '75vh', display: 'flex', flexDirection: 'column', boxShadow: '0 24px 64px rgba(0,0,0,0.2)' }}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '14px 20px', borderBottom: '1px solid var(--border)' }}>
              <span style={{ fontWeight: 700, fontSize: 14, color: 'var(--text-primary)' }}>View as Table</span>
              <button onClick={() => setShowTable(false)} style={{ background: 'transparent', border: 'none', cursor: 'pointer', color: 'var(--text-muted)', fontSize: 16 }}><i className="fas fa-times" /></button>
            </div>
            <div style={{ flex: 1, overflow: 'auto' }}>
              {sqlPreview?.dataColumns?.length ? (
                <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 12 }}>
                  <thead>
                    <tr style={{ background: 'var(--bg-elevated)', borderBottom: '2px solid var(--border)' }}>
                      {sqlPreview.dataColumns.map((col: string, i: number) => (
                        <th key={i} style={{ padding: '8px 12px', textAlign: 'left', fontWeight: 600, color: 'var(--text-secondary)', whiteSpace: 'nowrap', borderRight: i < sqlPreview.dataColumns.length - 1 ? '1px solid var(--border)' : 'none' }}>{col}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {(sqlPreview.dataRows || []).slice(0, 500).map((row: any[], ri: number) => (
                      <tr key={ri} style={{ borderBottom: '1px solid var(--border)', background: ri % 2 === 0 ? 'var(--bg-surface)' : 'var(--bg-hover)' }}>
                        {row.map((cell: any, ci: number) => (
                          <td key={ci} style={{ padding: '6px 12px', color: 'var(--text-primary)', borderRight: ci < row.length - 1 ? '1px solid var(--border)' : 'none' }}>{cell == null ? '—' : String(cell)}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <div style={{ padding: 24, color: 'var(--text-muted)', textAlign: 'center', fontSize: 13 }}>No data available.</div>
              )}
            </div>
          </div>
        </div>,
        document.body
      )}

      {/* Confirm remove modal */}
      {confirmRemove && typeof document !== 'undefined' && ReactDOM.createPortal(
        <div
          onClick={() => setConfirmRemove(false)}
          style={{ position: 'fixed', inset: 0, zIndex: 9999, background: 'rgba(15,23,42,0.45)', backdropFilter: 'blur(2px)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}
        >
          <div onClick={e => e.stopPropagation()} style={{ background: 'var(--bg-surface)', borderRadius: 12, boxShadow: '0 20px 60px rgba(0,0,0,0.4)', padding: '28px 28px 24px', width: 380, maxWidth: 'calc(100vw - 32px)' }}>
            <div style={{ display: 'flex', alignItems: 'flex-start', gap: 14, marginBottom: 12 }}>
              <div style={{ width: 38, height: 38, borderRadius: 10, flexShrink: 0, background: 'rgba(239,68,68,0.1)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                <i className="fas fa-trash-alt" style={{ fontSize: 16, color: '#ef4444' }} />
              </div>
              <div>
                <div style={{ fontSize: 15, fontWeight: 700, color: 'var(--text-primary)', lineHeight: 1.3 }}>Remove chart</div>
                <div style={{ fontSize: 13, color: 'var(--text-muted)', marginTop: 4, lineHeight: 1.5 }}>This chart will be removed from the dashboard.</div>
              </div>
            </div>
            <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', marginTop: 20 }}>
              <button onClick={() => setConfirmRemove(false)} style={{ padding: '8px 18px', background: '#f8fafc', border: '1px solid var(--border)', borderRadius: 7, cursor: 'pointer', fontSize: 13, fontWeight: 500, color: 'var(--text-secondary)' }}>Cancel</button>
              <button onClick={() => { setConfirmRemove(false); onRemove?.(); }} style={{ padding: '8px 18px', background: '#ef4444', border: 'none', borderRadius: 7, cursor: 'pointer', fontSize: 13, fontWeight: 600, color: '#fff' }}>Remove</button>
            </div>
          </div>
        </div>,
        document.body
      )}
    </>
  );
};

export default ChartActionsOverlay;
