import React, { useEffect, useState } from "react";

import ColorPaletteSelector from "./ColorPaletteSelector";
import styles from './AdvancedChartOptions.module.css'; // Import CSS Module
import { useChartBuilder } from "./ChartBuilderContext";
const legendPositions = [
  { value: "top", label: "Top Left" },
  { value: "topCenter", label: "Top Center" },
  { value: "bottom", label: "Bottom Left" },
  { value: "bottomCenter", label: "Bottom Center" },
  { value: "left", label: "Left" },
  { value: "right", label: "Right" },
];

const numberFormatOptions = [
  { value: "none", label: "Raw" },
  { value: "k", label: "Thousands (K)" },
  { value: "m", label: "Millions (M)" },
  { value: "b", label: "Billions (B)" },
  { value: "t", label: "Trillions (T)" },
];

const colorPalettes = [
  { name: "Default", colors: ["#5470C6", "#91CC75", "#EE6666", "#FAC858", "#73C0DE", "#3BA272", "#FC8452", "#9A60B4", "#EA7CCC"] },
  { name: "Superset", colors: ["#1FA8C9", "#FF5A5F", "#FFB400", "#00BFAE", "#A3333D", "#7B615C", "#F7B7A3", "#B2C9AB", "#F6D55C"] },
  { name: "Pastel", colors: ["#A3A1FB", "#FFD6E0", "#B5FFE1", "#FFABAB", "#FFC3A0", "#FF677D", "#D4A5A5", "#392F5A", "#31A2AC"] },
  { name: "Vivid", colors: ["#E4572E", "#29335C", "#F3A712", "#A8C686", "#669BBC", "#2E4057", "#EA5E5E", "#F4D35E", "#EE964B"] },
  { name: "Dark", colors: ["#22223B", "#4A4E69", "#9A8C98", "#C9ADA7", "#F2E9E4", "#3D405B", "#81B29A", "#E07A5F", "#F4F1DE"] },
  // Add more palettes as needed
];

// Utility for formatting numbers for Y axis
function formatYAxisValue(val: number, format: string) {
  if (format === "k") return `${(val / 1e3).toFixed(0)}K`;
  if (format === "m") return `${(val / 1e6).toFixed(0)}M`;
  if (format === "b") return `${(val / 1e9).toFixed(0)}B`;
  if (format === "t") return `${(val / 1e12).toFixed(0)}T`;
  return val.toLocaleString();
}

