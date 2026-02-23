"use client";

import React, { useState, useEffect } from 'react';
import { HexColorPicker } from 'react-colorful';
import { useTheme } from '../contexts/ThemeContext';
import { generateGradients } from '../utils/colorUtils';

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
}

const DEFAULT_THEME = '#0078D4'; // Microsoft blue default

export function SettingsModal({ isOpen, onClose }: SettingsModalProps) {
  const { primaryColor, setTheme, resetTheme, isLoading } = useTheme();
  const [tempColor, setTempColor] = useState<string>(primaryColor);
  const [isSaving, setIsSaving] = useState<boolean>(false);
  const [error, setError] = useState<string>('');

  // Sync tempColor with primaryColor when modal opens
  useEffect(() => {
    if (isOpen) {
      setTempColor(primaryColor);
      setError('');
    }
  }, [isOpen, primaryColor]);

  const handleSave = async () => {
    try {
      setIsSaving(true);
      setError('');
      await setTheme(tempColor);
      onClose();
    } catch (err: any) {
      setError(err.message || 'Failed to save theme');
    } finally {
      setIsSaving(false);
    }
  };

  const handleReset = async () => {
    try {
      setIsSaving(true);
      setError('');
      await resetTheme();
      setTempColor(DEFAULT_THEME);
    } catch (err: any) {
      setError(err.message || 'Failed to reset theme');
    } finally {
      setIsSaving(false);
    }
  };

  const handleClose = () => {
    if (!isSaving) {
      onClose();
    }
  };

  if (!isOpen) return null;

  const previewGradients = generateGradients(tempColor);

  return (
    <>
      <div className="settings-modal-backdrop" onClick={handleClose} />
      <div className="settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="settings-modal-header">
          <h2>Themes</h2>
          <button
            className="settings-modal-close"
            onClick={handleClose}
            disabled={isSaving}
            aria-label="Close"
          >
            ×
          </button>
        </div>

        <div className="settings-modal-body">
          <div className="settings-section">
            <h3>Theme Color</h3>
            <p className="settings-description">
              Choose your primary theme color. This will apply to the logo, buttons, and UI elements throughout the app.
            </p>

            <div className="color-picker-container">
              <HexColorPicker color={tempColor} onChange={setTempColor} />
            </div>

            <div className="color-input-group">
              <label htmlFor="color-input">Hex Color:</label>
              <input
                id="color-input"
                type="text"
                value={tempColor}
                onChange={(e) => setTempColor(e.target.value)}
                placeholder="#0099ff"
                maxLength={7}
                disabled={isSaving}
              />
            </div>

            <div className="color-preview-section">
              <div className="preview-label">Preview Gradient:</div>
              <div className="color-preview-boxes">
                <div
                  className="color-preview-box"
                  style={{ background: previewGradients.light }}
                  title="Light variant"
                />
                <div
                  className="color-preview-box"
                  style={{ background: tempColor }}
                  title="Base color"
                />
                <div
                  className="color-preview-box"
                  style={{ background: previewGradients.dark }}
                  title="Dark variant"
                />
              </div>
            </div>

            <button
              className="settings-reset-btn"
              onClick={handleReset}
              disabled={isSaving}
            >
              <i className="fas fa-undo" /> Reset to Default
            </button>

            {error && (
              <div className="settings-error">
                <i className="fas fa-exclamation-circle" /> {error}
              </div>
            )}
          </div>
        </div>

        <div className="settings-modal-actions">
          <button
            className="settings-btn settings-btn-secondary"
            onClick={handleClose}
            disabled={isSaving}
          >
            Cancel
          </button>
          <button
            className="settings-btn settings-btn-primary"
            onClick={handleSave}
            disabled={isSaving}
          >
            {isSaving ? (
              <>
                <i className="fas fa-spinner fa-spin" /> Saving...
              </>
            ) : (
              'Save Changes'
            )}
          </button>
        </div>
      </div>

      <style jsx>{`
        .settings-modal-backdrop {
          position: fixed;
          top: 0;
          left: 0;
          right: 0;
          bottom: 0;
          background: rgba(15, 23, 42, 0.45);
          z-index: 9998;
          backdrop-filter: blur(4px);
        }

        .settings-modal {
          position: fixed;
          top: 50%;
          left: 50%;
          transform: translate(-50%, -50%);
          background: white;
          border-radius: 12px;
          box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
          z-index: 9999;
          width: 90%;
          max-width: 520px;
          max-height: 90vh;
          overflow: auto;
        }

        .settings-modal-header {
          display: flex;
          justify-content: space-between;
          align-items: center;
          padding: 24px 24px 16px;
          border-bottom: 1px solid #e5e7eb;
        }

        .settings-modal-header h2 {
          margin: 0;
          font-size: 24px;
          font-weight: 600;
          color: #0f172a;
        }

        .settings-modal-close {
          background: none;
          border: none;
          font-size: 32px;
          line-height: 1;
          color: #64748b;
          cursor: pointer;
          padding: 0;
          width: 32px;
          height: 32px;
          display: flex;
          align-items: center;
          justify-content: center;
          border-radius: 4px;
          transition: all 0.2s;
        }

        .settings-modal-close:hover:not(:disabled) {
          background: #f1f5f9;
          color: #0f172a;
        }

        .settings-modal-close:disabled {
          opacity: 0.5;
          cursor: not-allowed;
        }

        .settings-modal-body {
          padding: 24px;
        }

        .settings-section {
          margin-bottom: 0;
        }

        .settings-section h3 {
          margin: 0 0 8px 0;
          font-size: 18px;
          font-weight: 600;
          color: #0f172a;
        }

        .settings-description {
          margin: 0 0 20px 0;
          font-size: 14px;
          color: #64748b;
          line-height: 1.5;
        }

        .color-picker-container {
          margin-bottom: 20px;
          display: flex;
          justify-content: center;
        }

        .color-picker-container :global(.react-colorful) {
          width: 100% !important;
          height: 200px !important;
        }

        .color-input-group {
          display: flex;
          align-items: center;
          gap: 12px;
          margin-bottom: 20px;
        }

        .color-input-group label {
          font-size: 14px;
          font-weight: 500;
          color: #0f172a;
        }

        .color-input-group input {
          flex: 1;
          padding: 8px 12px;
          border: 1px solid #d1d5db;
          border-radius: 6px;
          font-size: 14px;
          font-family: 'Courier New', monospace;
          transition: border-color 0.2s;
        }

        .color-input-group input:focus {
          outline: none;
          border-color: var(--theme-primary, #0099ff);
          box-shadow: 0 0 0 3px rgba(0, 153, 255, 0.1);
        }

        .color-input-group input:disabled {
          background: #f9fafb;
          cursor: not-allowed;
        }

        .color-preview-section {
          margin-bottom: 20px;
        }

        .preview-label {
          font-size: 14px;
          font-weight: 500;
          color: #0f172a;
          margin-bottom: 8px;
        }

        .color-preview-boxes {
          display: flex;
          gap: 12px;
        }

        .color-preview-box {
          flex: 1;
          height: 60px;
          border-radius: 8px;
          border: 2px solid #e5e7eb;
          transition: transform 0.2s;
          cursor: help;
        }

        .color-preview-box:hover {
          transform: scale(1.05);
        }

        .settings-reset-btn {
          width: 100%;
          padding: 10px 16px;
          background: #f1f5f9;
          border: 1px solid #cbd5e1;
          border-radius: 6px;
          font-size: 14px;
          font-weight: 500;
          color: #475569;
          cursor: pointer;
          transition: all 0.2s;
          display: flex;
          align-items: center;
          justify-content: center;
          gap: 8px;
        }

        .settings-reset-btn:hover:not(:disabled) {
          background: #e2e8f0;
          border-color: #94a3b8;
        }

        .settings-reset-btn:disabled {
          opacity: 0.5;
          cursor: not-allowed;
        }

        .settings-error {
          margin-top: 16px;
          padding: 12px;
          background: #fef2f2;
          border: 1px solid #fecaca;
          border-radius: 6px;
          color: #dc2626;
          font-size: 14px;
          display: flex;
          align-items: center;
          gap: 8px;
        }

        .settings-modal-actions {
          display: flex;
          gap: 12px;
          padding: 16px 24px;
          border-top: 1px solid #e5e7eb;
          background: #f9fafb;
          border-radius: 0 0 12px 12px;
        }

        .settings-btn {
          flex: 1;
          padding: 10px 20px;
          border-radius: 6px;
          font-size: 14px;
          font-weight: 500;
          cursor: pointer;
          transition: all 0.2s;
          border: none;
          display: flex;
          align-items: center;
          justify-content: center;
          gap: 8px;
        }

        .settings-btn-secondary {
          background: white;
          border: 1px solid #d1d5db;
          color: #374151;
        }

        .settings-btn-secondary:hover:not(:disabled) {
          background: #f9fafb;
          border-color: #9ca3af;
        }

        .settings-btn-primary {
          background: var(--theme-primary, #0099ff);
          color: white;
          border: 1px solid transparent;
        }

        .settings-btn-primary:hover:not(:disabled) {
          background: var(--theme-dark, #0047ff);
        }

        .settings-btn:disabled {
          opacity: 0.6;
          cursor: not-allowed;
        }
      `}</style>
    </>
  );
}
