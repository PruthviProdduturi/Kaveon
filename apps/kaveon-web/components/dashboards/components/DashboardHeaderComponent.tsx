/**
 * Dashboard Header Component — rich section heading
 *
 * View mode  : renders heading at chosen size, alignment, colour
 * Edit mode  : hover shows a floating toolbar with:
 *                - H1 / H2 / H3 size picker
 *                - Left / Centre / Right alignment
 *                - Text colour swatch (opens HexColorPicker)
 *                - Delete
 *              Click text to edit inline
 */

import React, { useState, useEffect, useRef } from 'react';
import { HexColorPicker } from 'react-colorful';
import type { DashboardComponentProps } from '../../../types/dashboard';
import { ConfirmModal } from '../../ConfirmModal';

const SIZE_OPTIONS = [
  { value: 'large',  label: 'H1', fontSize: 24, fontWeight: 800 },
  { value: 'medium', label: 'H2', fontSize: 18, fontWeight: 700 },
  { value: 'small',  label: 'H3', fontSize: 15, fontWeight: 600 },
] as const;

const ALIGNMENTS: { val: 'left' | 'center' | 'right'; icon: string; title: string }[] = [
  { val: 'left',   icon: 'fas fa-align-left',   title: 'Left' },
  { val: 'center', icon: 'fas fa-align-center',  title: 'Centre' },
  { val: 'right',  icon: 'fas fa-align-right',   title: 'Right' },
];