const AdvancedChartOptions: React.FC = () => {
  const { advancedOptions, setAdvancedOptions, previewOptions, setPreviewOptions, selectedTemplate, runPreviewQuery, chartType } = useChartBuilder();

  // Legend order state for UI (local, then sync to advancedOptions)
  const legendOrder = advancedOptions?.legend?.order || (previewOptions?.legend?.data || []);
  const [localLegendOrder, setLocalLegendOrder] = useState<string[]>(legendOrder);

  // Color picker state
  const [colorPickerOpen, setColorPickerOpen] = useState<number | null>(null);
  const [tempColor, setTempColor] = useState<string>("#000000");

  // Collapsible section state
  const [isColorsExpanded, setIsColorsExpanded] = useState(false);
  const [isSeriesOrderExpanded, setIsSeriesOrderExpanded] = useState(false);

  // Sync localLegendOrder to advancedOptions when changed
  useEffect(() => {
    // Only update if legend order has actually changed from what's stored
    const currentOrder = advancedOptions?.legend?.order;
    const orderChanged = JSON.stringify(currentOrder) !== JSON.stringify(localLegendOrder);

    if (orderChanged) {
      setAdvancedOptions((prev: any) => {
        const next = {
          ...(prev || {}),
          legend: { ...(prev?.legend || {}), order: localLegendOrder },
        };
        // Only update previewOptions if there's already a chart rendered
        // This prevents resetting the chart when switching tabs
        if (previewOptions?.series && previewOptions.series.length > 0) {
          setPreviewOptions((prevPreview: any) => ({
            ...prevPreview,
            legend: { ...(prevPreview?.legend || {}), order: localLegendOrder, data: localLegendOrder },
          }));
        }
        return next;
      });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [localLegendOrder.join(",")]);

  // Move legend item up/down
  const moveLegendItem = (idx: number, dir: -1 | 1) => {
    setLocalLegendOrder((order) => {
      const arr = [...order];
      const swapIdx = idx + dir;
      if (swapIdx < 0 || swapIdx >= arr.length) return arr;
      [arr[idx], arr[swapIdx]] = [arr[swapIdx], arr[idx]];
      return arr;
    });
  };

  // Handle color change for series
  const handleColorChange = (idx: number, newColor: string) => {
    const newPalette = [...palette];
    newPalette[idx] = newColor;
    setAdvancedOptions((prev: any) => ({ ...(prev || {}), color: newPalette }));
    setPreviewOptions((prev: any) => ({ ...(prev || {}), color: newPalette }));
  };

  // Open color picker
  const openColorPicker = (idx: number) => {
    setTempColor(palette[idx] || "#000000");
    setColorPickerOpen(idx);
  };

  // Apply color from picker
  const applyColor = () => {
    if (colorPickerOpen !== null) {
      handleColorChange(colorPickerOpen, tempColor);
      setColorPickerOpen(null);
    }
  };

  // Chart preview will only update on chart load or when user clicks 'Update chart'.

  const palette = advancedOptions?.color || colorPalettes[0].colors;
  const selectedPaletteName = colorPalettes.find(p => JSON.stringify(p.colors) === JSON.stringify(palette))?.name || "Custom";
  const legend = advancedOptions?.legend || { show: true, left: "top" };
    const chartTitle = typeof advancedOptions?.title === "object" && advancedOptions?.title !== null && "text" in advancedOptions.title
      ? advancedOptions.title.text
      : typeof advancedOptions?.title === "string"
        ? advancedOptions.title
        : "";
  const xAxis = advancedOptions?.xAxis || { show: true, name: "" };
  const yAxis = advancedOptions?.yAxis || { show: true, name: "" };
  const tooltip = advancedOptions?.tooltip || { show: true };
  const series = advancedOptions?.series || [];
  // For new unsaved charts, use previewOptions.series as fallback
  const previewSeries = previewOptions?.series || [];
  const activeSeries = series.length > 0 ? series : previewSeries;

  // Chart type checks
  const isShareChart = chartType?.endsWith("_share") || false;
  const isLineOrAreaChart = chartType && (chartType.includes("line") || chartType.includes("area"));
  const isLineOrBar = selectedTemplate && ["line", "bar"].some(t => (selectedTemplate.previewKind || "").includes(t));
  const isLineChart = selectedTemplate && (selectedTemplate.category === "Line" || (selectedTemplate.previewKind || "").includes("line"));
  // Remove ctx.yAxis (not present in context), use yAxisType from yAxis object
  const yAxisType = yAxis?.type || "value";
  const yAxisFormat = previewOptions?.yAxisFormat || "none";
  const stacking = Array.isArray(activeSeries) && activeSeries.some((s: any) => s.stack);
  const smooth = Array.isArray(activeSeries) && activeSeries.some((s: any) => s.smooth);
  const markers = Array.isArray(activeSeries) && activeSeries.some((s: any) => s.showSymbol || (s.symbol && s.symbol !== "none"));

  const handlePaletteChange = (colors: string[]) => {
    setAdvancedOptions((prev: any) => ({ ...(prev || {}), color: colors }));
    setPreviewOptions((prev: any) => ({ ...(prev || {}), color: colors }));
  };
  // Handler functions
  const handleChartTitle = (title: string) => {
    setAdvancedOptions((prev: any) => ({ ...(prev || {}), title: { text: title } }));
    setPreviewOptions((prev: any) => ({ ...(prev || {}), title: { ...(prev?.title || {}), text: title } }));
  };
  const handleAxis = (axis: "x" | "y", key: string, value: any) => {
    setAdvancedOptions((prev: any) => ({
      ...(prev || {}),
      [`${axis}Axis`]: {
        ...((prev && prev[`${axis}Axis`]) || {}),
        [key]: value,
        // Ensure hideOverlap is always preserved in axisLabel
        axisLabel: {
          ...((prev && prev[`${axis}Axis`]?.axisLabel) || {}),
          hideOverlap: true,
        },
      },
    }));
    setPreviewOptions((prev: any) => ({
      ...(prev || {}),
      [`${axis}Axis`]: {
        ...((prev && prev[`${axis}Axis`]) || {}),
        [key]: value,
        // Ensure hideOverlap is always preserved in axisLabel
        axisLabel: {
          ...((prev && prev[`${axis}Axis`]?.axisLabel) || {}),
          hideOverlap: true,
        },
      },
    }));
  };
  const handlePaletteDropdown = (name: string) => {
    const found = colorPalettes.find(p => p.name === name);
    if (found) {
      setAdvancedOptions((prev: any) => ({ ...(prev || {}), color: found.colors }));
      setPreviewOptions((prev: any) => ({ ...(prev || {}), color: found.colors }));
    }
  };
  const handleLegendShow = (show: boolean) => {
    setAdvancedOptions((prev: any) => ({ ...(prev || {}), legend: { ...(prev?.legend || {}), show } }));
    setPreviewOptions((prev: any) => ({ ...(prev || {}), legend: { ...(prev?.legend || {}), show } }));
  };
  const handleLegendPos = (pos: string) => {
    setAdvancedOptions((prev: any) => ({ ...(prev || {}), legend: { ...(prev?.legend || {}), left: pos } }));
    setPreviewOptions((prev: any) => ({ ...(prev || {}), legend: { ...(prev?.legend || {}), left: pos } }));
  };
  const handleTooltip = (show: boolean) => {
    setAdvancedOptions((prev: any) => ({ ...(prev || {}), tooltip: { ...(prev?.tooltip || {}), show } }));
    setPreviewOptions((prev: any) => ({ ...(prev || {}), tooltip: { ...(prev?.tooltip || {}), show } }));
  };
  const handleTooltipOption = (key: string, value: any) => {
    setAdvancedOptions((prev: any) => ({ ...(prev || {}), tooltip: { ...(prev?.tooltip || {}), [key]: value } }));
    setPreviewOptions((prev: any) => ({ ...(prev || {}), tooltip: { ...(prev?.tooltip || {}), [key]: value } }));
  };
  const handleYAxisFormat = (format: string) => {
    const formatter = (val: number) => formatYAxisValue(val, format);
    const labelFormatter = (params: any) => {
      const val = params.value;
      return formatYAxisValue(val, format);
    };

    setAdvancedOptions((prev: any) => ({
      ...(prev || {}),
      yAxisFormat: format,
      yAxis: {
        ...(prev?.yAxis || {}),
        axisLabel: {
          ...(prev?.yAxis?.axisLabel || {}),
          formatter,
          hideOverlap: true,
        }
      },
      // Update marker labels if they are visible
      series: (prev?.series || []).map((s: any) => ({
        ...s,
        label: s.label?.show ? {
          ...(s.label || {}),
          formatter: labelFormatter,
          distance: s.label.distance || 8,
        } : s.label
      }))
    }));
    setPreviewOptions((prev: any) => ({
      ...(prev || {}),
      yAxisFormat: format,
      yAxis: {
        ...(prev?.yAxis || {}),
        axisLabel: {
          ...(prev?.yAxis?.axisLabel || {}),
          formatter,
          hideOverlap: true,
        }
      },
      // Update marker labels if they are visible
      series: (prev?.series || []).map((s: any) => ({
        ...s,
        label: s.label?.show ? {
          ...(s.label || {}),
          formatter: labelFormatter,
          distance: s.label.distance || 8,
        } : s.label
      }))
    }));
  };
  const handleStacking = (enabled: boolean) => {
    setAdvancedOptions((prev: any) => ({
      ...(prev || {}),
      series: (prev?.series || []).map((s: any) => ({ ...s, stack: enabled ? "total" : undefined })),
      // Also update seriesSettings if using new format
      seriesSettings: prev?.seriesSettings ? {
        ...prev.seriesSettings,
        stack: enabled ? "total" : undefined,
      } : undefined,
    }));
    setPreviewOptions((prev: any) => ({
      ...(prev || {}),
      series: (prev?.series || []).map((s: any) => ({ ...s, stack: enabled ? "total" : undefined })),
    }));
  };
  const handleSmooth = (enabled: boolean) => {
    setAdvancedOptions((prev: any) => ({
      ...(prev || {}),
      series: (prev?.series || []).map((s: any) => ({ ...s, smooth: enabled })),
      // Also update seriesSettings if using new format
      seriesSettings: prev?.seriesSettings ? {
        ...prev.seriesSettings,
        smooth: enabled,
      } : undefined,
    }));
    setPreviewOptions((prev: any) => ({
      ...(prev || {}),
      series: (prev?.series || []).map((s: any) => ({ ...s, smooth: enabled })),
    }));
  };
  const handleMarkers = (enabled: boolean) => {
    // Create formatter based on current yAxisFormat
    const createLabelFormatter = (format: string, isShare: boolean) => {
      return (params: any) => {
        const val = params.value;
        // For share charts, always show as percentage
        if (isShare) {
          return `${val.toFixed(1)}%`;
        }
        return formatYAxisValue(val, format);
      };
    };

    const currentFormat = previewOptions?.yAxisFormat || yAxisFormat;

    setAdvancedOptions((prev: any) => ({
      ...(prev || {}),
      labelLayout: enabled ? {
        hideOverlap: true,
      } : undefined,
      series: (prev?.series || []).map((s: any) => {
        // Capture series data length in closure for showSymbol function
        const seriesDataLength = s.data?.length || 0;
        return {
          ...s,
          // Show exactly 6 points: first, last, and 4 evenly spaced in between
          symbol: enabled ? 'circle' : "none",
          symbolSize: enabled ? 6 : 4,
          showSymbol: enabled ? function (dataIndex: number) {
            const dataLength = seriesDataLength;

            // Always show first and last
            if (dataIndex === 0 || dataIndex === dataLength - 1) {
              return true;
            }

            // Calculate 4 evenly spaced middle points
            const point1 = Math.round((dataLength - 1) * 1 / 5);
            const point2 = Math.round((dataLength - 1) * 2 / 5);
            const point3 = Math.round((dataLength - 1) * 3 / 5);
            const point4 = Math.round((dataLength - 1) * 4 / 5);

            if (dataIndex === point1 || dataIndex === point2 || dataIndex === point3 || dataIndex === point4) {
              return true;
            }

            return false;
          } : false,
          label: {
            ...(s.label || {}),
            show: enabled,
            position: 'top',
            formatter: enabled ? createLabelFormatter(currentFormat, isShareChart) : '{c}',
            fontSize: 10,
            color: 'rgba(51, 65, 85, 0.85)',
            backgroundColor: 'rgba(255, 255, 255, 0.8)',
            padding: [2, 4],
            borderRadius: 3,
          },
        };
      }),
      // Also update seriesSettings if using new format
      seriesSettings: prev?.seriesSettings ? {
        ...prev.seriesSettings,
        symbol: enabled ? 'circle' : 'none',
        symbolSize: enabled ? 6 : 4,
      } : undefined,
    }));
    setPreviewOptions((prev: any) => ({
      ...(prev || {}),
      labelLayout: enabled ? {
        hideOverlap: true,
      } : undefined,
      series: (prev?.series || []).map((s: any) => {
        // Capture series data length in closure for showSymbol function
        const seriesDataLength = s.data?.length || 0;
        return {
          ...s,
          // Show exactly 6 points: first, last, and 4 evenly spaced in between
          symbol: enabled ? 'circle' : "none",
          symbolSize: enabled ? 6 : 4,
          showSymbol: enabled ? function (dataIndex: number) {
            const dataLength = seriesDataLength;

            // Always show first and last
            if (dataIndex === 0 || dataIndex === dataLength - 1) {
              return true;
            }

            // Calculate 4 evenly spaced middle points
            const point1 = Math.round((dataLength - 1) * 1 / 5);
            const point2 = Math.round((dataLength - 1) * 2 / 5);
            const point3 = Math.round((dataLength - 1) * 3 / 5);
            const point4 = Math.round((dataLength - 1) * 4 / 5);

            if (dataIndex === point1 || dataIndex === point2 || dataIndex === point3 || dataIndex === point4) {
              return true;
            }

            return false;
          } : false,
          label: {
            ...(s.label || {}),
            show: enabled,
            position: 'top',
            formatter: enabled ? createLabelFormatter(currentFormat, isShareChart) : '{c}',
            fontSize: 10,
            color: 'rgba(51, 65, 85, 0.85)',
            backgroundColor: 'rgba(255, 255, 255, 0.8)',
            padding: [2, 4],
            borderRadius: 3,
          },
        };
      }),
    }));
  };

  return (
    <div style={{ padding: '0 4px' }}>
      <div className="chart-builder-panel-title" style={{ marginBottom: 20 }}>Advanced Editor</div>

      {/* TITLE & STYLING SECTION */}
      <div style={{ marginBottom: 24 }}>
        <div style={{ fontSize: 13, fontWeight: 600, color: '#475569', marginBottom: 12, paddingBottom: 6, borderBottom: '1px solid #e2e8f0' }}>
          Title & Styling
        </div>
        <div className="chart-builder-field-group">
          <label className="chart-builder-label" htmlFor="chart-title-input">Chart Title</label>
          <input
            id="chart-title-input"
            type="text"
            value={chartTitle}
            onChange={e => handleChartTitle(e.target.value)}
            placeholder="Enter chart title"
            className="chart-builder-input"
          />
        </div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12, marginTop: 8 }}>
          <div>
            <label className="chart-builder-label" htmlFor="title-font">Font</label>
            <select
              id="title-font"
              className="chart-builder-select"
              value={previewOptions?.titleFont || "sans-serif"}
              onChange={e => {
                setAdvancedOptions((prev: any) => ({ ...(prev || {}), titleFont: e.target.value }));
                setPreviewOptions((prev: any) => ({ ...(prev || {}), titleFont: e.target.value }));
              }}
            >
              <option value="sans-serif">Sans-serif</option>
              <option value="serif">Serif</option>
              <option value="monospace">Monospace</option>
              <option value="Roboto">Roboto</option>
              <option value="Arial">Arial</option>
              <option value="Georgia">Georgia</option>
            </select>
          </div>
          <div>
            <label className="chart-builder-label" htmlFor="title-size">Size</label>
            <select
              id="title-size"
              className="chart-builder-select"
              value={previewOptions?.titleSize || "20"}
              onChange={e => {
                setAdvancedOptions((prev: any) => ({ ...(prev || {}), titleSize: e.target.value }));
                setPreviewOptions((prev: any) => ({ ...(prev || {}), titleSize: e.target.value }));
              }}
            >
              <option value="16">16px</option>
              <option value="18">18px</option>
              <option value="20">20px</option>
              <option value="24">24px</option>
              <option value="28">28px</option>
              <option value="32">32px</option>
            </select>
          </div>
        </div>
      </div>

      {/* AXES SECTION */}
      <div style={{ marginBottom: 24 }}>
        <div style={{ fontSize: 13, fontWeight: 600, color: '#475569', marginBottom: 12, paddingBottom: 6, borderBottom: '1px solid #e2e8f0' }}>
          Axes
        </div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12 }}>
          <div>
            <label className="chart-builder-label" htmlFor="x-axis-title">X Axis Title</label>
            <input
              id="x-axis-title"
              type="text"
              value={xAxis.name || ""}
              onChange={e => handleAxis("x", "name", e.target.value)}
              className="chart-builder-input"
              placeholder="X Axis"
            />
            <label style={{ display: 'flex', alignItems: 'center', marginTop: 6, fontSize: 13, color: '#334155' }}>
              <input
                type="checkbox"
                checked={xAxis.show !== false}
                onChange={e => handleAxis("x", "show", e.target.checked)}
                style={{ marginRight: 6 }}
              />
              Show axis
            </label>
          </div>
          <div>
            <label className="chart-builder-label" htmlFor="y-axis-title">Y Axis Title</label>
            <input
              id="y-axis-title"
              type="text"
              value={yAxis.name || ""}
              onChange={e => handleAxis("y", "name", e.target.value)}
              className="chart-builder-input"
              placeholder="Y Axis"
            />
            <label style={{ display: 'flex', alignItems: 'center', marginTop: 6, fontSize: 13, color: '#334155' }}>
              <input
                type="checkbox"
                checked={yAxis.show !== false}
                onChange={e => handleAxis("y", "show", e.target.checked)}
                style={{ marginRight: 6 }}
              />
              Show axis
            </label>
          </div>
        </div>

        {isLineChart && yAxisType === "value" && (
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12, marginTop: 12 }}>
            <div>
              <label className="chart-builder-label" htmlFor="x-axis-date-format">X Axis Date Format</label>
              <select
                id="x-axis-date-format"
                value={previewOptions?.xAxisDateFormat || "auto"}
                onChange={e => {
                  setAdvancedOptions((prev: any) => ({ ...(prev || {}), xAxisDateFormat: e.target.value }));
                  setPreviewOptions((prev: any) => ({ ...(prev || {}), xAxisDateFormat: e.target.value }));
                }}
                className="chart-builder-select"
              >
                <option value="auto">Auto</option>
                <option value="MM/DD/YYYY">MM/DD/YYYY</option>
                <option value="YYYY-MM-DD">YYYY-MM-DD</option>
                <option value="MMM YYYY">Jan 2025</option>
                <option value="MMM D, YYYY">Jan 1, 2025</option>
                <option value="YYYY">Year (2025)</option>
              </select>
            </div>
            <div>
              <label className="chart-builder-label" htmlFor="y-axis-format">
                Y Axis Number Format
              </label>
              <select
                id="y-axis-format"
                value={isShareChart ? "none" : yAxisFormat}
                onChange={e => handleYAxisFormat(e.target.value)}
                className="chart-builder-select"
                disabled={isShareChart}
                style={isShareChart ? {
                  backgroundColor: '#f1f5f9',
                  color: '#94a3b8',
                  cursor: 'not-allowed',
                  opacity: 0.6
                } : {}}
              >
                {numberFormatOptions.map(opt => (
                  <option key={opt.value} value={opt.value}>{opt.label}</option>
                ))}
              </select>
            </div>
          </div>
        )}
      </div>

      {/* LEGEND SECTION */}
      <div style={{ marginBottom: 24 }}>
        <div style={{ fontSize: 13, fontWeight: 600, color: '#475569', marginBottom: 12, paddingBottom: 6, borderBottom: '1px solid #e2e8f0' }}>
          Legend
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 12 }}>
          <label style={{ display: 'flex', alignItems: 'center', fontSize: 13, color: '#334155' }}>
            <input
              type="checkbox"
              checked={legend.show !== false}
              onChange={e => handleLegendShow(e.target.checked)}
              style={{ marginRight: 6 }}
            />
            Show legend
          </label>
          {legend.show !== false && (
            <>
              <span style={{ color: '#cbd5e1', marginLeft: 8, marginRight: 8 }}>|</span>
              <label className="chart-builder-label" htmlFor="legend-position" style={{ margin: 0, fontSize: 13 }}>Position:</label>
              <select
                id="legend-position"
                value={legend.left || "top"}
                onChange={e => handleLegendPos(e.target.value)}
                className="chart-builder-select"
                style={{ flex: 1, maxWidth: 160 }}
              >
                {legendPositions.map(pos => <option key={pos.value} value={pos.value}>{pos.label}</option>)}
              </select>
            </>
          )}
        </div>
      </div>

      {/* COLORS SECTION */}
      <div style={{ marginBottom: 24 }}>
        <div
          style={{
            fontSize: 13,
            fontWeight: 600,
            color: '#475569',
            marginBottom: isColorsExpanded ? 12 : 0,
            paddingBottom: 6,
            borderBottom: '1px solid #e2e8f0',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            cursor: 'pointer',
            userSelect: 'none'
          }}
          onClick={() => setIsColorsExpanded(!isColorsExpanded)}
        >
          <span>Colors</span>
          <i
            className={isColorsExpanded ? "fas fa-chevron-up" : "fas fa-chevron-down"}
            style={{ fontSize: 11, color: '#94a3b8', transition: 'transform 0.2s' }}
          />
        </div>
        {isColorsExpanded && (
          <div className="chart-builder-field-group" style={{ marginTop: 12 }}>
            <label className="chart-builder-label" htmlFor="palette-select">Color Palette</label>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
              <select
                id="palette-select"
                value={selectedPaletteName}
                onChange={e => handlePaletteDropdown(e.target.value)}
                className="chart-builder-select"
                style={{ flex: 1, maxWidth: 160 }}
              >
                {colorPalettes.map(p => (
                  <option key={p.name} value={p.name}>{p.name}</option>
                ))}
                <option value="Custom">Custom</option>
              </select>
              <div style={{ display: 'flex', gap: 4 }}>
                {(colorPalettes.find(p => p.name === selectedPaletteName)?.colors || palette).map((c: string, i: number) => (
                  <span
                    key={i}
                    style={{
                      width: 20,
                      height: 20,
                      borderRadius: 4,
                      border: '2px solid #fff',
                      boxShadow: '0 0 0 1px #e2e8f0',
                      background: c,
                      display: 'inline-block'
                    }}
                  />
                ))}
              </div>
            </div>
            {selectedPaletteName === "Custom" && (
              <ColorPaletteSelector
                value={palette}
                onChange={colors => {
                  setAdvancedOptions((prev: any) => ({ ...(prev || {}), color: colors }));
                  setPreviewOptions((prev: any) => ({ ...(prev || {}), color: colors }));
                }}
              />
            )}
          </div>
        )}
      </div>

      {/* SERIES ORDER SECTION */}
      {Array.isArray(localLegendOrder) && localLegendOrder.length > 1 && (
        <div style={{ marginBottom: 24 }}>
          <div
            style={{
              fontSize: 13,
              fontWeight: 600,
              color: '#475569',
              marginBottom: isSeriesOrderExpanded ? 12 : 0,
              paddingBottom: 6,
              borderBottom: '1px solid #e2e8f0',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              cursor: 'pointer',
              userSelect: 'none'
            }}
            onClick={() => setIsSeriesOrderExpanded(!isSeriesOrderExpanded)}
          >
            <span>Series Order</span>
            <i
              className={isSeriesOrderExpanded ? "fas fa-chevron-up" : "fas fa-chevron-down"}
              style={{ fontSize: 11, color: '#94a3b8', transition: 'transform 0.2s' }}
            />
          </div>
          {isSeriesOrderExpanded && (
            <div style={{ background: '#f8fafc', border: '1px solid #e2e8f0', borderRadius: 6, padding: 8, marginTop: 12 }}>
              {localLegendOrder.map((item, idx) => (
                <div key={item} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '6px 8px', background: '#fff', border: '1px solid #e2e8f0', borderRadius: 4, marginBottom: idx < localLegendOrder.length - 1 ? 6 : 0, fontSize: 13 }}>
                  <button type="button" onClick={() => openColorPicker(idx)} title="Click to change color" style={{ display: 'inline-block', width: 24, height: 24, borderRadius: 4, background: palette[idx % palette.length], border: '2px solid #fff', boxShadow: '0 0 0 1px #e2e8f0', cursor: 'pointer', padding: 0, transition: 'transform 0.1s' }} onMouseOver={(e) => e.currentTarget.style.transform = 'scale(1.1)'} onMouseOut={(e) => e.currentTarget.style.transform = 'scale(1)'} />
                  <span style={{ flex: 1, color: '#334155', fontWeight: 500 }}>{item}</span>
                  <button type="button" onClick={() => moveLegendItem(idx, -1)} disabled={idx === 0} style={{ padding: '4px 8px', fontSize: 12, border: '1px solid #e2e8f0', borderRadius: 4, background: idx === 0 ? '#f1f5f9' : '#fff', color: idx === 0 ? '#cbd5e1' : '#475569', cursor: idx === 0 ? 'not-allowed' : 'pointer', fontWeight: 600 }}>↑</button>
                  <button type="button" onClick={() => moveLegendItem(idx, 1)} disabled={idx === localLegendOrder.length - 1} style={{ padding: '4px 8px', fontSize: 12, border: '1px solid #e2e8f0', borderRadius: 4, background: idx === localLegendOrder.length - 1 ? '#f1f5f9' : '#fff', color: idx === localLegendOrder.length - 1 ? '#cbd5e1' : '#475569', cursor: idx === localLegendOrder.length - 1 ? 'not-allowed' : 'pointer', fontWeight: 600 }}>↓</button>
                </div>
              ))}
              <div style={{ fontSize: 11, color: '#64748b', marginTop: 6, fontStyle: 'italic' }}>Drag items to reorder • Changes are saved with the chart</div>
            </div>
          )}
        </div>
      )}

      {/* SERIES OPTIONS SECTION */}
      {(isLineOrBar || isLineOrAreaChart) && (
        <div style={{ marginBottom: 24 }}>
          <div style={{ fontSize: 13, fontWeight: 600, color: '#475569', marginBottom: 12, paddingBottom: 6, borderBottom: '1px solid #e2e8f0' }}>
            Series Options
          </div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 16 }}>
            {!isShareChart && (
              <label style={{ display: 'flex', alignItems: 'center', fontSize: 13, color: '#334155' }}>
                <input
                  type="checkbox"
                  checked={stacking}
                  onChange={e => handleStacking(e.target.checked)}
                  style={{ marginRight: 6 }}
                />
                Stack series
              </label>
            )}
            {isShareChart && (
              <label style={{ display: 'flex', alignItems: 'center', fontSize: 13, color: '#334155' }}>
                <input
                  type="checkbox"
                  checked={advancedOptions?.showAsPercentage !== false}
                  onChange={e => {
                    const isStacked = e.target.checked;
                    setAdvancedOptions((prev: any) => ({
                      ...(prev || {}),
                      showAsPercentage: isStacked,
                    }));
                    // Also update preview to show stacking change immediately
                    setPreviewOptions((prev: any) => ({
                      ...(prev || {}),
                      series: (prev?.series || []).map((s: any) => ({
                        ...s,
                        stack: isStacked ? "total" : undefined
                      })),
                    }));
                  }}
                  style={{ marginRight: 6 }}
                />
                Stacked
              </label>
            )}
            {isLineOrAreaChart && (
              <label style={{ display: 'flex', alignItems: 'center', fontSize: 13, color: '#334155' }}>
                <input
                  type="checkbox"
                  checked={smooth}
                  onChange={e => handleSmooth(e.target.checked)}
                  style={{ marginRight: 6 }}
                />
                Smooth lines
              </label>
            )}
            {isLineOrAreaChart && (
              <label style={{ display: 'flex', alignItems: 'center', fontSize: 13, color: '#334155' }}>
                <input
                  type="checkbox"
                  checked={markers}
                  onChange={e => handleMarkers(e.target.checked)}
                  style={{ marginRight: 6 }}
                />
                Show markers
              </label>
            )}
          </div>
        </div>
      )}

      {/* TOOLTIPS SECTION */}
      <div style={{ marginBottom: 24 }}>
        <div style={{ fontSize: 13, fontWeight: 600, color: '#475569', marginBottom: 12, paddingBottom: 6, borderBottom: '1px solid #e2e8f0' }}>
          Tooltips
        </div>
        <label style={{ display: 'flex', alignItems: 'center', fontSize: 13, color: '#334155', marginBottom: 12 }}>
          <input
            type="checkbox"
            checked={tooltip.show !== false}
            onChange={e => handleTooltip(e.target.checked)}
            style={{ marginRight: 6 }}
          />
          Enable tooltips
        </label>

        {tooltip.show !== false && (
          <div style={{
            background: '#f8fafc',
            border: '1px solid #e2e8f0',
            borderRadius: 6,
            padding: 12
          }}>
            <div style={{ display: 'grid', gridTemplateColumns: isShareChart ? '1fr' : '1fr 1fr', gap: 12 }}>
              <div>
                <label className="chart-builder-label" htmlFor="tooltip-date-format">Date Format</label>
                <select
                  id="tooltip-date-format"
                  value={tooltip.dateFormat || "auto"}
                  onChange={e => handleTooltipOption("dateFormat", e.target.value)}
                  className="chart-builder-select"
                >
                  <option value="auto">Auto</option>
                  <option value="MM/DD/YYYY">MM/DD/YYYY</option>
                  <option value="YYYY-MM-DD">YYYY-MM-DD</option>
                  <option value="MMM YYYY">Jan 2025</option>
                  <option value="MMM D, YYYY">Jan 1, 2025</option>
                  <option value="YYYY">Year (2025)</option>
                </select>
              </div>
              {!isShareChart && (
                <div>
                  <label className="chart-builder-label" htmlFor="tooltip-number-format">Number Format</label>
                  <select
                    id="tooltip-number-format"
                    value={tooltip.numberFormat || "none"}
                    onChange={e => handleTooltipOption("numberFormat", e.target.value)}
                    className="chart-builder-select"
                  >
                    <option value="none">Raw</option>
                    <option value="k">Thousands (K)</option>
                    <option value="m">Millions (M)</option>
                    <option value="b">Billions (B)</option>
                    <option value="t">Trillions (T)</option>
                  </select>
                </div>
              )}
            </div>

            {/* Decimal Places - always shown for all chart types */}
            <div style={{ display: 'grid', gridTemplateColumns: isShareChart ? '1fr' : '1fr 1fr', gap: 12, marginTop: 12 }}>
              <div>
                <label className="chart-builder-label" htmlFor="tooltip-decimal-places">Decimal Places</label>
                <select
                  id="tooltip-decimal-places"
                  value={tooltip.decimalPlaces ?? 2}
                  onChange={e => handleTooltipOption("decimalPlaces", parseInt(e.target.value))}
                  className="chart-builder-select"
                >
                  <option value="0">0</option>
                  <option value="1">1</option>
                  <option value="2">2</option>
                  <option value="3">3</option>
                  <option value="4">4</option>
                </select>
              </div>

              {/* Use Commas - only for non-share charts */}
              {!isShareChart && (
                <div style={{ display: 'flex', alignItems: 'flex-end' }}>
                  <label style={{ display: 'flex', alignItems: 'center', fontSize: 13, color: '#334155', paddingBottom: 8 }}>
                    <input
                      type="checkbox"
                      checked={tooltip.useCommas ?? true}
                      onChange={e => handleTooltipOption("useCommas", e.target.checked)}
                      style={{ marginRight: 6 }}
                    />
                    Use commas
                  </label>
                </div>
              )}
            </div>
          </div>
        )}
      </div>

      {/* Color Picker Modal */}
      {colorPickerOpen !== null && (
        <div
          style={{
            position: 'fixed',
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
            background: 'rgba(0, 0, 0, 0.5)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            zIndex: 9999,
          }}
          onClick={() => setColorPickerOpen(null)}
        >
          <div
            style={{
              background: '#fff',
              borderRadius: 12,
              padding: 24,
              boxShadow: '0 20px 25px -5px rgba(0, 0, 0, 0.1), 0 10px 10px -5px rgba(0, 0, 0, 0.04)',
              minWidth: 320,
              maxWidth: 400,
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <div style={{ marginBottom: 16 }}>
              <h3 style={{ margin: 0, fontSize: 16, fontWeight: 600, color: '#1e293b', marginBottom: 4 }}>
                Choose Color
              </h3>
              <p style={{ margin: 0, fontSize: 13, color: '#64748b' }}>
                Series: {localLegendOrder[colorPickerOpen]}
              </p>
            </div>

            <div style={{ marginBottom: 16 }}>
              <label className="chart-builder-label" style={{ marginBottom: 8, display: 'block' }}>
                Color Picker
              </label>
              <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
                <input
                  type="color"
                  value={tempColor}
                  onChange={(e) => setTempColor(e.target.value)}
                  style={{
                    width: 60,
                    height: 40,
                    border: '2px solid #e2e8f0',
                    borderRadius: 6,
                    cursor: 'pointer',
                  }}
                />
                <div style={{ flex: 1 }}>
                  <div
                    style={{
                      width: '100%',
                      height: 40,
                      borderRadius: 6,
                      background: tempColor,
                      border: '2px solid #e2e8f0',
                    }}
                  />
                </div>
              </div>
            </div>

            <div style={{ marginBottom: 20 }}>
              <label className="chart-builder-label" htmlFor="hex-input" style={{ marginBottom: 8, display: 'block' }}>
                Hex Code
              </label>
              <input
                id="hex-input"
                type="text"
                value={tempColor}
                onChange={(e) => {
                  const val = e.target.value;
                  if (/^#[0-9A-Fa-f]{0,6}$/.test(val)) {
                    setTempColor(val);
                  }
                }}
                placeholder="#000000"
                className="chart-builder-input"
                style={{ fontFamily: 'monospace', textTransform: 'uppercase' }}
              />
              <div style={{ fontSize: 11, color: '#64748b', marginTop: 4 }}>
                Or enter RGB: rgb(255, 0, 0)
              </div>
            </div>

            <div style={{ marginBottom: 16 }}>
              <label className="chart-builder-label" style={{ marginBottom: 8, display: 'block' }}>
                Quick Colors
              </label>
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(8, 1fr)', gap: 6 }}>
                {['#EF4444', '#F59E0B', '#10B981', '#3B82F6', '#8B5CF6', '#EC4899', '#6366F1', '#14B8A6'].map(color => (
                  <button
                    key={color}
                    type="button"
                    onClick={() => setTempColor(color)}
                    style={{
                      width: '100%',
                      height: 32,
                      background: color,
                      border: tempColor === color ? '3px solid #1e293b' : '2px solid #e2e8f0',
                      borderRadius: 6,
                      cursor: 'pointer',
                      transition: 'transform 0.1s',
                    }}
                    onMouseOver={(e) => e.currentTarget.style.transform = 'scale(1.1)'}
                    onMouseOut={(e) => e.currentTarget.style.transform = 'scale(1)'}
                  />
                ))}
              </div>
            </div>

            <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
              <button
                type="button"
                onClick={() => setColorPickerOpen(null)}
                style={{
                  padding: '8px 16px',
                  border: '1px solid #e2e8f0',
                  borderRadius: 6,
                  background: '#fff',
                  color: '#475569',
                  cursor: 'pointer',
                  fontSize: 14,
                  fontWeight: 500,
                }}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={applyColor}
                style={{
                  padding: '8px 16px',
                  border: 'none',
                  borderRadius: 6,
                  background: '#3B82F6',
                  color: '#fff',
                  cursor: 'pointer',
                  fontSize: 14,
                  fontWeight: 500,
                }}
              >
                Apply
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

export default AdvancedChartOptions;