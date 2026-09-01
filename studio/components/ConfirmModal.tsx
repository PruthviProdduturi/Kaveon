"use client";

import React, { useEffect } from 'react';
import ReactDOM from 'react-dom';

interface ConfirmModalProps {
  isOpen: boolean;
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmModal({
  isOpen,
  title,
  message,
  confirmLabel = 'Remove',
  cancelLabel = 'Cancel',
  danger = true,
  onConfirm,
  onCancel,
}: ConfirmModalProps) {
  useEffect(() => {
    if (!isOpen) return;
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape') onCancel(); };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [isOpen, onCancel]);

  if (!isOpen || typeof document === 'undefined') return null;

  return ReactDOM.createPortal(
    <div
      style={{
        position: 'fixed', inset: 0, zIndex: 9999,
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        background: 'rgba(15, 23, 42, 0.45)',
        backdropFilter: 'blur(2px)',
      }}
      onClick={(e) => { if (e.target === e.currentTarget) onCancel(); }}
    >
      <div
        style={{
          background: '#fff',
          borderRadius: 12,
          boxShadow: '0 20px 60px rgba(0,0,0,0.18)',
          padding: '28px 28px 24px',
          width: 380,
          maxWidth: 'calc(100vw - 32px)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'flex-start', gap: 14, marginBottom: 12 }}>
          <div style={{
            width: 38, height: 38, borderRadius: 10, flexShrink: 0,
            background: danger ? '#fef2f2' : '#eff6ff',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
          }}>
            <i
              className={danger ? 'fas fa-trash-alt' : 'fas fa-question-circle'}
              style={{ fontSize: 16, color: danger ? '#ef4444' : '#2563eb' }}
            />
          </div>
          <div>
            <div style={{ fontSize: 15, fontWeight: 700, color: '#0f172a', lineHeight: 1.3 }}>
              {title}
            </div>
            <div style={{ fontSize: 13, color: '#64748b', marginTop: 4, lineHeight: 1.5 }}>
              {message}
            </div>
          </div>
        </div>

        <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', marginTop: 20 }}>
          <button
            onClick={onCancel}
            style={{
              padding: '8px 18px',
              background: '#f8fafc',
              border: '1px solid #e2e8f0',
              borderRadius: 7,
              cursor: 'pointer',
              fontSize: 13,
              fontWeight: 500,
              color: '#475569',
            }}
            onMouseOver={(e) => { e.currentTarget.style.background = '#f1f5f9'; }}
            onMouseOut={(e) => { e.currentTarget.style.background = '#f8fafc'; }}
          >
            {cancelLabel}
          </button>
          <button
            onClick={onConfirm}
            style={{
              padding: '8px 18px',
              background: danger ? '#ef4444' : '#2563eb',
              border: 'none',
              borderRadius: 7,
              cursor: 'pointer',
              fontSize: 13,
              fontWeight: 600,
              color: '#fff',
            }}
            onMouseOver={(e) => { e.currentTarget.style.background = danger ? '#dc2626' : '#1d4ed8'; }}
            onMouseOut={(e) => { e.currentTarget.style.background = danger ? '#ef4444' : '#2563eb'; }}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}
