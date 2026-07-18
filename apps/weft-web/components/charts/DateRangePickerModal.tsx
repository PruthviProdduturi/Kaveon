import React, { useState, useEffect } from "react";
import { TimeRangePreset } from "./ChartBuilderContext";

interface DateRangePickerModalProps {
  isOpen: boolean;
  onClose: () => void;
  onApply: (
    rangeType: TimeRangePreset,
    startDate: string | null,
    endDate: string | null
  ) => void;
  initialRangeType?: TimeRangePreset;
  initialStartDate?: string | null;
  initialEndDate?: string | null;
}

export const DateRangePickerModal: React.FC<DateRangePickerModalProps> = ({
  isOpen,
  onClose,
  onApply,
  initialRangeType = "custom",
  initialStartDate = null,
  initialEndDate = null,
}) => {
  const [rangeType, setRangeType] = useState<TimeRangePreset>(initialRangeType);
  const [startDate, setStartDate] = useState<string>(initialStartDate || "");
  const [endDate, setEndDate] = useState<string>(initialEndDate || "");

  useEffect(() => {
    if (isOpen) {
      setRangeType(initialRangeType);
      setStartDate(initialStartDate || "");
      setEndDate(initialEndDate || "");
    }
  }, [isOpen, initialRangeType, initialStartDate, initialEndDate]);

  const handleApply = () => {
    if (rangeType === "custom" && (!startDate || !endDate)) {
      alert("Please select both start and end dates for custom range");
      return;
    }
    if (rangeType === "custom_to_latest" && !startDate) {
      alert("Please select a start date");
      return;
    }

    onApply(
      rangeType,
      startDate || null,
      rangeType === "custom_to_latest" ? null : endDate || null
    );
    onClose();
  };

  if (!isOpen) return null;

  return (
    <div
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        backgroundColor: "rgba(0, 0, 0, 0.5)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
      }}
      onClick={onClose}
    >
      <div
        style={{
          backgroundColor: "white",
          borderRadius: "8px",
          padding: "24px",
          minWidth: "400px",
          maxWidth: "500px",
          boxShadow: "0 4px 6px rgba(0, 0, 0, 0.1)",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div style={{ marginBottom: "20px" }}>
          <h3 style={{ margin: "0 0 16px 0", fontSize: "18px", fontWeight: "600" }}>
            Custom Date Range
          </h3>
          <p style={{ margin: 0, fontSize: "14px", color: "#666" }}>
            Select a custom date range for your chart
          </p>
        </div>

        <div style={{ marginBottom: "20px" }}>
          <label style={{ display: "block", marginBottom: "8px", fontWeight: "500" }}>
            Range Type
          </label>
          <select
            value={rangeType}
            onChange={(e) => setRangeType(e.target.value as TimeRangePreset)}
            style={{
              width: "100%",
              padding: "8px 12px",
              border: "1px solid #ddd",
              borderRadius: "4px",
              fontSize: "14px",
            }}
          >
            <option value="custom">Custom Range (Start Date to End Date)</option>
            <option value="custom_to_latest">
              Custom to Latest (Start Date to Latest Available)
            </option>
          </select>
        </div>

        <div style={{ marginBottom: "20px" }}>
          <label
            htmlFor="startDate"
            style={{ display: "block", marginBottom: "8px", fontWeight: "500" }}
          >
            Start Date
          </label>
          <input
            id="startDate"
            type="date"
            value={startDate}
            onChange={(e) => setStartDate(e.target.value)}
            style={{
              width: "100%",
              padding: "8px 12px",
              border: "1px solid #ddd",
              borderRadius: "4px",
              fontSize: "14px",
            }}
          />
        </div>

        {rangeType === "custom" && (
          <div style={{ marginBottom: "20px" }}>
            <label
              htmlFor="endDate"
              style={{ display: "block", marginBottom: "8px", fontWeight: "500" }}
            >
              End Date
            </label>
            <input
              id="endDate"
              type="date"
              value={endDate}
              onChange={(e) => setEndDate(e.target.value)}
              style={{
                width: "100%",
                padding: "8px 12px",
                border: "1px solid #ddd",
                borderRadius: "4px",
                fontSize: "14px",
              }}
            />
          </div>
        )}

        {rangeType === "custom_to_latest" && (
          <div
            style={{
              marginBottom: "20px",
              padding: "12px",
              backgroundColor: "#f0f9ff",
              border: "1px solid #bfdbfe",
              borderRadius: "4px",
            }}
          >
            <p style={{ margin: 0, fontSize: "13px", color: "#1e40af" }}>
              <i className="fas fa-info-circle" style={{ marginRight: "6px" }}></i>
              End date will be set to the latest available date in your dataset
            </p>
          </div>
        )}

        <div style={{ display: "flex", gap: "12px", justifyContent: "flex-end" }}>
          <button
            onClick={onClose}
            style={{
              padding: "8px 16px",
              border: "1px solid #ddd",
              borderRadius: "4px",
              backgroundColor: "white",
              cursor: "pointer",
              fontSize: "14px",
            }}
          >
            Cancel
          </button>
          <button
            onClick={handleApply}
            style={{
              padding: "8px 16px",
              border: "none",
              borderRadius: "4px",
              backgroundColor: "#2563eb",
              color: "white",
              cursor: "pointer",
              fontSize: "14px",
              fontWeight: "500",
            }}
          >
            Apply
          </button>
        </div>
      </div>
    </div>
  );
};
