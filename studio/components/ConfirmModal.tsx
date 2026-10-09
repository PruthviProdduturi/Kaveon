"use client";

import React, { useEffect } from 'react';
import ReactDOM from 'react-dom';

interface ConfirmModalProps {
  isOpen: boolean;
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /**
   * Optional third choice, rendered between Cancel and Confirm. Use it when
   * dismissing the dialog would otherwise force the user to lose something —
   * for example "Discard and leave" next to a primary "Save and leave".
   */
  secondaryLabel?: string;
  onSecondary?: () => void;
  danger?: boolean;
  /** Font Awesome class for the leading icon. Defaults by `danger`. */
  icon?: string;
  /** Disables every action and spins the primary button while work is in flight. */
  busy?: boolean;
  /** Dialog width in pixels. Three actions need more room than two. */
  width?: number;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmModal({
  isOpen,
  title,
  message,
  confirmLabel = 'Remove',
  cancelLabel = 'Cancel',
  secondaryLabel,
  onSecondary,
  danger = true,
  icon,
  busy = false,
  width = 380,
  onConfirm,
  onCancel,
}: ConfirmModalProps) {
  useEffect(() => {
    if (!isOpen) return;
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape' && !busy) onCancel(); };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [isOpen, busy, onCancel]);

  if (!isOpen || typeof document === 'undefined') return null;

  const showSecondary = Boolean(secondaryLabel && onSecondary);

  return ReactDOM.createPortal(
    <div
      style={{
        position: 'fixed', inset: 0, zIndex: 9999,
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        background: 'rgba(15, 23, 42, 0.45)',
        backdropFilter: 'blur(2px)',
      }}
      onClick={(e) => { if (e.target === e.currentTarget && !busy) onCancel(); }}
    >
      <div
        role="dialog"
        aria-modal="true"
        style={{
          background: 'var(--bg-surface)',
          border: '1px solid var(--border)',
          borderRadius: 12,
          boxShadow: 'var(--shadow-lg)',
          padding: '28px 28px 24px',
          width,
          maxWidth: 'calc(100vw - 32px)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'flex-start', gap: 14, marginBottom: 12 }}>
          <div style={{
            width: 38, height: 38, borderRadius: 10, flexShrink: 0,
            background: danger
              ? 'color-mix(in srgb, var(--error) 14%, transparent)'
              : 'color-mix(in srgb, var(--accent) 14%, transparent)',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
          }}>
            <i
              className={icon || (danger ? 'fas fa-trash-alt' : 'fas fa-question-circle')}
              aria-hidden="true"
              style={{ fontSize: 16, color: danger ? 'var(--error)' : 'var(--accent)' }}
            />
          </div>
          <div>
            <div style={{ fontSize: 15, fontWeight: 700, color: 'var(--text-primary)', lineHeight: 1.3 }}>
              {title}
            </div>
            <div style={{ fontSize: 13, color: 'var(--text-muted)', marginTop: 4, lineHeight: 1.5 }}>
              {message}
            </div>
          </div>
        </div>

        <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', marginTop: 20, flexWrap: 'wrap' }}>
          <button
            onClick={onCancel}
            disabled={busy}
            style={{
              padding: '8px 18px',
              background: 'var(--bg-elevated)',
              border: '1px solid var(--border)',
              borderRadius: 7,
              cursor: busy ? 'not-allowed' : 'pointer',
              fontSize: 13,
              fontWeight: 500,
              color: 'var(--text-secondary)',
              fontFamily: 'inherit',
            }}
            onMouseOver={(e) => { if (!busy) e.currentTarget.style.background = 'var(--bg-hover)'; }}
            onMouseOut={(e) => { e.currentTarget.style.background = 'var(--bg-elevated)'; }}
          >
            {cancelLabel}
          </button>
          {showSecondary && (
            <button
              onClick={onSecondary}
              disabled={busy}
              style={{
                padding: '8px 18px',
                background: 'transparent',
                border: '1px solid color-mix(in srgb, var(--error) 45%, var(--border))',
                borderRadius: 7,
                cursor: busy ? 'not-allowed' : 'pointer',
                fontSize: 13,
                fontWeight: 500,
                color: 'var(--error)',
                fontFamily: 'inherit',
              }}
              onMouseOver={(e) => {
                if (!busy) e.currentTarget.style.background = 'color-mix(in srgb, var(--error) 10%, transparent)';
              }}
              onMouseOut={(e) => { e.currentTarget.style.background = 'transparent'; }}
            >
              {secondaryLabel}
            </button>
          )}
          <button
            onClick={onConfirm}
            disabled={busy}
            style={{
              padding: '8px 18px',
              background: danger ? 'var(--error)' : 'var(--accent)',
              border: 'none',
              borderRadius: 7,
              cursor: busy ? 'progress' : 'pointer',
              fontSize: 13,
              fontWeight: 600,
              color: '#fff',
              fontFamily: 'inherit',
              display: 'flex',
              alignItems: 'center',
              gap: 7,
              opacity: busy ? 0.85 : 1,
            }}
            onMouseOver={(e) => {
              if (busy) return;
              e.currentTarget.style.background = danger
                ? 'color-mix(in srgb, var(--error) 86%, black)'
                : 'var(--accent-dark)';
            }}
            onMouseOut={(e) => { e.currentTarget.style.background = danger ? 'var(--error)' : 'var(--accent)'; }}
          >
            {busy && <i className="fas fa-spinner fa-spin" aria-hidden="true" />}
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}