const DashboardHeaderComponent: React.FC<DashboardComponentProps> = ({ item, isEditMode, onConfigChange, onRemove }) => {
  const [content,     setContent]     = useState(item.headerConfig?.content   || '');
  const [size,        setSize]        = useState(item.headerConfig?.size       || 'large');
  const [alignment,   setAlignment]   = useState<'left'|'center'|'right'>(item.headerConfig?.alignment || 'left');
  const [color,       setColor]       = useState(item.headerConfig?.color      || '#0f172a');
  const [isEditing,   setIsEditing]   = useState(false);
  const [hovered,     setHovered]     = useState(false);
  const [showColor,   setShowColor]   = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);

  const inputRef    = useRef<HTMLInputElement>(null);
  const colorRef    = useRef<HTMLDivElement>(null);
  const toolbarRef  = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setContent(item.headerConfig?.content   || '');
    setSize(item.headerConfig?.size          || 'large');
    setAlignment(item.headerConfig?.alignment || 'left');
    setColor(item.headerConfig?.color         || '#0f172a');
  }, [item.headerConfig?.content, item.headerConfig?.size, item.headerConfig?.alignment, item.headerConfig?.color]);

  useEffect(() => {
    if (isEditing) requestAnimationFrame(() => inputRef.current?.select());
  }, [isEditing]);

  // Close colour picker on outside click
  useEffect(() => {
    if (!showColor) return;
    const h = (e: MouseEvent) => {
      if (colorRef.current && !colorRef.current.contains(e.target as Node)) setShowColor(false);
    };
    document.addEventListener('mousedown', h);
    return () => document.removeEventListener('mousedown', h);
  }, [showColor]);

  const opt = SIZE_OPTIONS.find(o => o.value === size) ?? SIZE_OPTIONS[0];

  const commit = () => {
    setIsEditing(false);
    onConfigChange?.(item.i, { headerConfig: { content, size, alignment, color } });
  };

  const changeSize = (v: typeof size) => {
    setSize(v);
    onConfigChange?.(item.i, { headerConfig: { content, size: v, alignment, color } });
  };

  const changeAlignment = (v: 'left' | 'center' | 'right') => {
    setAlignment(v);
    onConfigChange?.(item.i, { headerConfig: { content, size, alignment: v, color } });
  };

  const changeColor = (c: string) => {
    setColor(c);
    // don't flood the context on every picker drag — committed on picker close / done
  };

  const commitColor = () => {
    onConfigChange?.(item.i, { headerConfig: { content, size, alignment, color } });
  };

  return (
    <>
      <div
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => { setHovered(false); }}
        style={{ position: 'relative', width: '100%', height: '100%', padding: '4px 8px', boxSizing: 'border-box', display: 'flex', alignItems: 'center' }}
      >
        {/* ── Hover toolbar ── */}
        {isEditMode && (hovered || showColor) && (
          <div
            ref={toolbarRef}
            style={{
              position: 'absolute', top: 0, right: 8, zIndex: 20,
              display: 'flex', alignItems: 'center', gap: 2,
              background: '#fff', border: '1px solid #e2e8f0',
              borderRadius: 7, padding: '3px 6px',
              boxShadow: '0 3px 10px rgba(0,0,0,0.10)',
            }}
          >
            {/* Size */}
            {SIZE_OPTIONS.map(o => (
              <button
                key={o.value}
                onClick={() => changeSize(o.value)}
                title={`Heading ${o.label}`}
                style={{
                  width: 26, height: 24,
                  display: 'flex', alignItems: 'center', justifyContent: 'center',
                  background: size === o.value ? '#eff6ff' : 'transparent',
                  border: size === o.value ? '1px solid #93c5fd' : '1px solid transparent',
                  borderRadius: 4, cursor: 'pointer',
                  fontSize: 10, fontWeight: 800,
                  color: size === o.value ? '#2563eb' : '#64748b',
                }}
              >
                {o.label}
              </button>
            ))}

            <div style={{ width: 1, height: 16, background: '#e2e8f0', margin: '0 2px' }} />

            {/* Alignment */}
            {ALIGNMENTS.map(a => (
              <button
                key={a.val}
                onClick={() => changeAlignment(a.val)}
                title={a.title}
                style={{
                  width: 24, height: 24,
                  display: 'flex', alignItems: 'center', justifyContent: 'center',
                  background: alignment === a.val ? '#eff6ff' : 'transparent',
                  border: alignment === a.val ? '1px solid #93c5fd' : '1px solid transparent',
                  borderRadius: 4, cursor: 'pointer', fontSize: 10,
                  color: alignment === a.val ? '#2563eb' : '#64748b',
                }}
              >
                <i className={a.icon} />
              </button>
            ))}

            <div style={{ width: 1, height: 16, background: '#e2e8f0', margin: '0 2px' }} />

            {/* Colour swatch */}
            <div ref={colorRef} style={{ position: 'relative' }}>
              <button
                title="Text colour"
                onClick={() => { setShowColor(v => !v); }}
                style={{
                  width: 24, height: 24,
                  display: 'flex', alignItems: 'center', justifyContent: 'center',
                  background: 'transparent', border: '1px solid #e2e8f0',
                  borderRadius: 4, cursor: 'pointer', padding: 0,
                }}
              >
                <span style={{ width: 14, height: 14, borderRadius: 3, background: color, border: '1px solid rgba(0,0,0,0.18)', display: 'block' }} />
              </button>
              {showColor && (
                <div style={{ position: 'absolute', top: 28, right: 0, zIndex: 50, boxShadow: '0 4px 16px rgba(0,0,0,0.15)', borderRadius: 8, overflow: 'hidden', border: '1px solid #e2e8f0', background: '#fff', padding: 8 }}>
                  <HexColorPicker color={color} onChange={changeColor} />
                  <button
                    onClick={() => { setShowColor(false); commitColor(); }}
                    style={{ width: '100%', marginTop: 8, padding: '4px 0', background: '#2563eb', color: '#fff', border: 'none', borderRadius: 5, cursor: 'pointer', fontSize: 12, fontWeight: 600 }}
                  >
                    Apply
                  </button>
                </div>
              )}
            </div>

            <div style={{ width: 1, height: 16, background: '#e2e8f0', margin: '0 2px' }} />

            {/* Delete */}
            <button
              onClick={() => setConfirmOpen(true)}
              title="Remove header"
              style={{
                width: 24, height: 24,
                display: 'flex', alignItems: 'center', justifyContent: 'center',
                background: 'transparent', border: '1px solid transparent',
                borderRadius: 4, cursor: 'pointer', color: '#ef4444', fontSize: 11,
              }}
              onMouseOver={e => { e.currentTarget.style.background = '#fef2f2'; e.currentTarget.style.borderColor = '#fecaca'; }}
              onMouseOut={e  => { e.currentTarget.style.background = 'transparent'; e.currentTarget.style.borderColor = 'transparent'; }}
            >
              <i className="fas fa-trash" />
            </button>
          </div>
        )}

        {/* ── Content ── */}
        {isEditing ? (
          <input
            ref={inputRef}
            value={content}
            onChange={e => setContent(e.target.value)}
            onBlur={commit}
            onKeyDown={e => { if (e.key === 'Enter' || e.key === 'Escape') commit(); }}
            style={{
              width: '100%',
              fontSize: opt.fontSize,
              fontWeight: opt.fontWeight,
              color,
              textAlign: alignment,
              background: 'transparent',
              border: 'none',
              borderBottom: '2px solid #2563eb',
              outline: 'none',
              fontFamily: 'inherit',
              padding: '2px 0',
            }}
          />
        ) : (
          <div
            onClick={() => { if (isEditMode) setIsEditing(true); }}
            title={isEditMode ? 'Click to edit' : undefined}
            style={{
              width: '100%',
              fontSize: opt.fontSize,
              fontWeight: opt.fontWeight,
              color,
              textAlign: alignment,
              cursor: isEditMode ? 'text' : 'default',
              lineHeight: 1.3,
              letterSpacing: size === 'large' ? '-0.3px' : 'normal',
            }}
          >
            {content || (isEditMode
              ? <span style={{ color: '#94a3b8', fontWeight: 400, fontSize: 14 }}>Click to add header…</span>
              : ''
            )}
          </div>
        )}
      </div>

      <ConfirmModal
        isOpen={confirmOpen}
        title="Remove header"
        message="This header will be removed from the dashboard."
        confirmLabel="Remove"
        onConfirm={() => { setConfirmOpen(false); onRemove?.(item.i); }}
        onCancel={() => setConfirmOpen(false)}
      />
    </>
  );
};

export default DashboardHeaderComponent;
