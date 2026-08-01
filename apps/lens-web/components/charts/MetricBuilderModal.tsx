import React from "react";

interface MetricBuilderModalProps {
  isOpen: boolean;
  onClose: () => void;
}

const MetricBuilderModal: React.FC<MetricBuilderModalProps> = ({ isOpen, onClose }) => {
  if (!isOpen) return null;

  return (
    <div className="card" style={{ marginTop: 12 }}>
      <div className="chart-builder-panel-title">Metric builder</div>
      <div className="muted" style={{ fontSize: 13, marginBottom: 8 }}>
        Define custom metrics (SUM, AVG, COUNT, custom expressions) here.
      </div>
      <button type="button" className="overview-primary-btn" onClick={onClose}>
        Close
      </button>
    </div>
  );
};

export default MetricBuilderModal;
