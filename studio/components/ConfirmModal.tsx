"use client";

import React, { useEffect, useId, useRef } from 'react';
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
  const ids = useId();
  const titleId = `${ids}-title`;
  const messageId = `${ids}-message`;
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!isOpen) return;
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape' && !busy) onCancel(); };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [isOpen, busy, onCancel]);

  /**
   * Focus belonged to whatever opened the dialog, and stayed there: the overlay
   * covered the page but not the tab order, so Tab walked the page behind it
   * and Enter re-fired the control that had just opened this. The dialog takes
   * focus on the least destructive action, keeps Tab inside itself, and hands
   * focus back to the opener when it closes.
   */
  useEffect(() => {
    if (!isOpen) return;
    const opener = document.activeElement as HTMLElement | null;
    cancelRef.current?.focus();
    const trap = (e: KeyboardEvent) => {
      if (e.key !== 'Tab') return;
      const root = dialogRef.current;
      if (!root) return;
      const focusable = Array.from(
        root.querySelectorAll<HTMLElement>('button:not([disabled]), [href], input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])'),
      );
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || !root.contains(active))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (active === last || !root.contains(active))) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener('keydown', trap);
    return () => {
      window.removeEventListener('keydown', trap);
      if (opener && document.body.contains(opener)) opener.focus();
    };
  }, [isOpen]);

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
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={messageId}
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
            <div id={titleId} style={{ fontSize: 15, fontWeight: 700, color: 'var(--text-primary)', lineHeight: 1.3 }}>
              {title}
            </div>
            <div id={messageId} style={{ fontSize: 13, color: 'var(--text-muted)', marginTop: 4, lineHeight: 1.5 }}>
              {message}
            </div>
          </div>
        </div>

        <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', marginTop: 20, flexWrap: 'wrap' }}>
          <button
            ref={cancelRef}
            type="button"
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
              // Fill tokens, not ink tokens. `--error` and `--accent` were
              // both chosen as text colours: white on dark's `--error`
              // measures 2.77:1 and on `--accent` 2.86:1, so the confirm
              // button -- the one that destroys something -- had the least
              // legible label in the dialog. These two carry white at
              // 5.74:1 and 5.17:1 in both themes.
              background: danger ? 'var(--danger-strong)' : 'var(--accent-strong)',
              border: 'none',
              borderRadius: 7,
              cursor: busy ? 'progress' : 'pointer',
              fontSize: 13,
              fontWeight: 600,
              color: 'var(--on-accent)',
              fontFamily: 'inherit',
              display: 'flex',
              alignItems: 'center',
              gap: 7,
              opacity: busy ? 0.85 : 1,
            }}
            onMouseOver={(e) => {
              if (busy) return;
              e.currentTarget.style.background = danger
                ? 'color-mix(in srgb, var(--danger-strong) 86%, black)'
                : 'var(--accent-strong-deep)';
            }}
            onMouseOut={(e) => { e.currentTarget.style.background = danger ? 'var(--danger-strong)' : 'var(--accent-strong)'; }}
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
