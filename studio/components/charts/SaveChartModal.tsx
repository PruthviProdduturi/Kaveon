import React from "react";
import { useChartBuilder } from "./ChartBuilderContext";

interface SaveChartModalProps {
  isOpen: boolean;
  onClose: () => void;
}

const SaveChartModal: React.FC<SaveChartModalProps> = ({ isOpen, onClose }) => {
  const { name, setName, description, setDescription, canSave, isSaving, saveError, handleSave } =
    useChartBuilder();

  if (!isOpen) return null;

  const canConfirm = Boolean(name.trim()) && canSave;

  const onConfirm = async () => {
    if (!canConfirm) return;
    await handleSave();
    onClose();
  };

  return (
    <div className="chart-save-modal-backdrop">
      <div className="chart-save-modal">
        <div className="chart-builder-panel-title">Save chart</div>
        <div className="chart-builder-field-group">
          <label className="chart-builder-label" htmlFor="save-chart-name">
            Name
          </label>
          <input
            id="save-chart-name"
            className="chart-builder-input"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <div className="chart-builder-field-group">
          <label className="chart-builder-label" htmlFor="save-chart-description">
            Description (optional)
          </label>
          <textarea
            id="save-chart-description"
            className="chart-builder-textarea"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </div>
        {saveError && (
          <div className="muted" style={{ marginBottom: 8, color: "#b91c1c" }}>
            {saveError}
          </div>
        )}
        <div className="chart-save-modal-actions">
          <button
            type="button"
            className="chart-save-modal-btn chart-save-modal-btn-secondary"
            onClick={onClose}
          >
            Cancel
          </button>
          <button
            type="button"
            className="chart-save-modal-btn chart-save-modal-btn-primary"
            onClick={onConfirm}
            disabled={!canConfirm || isSaving}
          >
            {isSaving ? "Saving..." : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default SaveChartModal;
