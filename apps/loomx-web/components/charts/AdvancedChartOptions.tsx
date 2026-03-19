import React, { useEffect, useState } from "react";

import ColorPaletteSelector from "./ColorPaletteSelector";
import styles from './AdvancedChartOptions.module.css'; // Import CSS Module
import { useChartBuilder } from "./ChartBuilderContext";
import { getPlugin } from "./chartPluginRegistry";
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

// ─── Chart-type-specific options ─────────────────────────────────────────────

interface ChartTypeOptionsProps {
  chartType: string;
  advancedOptions: any;
  setAdvancedOptions: (updater: ((prev: any) => any) | any) => void;
  previewOptions: any;
  setPreviewOptions: (updater: ((prev: any) => any) | any) => void;
}

const SectionHeader: React.FC<{ title: string }> = ({ title }) => (
  <div style={{ fontSize: 13, fontWeight: 600, color: "#475569", marginBottom: 12, paddingBottom: 6, borderBottom: "1px solid #e2e8f0" }}>
    {title}
  </div>
);

const Row2: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12, marginBottom: 12 }}>{children}</div>
);

const CheckRow: React.FC<{ id: string; label: string; checked: boolean; onChange: (v: boolean) => void }> = ({ id, label, checked, onChange }) => (
  <label htmlFor={id} style={{ display: "flex", alignItems: "center", fontSize: 13, color: "#334155", marginBottom: 10, cursor: "pointer" }}>
    <input id={id} type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} style={{ marginRight: 6 }} />
    {label}
  </label>
);

const CollapsibleSection: React.FC<{
  title: string;
  expanded: boolean;
  onToggle: () => void;
  children: React.ReactNode;
}> = ({ title, expanded, onToggle, children }) => (
  <div style={{ marginBottom: 20 }}>
    <div
      style={{
        fontSize: 13,
        fontWeight: 600,
        color: '#475569',
        marginBottom: expanded ? 12 : 0,
        paddingBottom: 6,
        borderBottom: '1px solid #e2e8f0',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        cursor: 'pointer',
        userSelect: 'none' as const,
      }}
      onClick={onToggle}
    >
      <span>{title}</span>
      <i
        className={expanded ? "fas fa-chevron-up" : "fas fa-chevron-down"}
        style={{ fontSize: 11, color: '#94a3b8' }}
      />
    </div>
    {expanded && <div>{children}</div>}
  </div>
);

const ChartTypeOptions: React.FC<ChartTypeOptionsProps> = ({ chartType, advancedOptions, setAdvancedOptions, previewOptions, setPreviewOptions }) => {
  const ctOpts = advancedOptions?.chartTypeOptions || {};

  const set = (key: string, value: any) => {
    setAdvancedOptions((prev: any) => ({
      ...(prev || {}),
      chartTypeOptions: { ...(prev?.chartTypeOptions || {}), [key]: value },
    }));
    // Immediately apply to series in previewOptions so the chart updates live
    applyLive(key, value);
  };

  const applyLive = (key: string, value: any) => {
    if (!previewOptions) return;
    setPreviewOptions((prev: any) => {
      if (!prev) return prev;
      if (!Array.isArray(prev.series)) return prev;

      const isBar = ["bar_vertical", "bar_horizontal", "stacked_bar_vertical", "stacked_bar_horizontal", "grouped_bar"].includes(chartType);
      const isPie = chartType === "pie" || chartType === "donut";
      const isScatter = chartType === "scatter" || chartType === "bubble";
      const isLine = ["time_series_line","time_series_line_share","time_series_area","time_series_area_share","line_multi_series","area_stack"].includes(chartType);

      if (isBar) {
        const current = { ...(prev.chartTypeOptions || {}), [key]: value };
        const labelNumFmt = current.labelNumberFormat || "none";
        const fmtLabel = (v: number) => {
          if (labelNumFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
          if (labelNumFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
          if (labelNumFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
          if (labelNumFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
          return v.toLocaleString();
        };
        const updated = prev.series.map((s: any) => {
          const u: any = { ...s };
          if (current.barBorderRadius !== undefined) u.itemStyle = { ...s.itemStyle, borderRadius: Number(current.barBorderRadius) };
          if (current.showDataLabels !== undefined || current.labelPosition !== undefined || current.labelNumberFormat !== undefined || current.labelStep !== undefined) {
            u.label = {
              ...s.label,
              show: current.showDataLabels ?? s.label?.show ?? false,
              position: current.labelPosition || (chartType === "bar_horizontal" ? "right" : "top"),
              fontSize: 11,
              formatter: current.showDataLabels ?? s.label?.show
                ? (params: any) => fmtLabel(Number(params.value))
                : undefined,
            };
          }
          return u;
        });
        return { ...prev, series: updated, chartTypeOptions: current };
      }

      if (isPie) {
        const current = { ...(prev.chartTypeOptions || {}), [key]: value };
        const updated = prev.series.map((s: any) => {
          const u: any = { ...s };
          if (current.showPieLabels !== undefined) {
            const lc = current.labelContent || "name_pct";
            const fmt = lc === "percentage" ? "{d}%" : lc === "value" ? "{c}" : lc === "name_value" ? "{b}: {c}" : "{b}: {d}%";
            u.label = { show: current.showPieLabels, formatter: fmt };
            u.labelLine = { show: current.showPieLabels };
          }
          if (chartType === "donut" && current.donutHoleSize !== undefined) u.radius = [`${current.donutHoleSize}%`, "75%"];
          if (current.roseType !== undefined) u.roseType = current.roseType ? "area" : undefined;
          return u;
        });
        return { ...prev, series: updated, chartTypeOptions: current };
      }

      if (isScatter) {
        const current = { ...(prev.chartTypeOptions || {}), [key]: value };
        const updated = prev.series.map((s: any) => ({
          ...s,
          symbolSize: current.symbolSize !== undefined ? Number(current.symbolSize) : s.symbolSize,
          itemStyle: current.pointOpacity !== undefined ? { ...s.itemStyle, opacity: Number(current.pointOpacity) } : s.itemStyle,
        }));
        return { ...prev, series: updated, chartTypeOptions: current };
      }

      if (isLine) {
        const current = { ...(prev.chartTypeOptions || {}), [key]: value };
        const labelNumFmt = current.labelNumberFormat || "none";
        const fmtLabel = (v: number) => {
          if (labelNumFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
          if (labelNumFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
          if (labelNumFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
          if (labelNumFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
          return v.toLocaleString();
        };
        if (current.showDataLabels !== undefined || current.labelNumberFormat !== undefined) {
          const updated = prev.series.map((s: any) => ({
            ...s,
            label: {
              ...s.label,
              show: current.showDataLabels ?? s.label?.show ?? false,
              position: "top",
              formatter: current.showDataLabels ?? s.label?.show
                ? (params: any) => fmtLabel(Number(params.value))
                : undefined,
            },
          }));
          return { ...prev, series: updated, chartTypeOptions: current };
        }
        return { ...prev, chartTypeOptions: current };
      }

      // For kpiOptions (big_number), store directly
      if (chartType === "big_number" || chartType === "big_number_trend") {
        const current = { ...(prev.kpiOptions || {}), [key]: value };
        return { ...prev, kpiOptions: current };
      }

      // Generic series update for funnel, gauge, heatmap, radar
      return { ...prev, chartTypeOptions: { ...(prev.chartTypeOptions || {}), [key]: value } };
    });
  };

  const setKpi = (key: string, value: any) => {
    setAdvancedOptions((prev: any) => ({
      ...(prev || {}),
      chartTypeOptions: { ...(prev?.chartTypeOptions || {}), [key]: value },
    }));
    setPreviewOptions((prev: any) => {
      if (!prev) return prev;
      return { ...prev, kpiOptions: { ...(prev.kpiOptions || {}), [key]: value } };
    });
  };

  const isBar = ["bar_vertical", "bar_horizontal", "stacked_bar_vertical", "stacked_bar_horizontal", "grouped_bar"].includes(chartType);
  const isPie = chartType === "pie" || chartType === "donut";
  const isScatter = chartType === "scatter" || chartType === "bubble";
  const isKpi = chartType === "big_number" || chartType === "big_number_trend";
  const isFunnel = chartType === "funnel";
  const isGauge = chartType === "gauge";
  const isHeatmap = chartType === "heatmap";
  const isRadar = chartType === "radar";
  const isLine = ["time_series_line","time_series_line_share","time_series_area","time_series_area_share","line_multi_series","area_stack"].includes(chartType);
  const isMixed = chartType === "mixed_line_bar";

  if (!isBar && !isPie && !isScatter && !isKpi && !isFunnel && !isGauge && !isHeatmap && !isRadar && !isLine && !isMixed) return null;

  return (
    <div style={{ marginBottom: 24 }}>
      {/* BAR OPTIONS */}
      {isBar && (
        <>
          <CheckRow id="bar-labels" label="Show data labels" checked={ctOpts.showDataLabels ?? false} onChange={(v) => set("showDataLabels", v)} />
          {ctOpts.showDataLabels && (
            <Row2>
              <div>
                <label className="chart-builder-label" htmlFor="bar-label-pos">Label position</label>
                <select id="bar-label-pos" className="chart-builder-select" value={ctOpts.labelPosition || (chartType === "bar_horizontal" ? "right" : "top")} onChange={(e) => set("labelPosition", e.target.value)}>
                  {chartType === "bar_horizontal" ? (
                    <>
                      <option value="right">Outside (right)</option>
                      <option value="insideRight">Inside right</option>
                      <option value="inside">Center</option>
                    </>
                  ) : (
                    <>
                      <option value="top">Outside (top)</option>
                      <option value="inside">Inside</option>
                      <option value="insideTop">Inside top</option>
                    </>
                  )}
                </select>
              </div>
              <div>
                <label className="chart-builder-label" htmlFor="bar-label-fmt">Label number format</label>
                <select id="bar-label-fmt" className="chart-builder-select" value={ctOpts.labelNumberFormat || "none"} onChange={(e) => set("labelNumberFormat", e.target.value)}>
                  <option value="none">Raw</option>
                  <option value="k">Thousands (K)</option>
                  <option value="m">Millions (M)</option>
                  <option value="b">Billions (B)</option>
                  <option value="t">Trillions (T)</option>
                </select>
              </div>
            </Row2>
          )}
          <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
            <label className="chart-builder-label" htmlFor="bar-radius">Bar corner radius</label>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <input id="bar-radius" type="range" min={0} max={12} step={1} value={ctOpts.barBorderRadius ?? 0} onChange={(e) => set("barBorderRadius", Number(e.target.value))} style={{ flex: 1 }} />
              <span style={{ fontSize: 12, color: "#64748b", minWidth: 28 }}>{ctOpts.barBorderRadius ?? 0}px</span>
            </div>
          </div>
        </>
      )}

      {/* PIE / DONUT OPTIONS */}
      {isPie && (
        <>
          <CheckRow id="pie-labels" label="Show labels" checked={ctOpts.showPieLabels ?? false} onChange={(v) => set("showPieLabels", v)} />
          {ctOpts.showPieLabels && (
            <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
              <label className="chart-builder-label" htmlFor="pie-label-content">Label content</label>
              <select id="pie-label-content" className="chart-builder-select" value={ctOpts.labelContent || "name_pct"} onChange={(e) => set("labelContent", e.target.value)}>
                <option value="name_pct">Name + %</option>
                <option value="percentage">% only</option>
                <option value="value">Value only</option>
                <option value="name_value">Name + value</option>
              </select>
            </div>
          )}
          {chartType === "donut" && (
            <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
              <label className="chart-builder-label" htmlFor="donut-hole">Hole size</label>
              <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <input id="donut-hole" type="range" min={20} max={70} step={5} value={ctOpts.donutHoleSize ?? 50} onChange={(e) => set("donutHoleSize", Number(e.target.value))} style={{ flex: 1 }} />
                <span style={{ fontSize: 12, color: "#64748b", minWidth: 30 }}>{ctOpts.donutHoleSize ?? 50}%</span>
              </div>
            </div>
          )}
          <CheckRow id="rose-chart" label="Rose chart (varied radius)" checked={ctOpts.roseType ?? false} onChange={(v) => set("roseType", v)} />
        </>
      )}

      {/* SCATTER / BUBBLE OPTIONS */}
      {isScatter && (
        <>
          <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
            <label className="chart-builder-label" htmlFor="scatter-size">Point size</label>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <input id="scatter-size" type="range" min={4} max={30} step={1} value={ctOpts.symbolSize ?? 10} onChange={(e) => set("symbolSize", Number(e.target.value))} style={{ flex: 1 }} />
              <span style={{ fontSize: 12, color: "#64748b", minWidth: 28 }}>{ctOpts.symbolSize ?? 10}px</span>
            </div>
          </div>
          <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
            <label className="chart-builder-label" htmlFor="scatter-opacity">Point opacity</label>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <input id="scatter-opacity" type="range" min={0.1} max={1} step={0.05} value={ctOpts.pointOpacity ?? 0.8} onChange={(e) => set("pointOpacity", Number(e.target.value))} style={{ flex: 1 }} />
              <span style={{ fontSize: 12, color: "#64748b", minWidth: 30 }}>{Math.round((ctOpts.pointOpacity ?? 0.8) * 100)}%</span>
            </div>
          </div>
        </>
      )}

      {/* BIG NUMBER OPTIONS */}
      {isKpi && (
        <>
          <Row2>
            <div>
              <label className="chart-builder-label" htmlFor="kpi-fmt">Number format</label>
              <select id="kpi-fmt" className="chart-builder-select" value={ctOpts.numberFormat || "auto"} onChange={(e) => setKpi("numberFormat", e.target.value)}>
                <option value="auto">Auto (K/M/B)</option>
                <option value="k">Thousands (K)</option>
                <option value="m">Millions (M)</option>
                <option value="b">Billions (B)</option>
                <option value="t">Trillions (T)</option>
                <option value="fixed">Fixed decimals</option>
              </select>
            </div>
            {ctOpts.numberFormat === "fixed" && (
              <div>
                <label className="chart-builder-label" htmlFor="kpi-dec">Decimal places</label>
                <select id="kpi-dec" className="chart-builder-select" value={ctOpts.decimalPlaces ?? 2} onChange={(e) => setKpi("decimalPlaces", Number(e.target.value))}>
                  {[0, 1, 2, 3, 4].map((n) => <option key={n} value={n}>{n}</option>)}
                </select>
              </div>
            )}
          </Row2>
          <Row2>
            <div>
              <label className="chart-builder-label" htmlFor="kpi-prefix">Prefix</label>
              <input id="kpi-prefix" type="text" className="chart-builder-input" placeholder='e.g. "$"' maxLength={8} value={ctOpts.prefix || ""} onChange={(e) => setKpi("prefix", e.target.value)} />
            </div>
            <div>
              <label className="chart-builder-label" htmlFor="kpi-suffix">Suffix</label>
              <input id="kpi-suffix" type="text" className="chart-builder-input" placeholder='e.g. " users"' maxLength={12} value={ctOpts.suffix || ""} onChange={(e) => setKpi("suffix", e.target.value)} />
            </div>
          </Row2>
        </>
      )}

      {/* FUNNEL OPTIONS */}
      {isFunnel && (
        <>
          <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
            <label className="chart-builder-label" htmlFor="funnel-sort">Sort order</label>
            <select id="funnel-sort" className="chart-builder-select" value={ctOpts.funnelSort || "descending"} onChange={(e) => set("funnelSort", e.target.value)}>
              <option value="descending">Descending (largest first)</option>
              <option value="ascending">Ascending (smallest first)</option>
              <option value="none">None</option>
            </select>
          </div>
          <CheckRow id="funnel-labels" label="Show labels" checked={ctOpts.showFunnelLabels ?? true} onChange={(v) => set("showFunnelLabels", v)} />
          {ctOpts.showFunnelLabels !== false && (
            <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
              <label className="chart-builder-label" htmlFor="funnel-label-pos">Label position</label>
              <select id="funnel-label-pos" className="chart-builder-select" value={ctOpts.funnelLabelPos || "inside"} onChange={(e) => set("funnelLabelPos", e.target.value)}>
                <option value="inside">Inside</option>
                <option value="left">Left</option>
                <option value="right">Right</option>
              </select>
            </div>
          )}
        </>
      )}

      {/* GAUGE OPTIONS */}
      {isGauge && (
        <Row2>
          <div>
            <label className="chart-builder-label" htmlFor="gauge-min">Min value</label>
            <input id="gauge-min" type="number" className="chart-builder-input" value={ctOpts.gaugeMin ?? 0} onChange={(e) => set("gaugeMin", Number(e.target.value))} />
          </div>
          <div>
            <label className="chart-builder-label" htmlFor="gauge-max">Max value</label>
            <input id="gauge-max" type="number" className="chart-builder-input" value={ctOpts.gaugeMax ?? 100} onChange={(e) => set("gaugeMax", Number(e.target.value))} />
          </div>
        </Row2>
      )}

      {/* HEATMAP OPTIONS */}
      {isHeatmap && (
        <CheckRow id="heatmap-values" label="Show cell values" checked={ctOpts.showCellValues ?? false} onChange={(v) => set("showCellValues", v)} />
      )}

      {/* RADAR OPTIONS */}
      {isRadar && (
        <>
          <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
            <label className="chart-builder-label" htmlFor="radar-shape">Shape</label>
            <select id="radar-shape" className="chart-builder-select" value={ctOpts.radarShape || "polygon"} onChange={(e) => set("radarShape", e.target.value)}>
              <option value="polygon">Polygon</option>
              <option value="circle">Circle</option>
            </select>
          </div>
          <CheckRow id="radar-fill" label="Fill area" checked={ctOpts.radarFill ?? false} onChange={(v) => set("radarFill", v)} />
        </>
      )}

      {/* LINE / AREA OPTIONS */}
      {isLine && (
        <>
          <CheckRow id="line-labels" label="Show data labels" checked={ctOpts.showDataLabels ?? false} onChange={(v) => set("showDataLabels", v)} />
          {ctOpts.showDataLabels && !chartType.endsWith("_share") && (
            <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
              <label className="chart-builder-label" htmlFor="line-label-fmt">Label number format</label>
              <select id="line-label-fmt" className="chart-builder-select" value={ctOpts.labelNumberFormat || "none"} onChange={(e) => set("labelNumberFormat", e.target.value)}>
                <option value="none">Raw</option>
                <option value="k">Thousands (K)</option>
                <option value="m">Millions (M)</option>
                <option value="b">Billions (B)</option>
                <option value="t">Trillions (T)</option>
              </select>
            </div>
          )}
        </>
      )}

      {/* MIXED LINE + BAR OPTIONS */}
      {isMixed && (
        <>
          <CheckRow id="mixed-labels" label="Show data labels" checked={ctOpts.showDataLabels ?? false} onChange={(v) => set("showDataLabels", v)} />
          {ctOpts.showDataLabels && (
            <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
              <label className="chart-builder-label" htmlFor="mixed-label-fmt">Label number format</label>
              <select id="mixed-label-fmt" className="chart-builder-select" value={ctOpts.labelNumberFormat || "none"} onChange={(e) => set("labelNumberFormat", e.target.value)}>
                <option value="none">Raw</option>
                <option value="k">Thousands (K)</option>
                <option value="m">Millions (M)</option>
                <option value="b">Billions (B)</option>
                <option value="t">Trillions (T)</option>
              </select>
            </div>
          )}
          {Array.isArray(previewOptions?.series) && previewOptions.series.filter((s: any) => s.tooltip?.show !== false).map((s: any, idx: number) => (
            <div key={s.name || idx} style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, marginBottom: 8, alignItems: "end" }}>
              <div>
                <label className="chart-builder-label" style={{ marginBottom: 4 }}>{s.name || `Series ${idx + 1}`} — type</label>
                <select className="chart-builder-select"
                  value={(ctOpts.seriesTypes || {})[s.name] || (idx === 0 ? "bar" : "line")}
                  onChange={(e) => set("seriesTypes", { ...(ctOpts.seriesTypes || {}), [s.name]: e.target.value })}>
                  <option value="bar">Bar</option>
                  <option value="line">Line</option>
                </select>
              </div>
              <div>
                <label className="chart-builder-label" style={{ marginBottom: 4 }}>Axis</label>
                <select className="chart-builder-select"
                  value={(ctOpts.seriesAxis || {})[s.name] !== undefined ? String((ctOpts.seriesAxis || {})[s.name]) : (idx === 0 ? "0" : "1")}
                  onChange={(e) => set("seriesAxis", { ...(ctOpts.seriesAxis || {}), [s.name]: Number(e.target.value) })}>
                  <option value="0">Left</option>
                  <option value="1">Right</option>
                </select>
              </div>
            </div>
          ))}
          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, marginBottom: 8 }}>
            <div>
              <label className="chart-builder-label" htmlFor="mixed-left-axis-name">Left axis label</label>
              <input id="mixed-left-axis-name" type="text" className="chart-builder-input" placeholder="Left axis"
                value={ctOpts.leftAxisName || ""}
                onChange={(e) => set("leftAxisName", e.target.value)} />
            </div>
            <div>
              <label className="chart-builder-label" htmlFor="mixed-right-axis-name">Right axis label</label>
              <input id="mixed-right-axis-name" type="text" className="chart-builder-input" placeholder="Right axis"
                value={ctOpts.rightAxisName || ""}
                onChange={(e) => set("rightAxisName", e.target.value)} />
            </div>
          </div>
        </>
      )}

      {/* GLOBAL LABEL DENSITY — shown for any chart where labels are visible */}
      {(
        ctOpts.showDataLabels ||
        ctOpts.showPieLabels ||
        (ctOpts.showFunnelLabels !== false && isFunnel) ||
        Array.isArray(previewOptions?.series) && previewOptions.series.some((s: any) => s.label?.show)
      ) && (
        <div className="chart-builder-field-group" style={{ marginTop: 8, marginBottom: 10 }}>
          <label className="chart-builder-label" htmlFor="label-step">
            Label density
            <span style={{ fontWeight: 400, color: "#94a3b8", marginLeft: 4 }}>
              {(ctOpts.labelStep ?? 1) === 1 ? "(show all)" : `(every ${ctOpts.labelStep})`}
            </span>
          </label>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <input
              id="label-step"
              type="range" min={1} max={10} step={1}
              value={ctOpts.labelStep ?? 1}
              onChange={(e) => set("labelStep", Number(e.target.value))}
              style={{ flex: 1 }}
            />
            <span style={{ fontSize: 12, color: "#64748b", minWidth: 20 }}>
              {ctOpts.labelStep ?? 1}
            </span>
          </div>
        </div>
      )}
    </div>
  );
};

// ─── Shared tooltip formatter (mirrors ChartBuilderContext runPreviewQuery logic) ──

function buildTooltipFormatter(
  dateFormat: string,
  numFormat: string,
  decimals: number,
  useCommas: boolean,
  isShare: boolean,
) {
  const fmtNum = (v: number): string => {
    if (numFormat === "k") return `${(v / 1e3).toFixed(decimals)}K`;
    if (numFormat === "m") return `${(v / 1e6).toFixed(decimals)}M`;
    if (numFormat === "b") return `${(v / 1e9).toFixed(decimals)}B`;
    if (numFormat === "t") return `${(v / 1e12).toFixed(decimals)}T`;
    const fixed = v.toFixed(decimals);
    if (useCommas) {
      const parts = fixed.split(".");
      parts[0] = parts[0].replace(/\B(?=(\d{3})+(?!\d))/g, ",");
      return parts.join(".");
    }
    return fixed;
  };

  const fmtDate = (val: string): string => {
    if (!val || dateFormat === "auto") return val;
    const parts = val.split("-");
    if (parts.length < 2) return val;
    const y = parseInt(parts[0], 10);
    const m = parseInt(parts[1], 10);
    const d = parts[2] ? parseInt(parts[2], 10) : 1;
    if (isNaN(y) || isNaN(m)) return val;
    const mNames = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
    const mn = mNames[m - 1] || String(m);
    if (dateFormat === "month_year" || dateFormat === "MMM YYYY") return `${mn} ${y}`;
    if (dateFormat === "date" || dateFormat === "MMM D, YYYY") return `${mn} ${d}, ${y}`;
    if (dateFormat === "YYYY-MM-DD") return val;
    if (dateFormat === "MM/DD/YYYY") return `${String(m).padStart(2,"0")}/${String(d).padStart(2,"0")}/${y}`;
    if (dateFormat === "YYYY") return String(y);
    return val;
  };

  return (params: any) => {
    if (!Array.isArray(params)) params = [params];

    // Pie / item trigger
    if (params[0]?.componentSubType === "pie" || params[0]?.seriesType === "pie") {
      const v = Number(params[0].value);
      const fv = isNaN(v) ? String(params[0].value ?? "") : fmtNum(v);
      return `${params[0].marker} ${params[0].name}: ${fv} (${params[0].percent}%)`;
    }

    const axisVal = params[0]?.axisValue || params[0]?.name || "";
    const header = `<strong>${fmtDate(String(axisVal))}</strong><br/>`;
    return header + params.map((p: any) => {
      const v = Number(p.value);
      const fv = isNaN(v) ? String(p.value ?? "") : (isShare ? `${v.toFixed(1)}%` : fmtNum(v));
      return `${p.marker} ${p.seriesName}: ${fv}`;
    }).join("<br/>");
  };
}

// ─── Main component ───────────────────────────────────────────────────────────

const AdvancedChartOptions: React.FC = () => {
  const { advancedOptions, setAdvancedOptions, previewOptions, setPreviewOptions, selectedTemplate, runPreviewQuery, chartType, timeColumn } = useChartBuilder();

  // Legend order state for UI (local, then sync to advancedOptions)
  const legendOrder = advancedOptions?.legend?.order || (previewOptions?.legend?.data || []);
  const [localLegendOrder, setLocalLegendOrder] = useState<string[]>(legendOrder);

  // Color picker state
  const [colorPickerOpen, setColorPickerOpen] = useState<number | null>(null);
  const [tempColor, setTempColor] = useState<string>("#000000");

  // Collapsible section state
  const [isColorsExpanded, setIsColorsExpanded] = useState(false);
  const [isSeriesOrderExpanded, setIsSeriesOrderExpanded] = useState(false);
  const [isTitleExpanded, setIsTitleExpanded] = useState(false);
  const [isChartOptionsExpanded, setIsChartOptionsExpanded] = useState(true);
  const [isSeriesExpanded, setIsSeriesExpanded] = useState(false);
  const [isAxesExpanded, setIsAxesExpanded] = useState(false);
  const [isTooltipsExpanded, setIsTooltipsExpanded] = useState(false);
  const [isLegendExpanded, setIsLegendExpanded] = useState(false);
  const [isRefLinesExpanded, setIsRefLinesExpanded] = useState(false);

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
  const isBarType = chartType ? ["bar_vertical","bar_horizontal","stacked_bar_vertical","stacked_bar_horizontal","grouped_bar"].includes(chartType) : false;
  const isPieType = chartType === "pie" || chartType === "donut";
  const isScatterType = chartType === "scatter" || chartType === "bubble";
  const isKpiType = chartType === "big_number" || chartType === "big_number_trend";
  const showChartTypeSection = Boolean(chartType && (isBarType || isPieType || isScatterType || isKpiType || isLineOrAreaChart || chartType === "mixed_line_bar" || chartType === "funnel" || chartType === "gauge" || chartType === "heatmap" || chartType === "radar"));

  // Axis capability flags — controls which formatting rows appear in the Axes section
  const CARTESIAN_CHARTS = [
    "time_series_line","time_series_line_share","time_series_area","time_series_area_share",
    "line_multi_series","area_stack",
    "bar_vertical","grouped_bar","stacked_bar_vertical",
    "bar_horizontal","stacked_bar_horizontal",
    "scatter","bubble",
    "boxplot","candlestick","pictorial_bar",
  ];
  const hasCartesianAxes = CARTESIAN_CHARTS.includes(chartType || "");
  const isHorizontalBar = chartType === "bar_horizontal" || chartType === "stacked_bar_horizontal";
  const isScatterLike = chartType === "scatter" || chartType === "bubble";

  // Which axis holds dates (category axis with time column)?
  // Vertical charts: X is category → date format on X
  // Horizontal bars: Y is category → date format on Y
  const hasCategoryXAxis = hasCartesianAxes && !isHorizontalBar && !isScatterLike;
  const hasCategoryYAxis = isHorizontalBar; // Y is category for horizontal bars

  // Which axis holds numeric values?
  // Vertical/line/scatter: Y is value
  // Horizontal bars: X is value
  // Scatter/bubble: BOTH axes are value
  const hasValueYAxis = hasCartesianAxes && (!isHorizontalBar || isScatterLike);
  const hasValueXAxis = isHorizontalBar || isScatterLike;

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
    setPreviewOptions((prev: any) => {
      if (!prev) return prev;
      const merged = { ...(prev.tooltip || {}), [key]: value };
      const fmt = merged.dateFormat || "auto";
      const numFmt = merged.numberFormat || "none";
      const decimals = merged.decimalPlaces ?? 2;
      const useCommas = merged.useCommas ?? true;
      const share = isShareChart;
      const formatter = buildTooltipFormatter(fmt, numFmt, decimals, useCommas, share);
      return { ...prev, tooltip: { ...merged, formatter } };
    });
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
  // Y-axis date format — for horizontal bars where Y is the category axis
  const handleYAxisDateFormat = (val: string) => {
    setAdvancedOptions((prev: any) => ({ ...(prev || {}), yAxisDateFormat: val }));
    setPreviewOptions((prev: any) => ({ ...(prev || {}), yAxisDateFormat: val }));
  };

  // For horizontal bar charts the value axis is X, not Y
  const handleXAxisFormat = (format: string) => {
    const formatter = (val: number) => formatYAxisValue(val, format);
    const update = (prev: any) => ({
      ...(prev || {}),
      xAxisFormat: format,
      xAxis: {
        ...(prev?.xAxis || {}),
        axisLabel: { ...(prev?.xAxis?.axisLabel || {}), formatter, hideOverlap: true },
      },
    });
    setAdvancedOptions(update);
    setPreviewOptions(update);
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
      <div className="chart-builder-panel-title" style={{ marginBottom: 20 }}>Customize</div>

      {/* TITLE & STYLING */}
      <CollapsibleSection title="Title & Styling" expanded={isTitleExpanded} onToggle={() => setIsTitleExpanded(v => !v)}>
        <div className="chart-builder-field-group" style={{ marginBottom: 10 }}>
          <label className="chart-builder-label" htmlFor="chart-title-input">Chart title</label>
          <input
            id="chart-title-input"
            type="text"
            value={chartTitle}
            onChange={e => handleChartTitle(e.target.value)}
            placeholder="Enter chart title"
            className="chart-builder-input"
          />
        </div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10 }}>
          <div>
            <label className="chart-builder-label" htmlFor="title-font">Font</label>
            <select id="title-font" className="chart-builder-select" value={previewOptions?.titleFont || "sans-serif"}
              onChange={e => { setAdvancedOptions((p: any) => ({ ...(p||{}), titleFont: e.target.value })); setPreviewOptions((p: any) => ({ ...(p||{}), titleFont: e.target.value })); }}>
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
            <select id="title-size" className="chart-builder-select" value={previewOptions?.titleSize || "20"}
              onChange={e => { setAdvancedOptions((p: any) => ({ ...(p||{}), titleSize: e.target.value })); setPreviewOptions((p: any) => ({ ...(p||{}), titleSize: e.target.value })); }}>
              <option value="16">16px</option>
              <option value="18">18px</option>
              <option value="20">20px</option>
              <option value="24">24px</option>
              <option value="28">28px</option>
              <option value="32">32px</option>
            </select>
          </div>
        </div>
      </CollapsibleSection>

      {/* CHART TYPE-SPECIFIC OPTIONS */}
      {showChartTypeSection && chartType && (
        <CollapsibleSection title="Chart options" expanded={isChartOptionsExpanded} onToggle={() => setIsChartOptionsExpanded(v => !v)}>
          <ChartTypeOptions
            chartType={chartType}
            advancedOptions={advancedOptions}
            setAdvancedOptions={setAdvancedOptions}
            previewOptions={previewOptions}
            setPreviewOptions={setPreviewOptions}
          />
        </CollapsibleSection>
      )}

      {/* PLUGIN CUSTOM PANEL */}
      {chartType && (() => {
        const plugin = getPlugin(chartType);
        if (!plugin?.CustomizePanel) return null;
        const Panel = plugin.CustomizePanel;
        return (
          <div style={{ marginBottom: 20, paddingBottom: 6, borderBottom: '1px solid #e2e8f0' }}>
            <Panel advancedOptions={advancedOptions} setAdvancedOptions={setAdvancedOptions} />
          </div>
        );
      })()}

      {/* SERIES OPTIONS */}
      {(isLineOrBar || isLineOrAreaChart) && (
        <CollapsibleSection title="Series" expanded={isSeriesExpanded} onToggle={() => setIsSeriesExpanded(v => !v)}>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {!isShareChart && (
              <CheckRow id="stack-series" label="Stack series" checked={stacking} onChange={handleStacking} />
            )}
            {isShareChart && (
              <CheckRow id="show-pct" label="Stacked (100%)" checked={advancedOptions?.showAsPercentage !== false}
                onChange={isStacked => {
                  setAdvancedOptions((p: any) => ({ ...(p||{}), showAsPercentage: isStacked }));
                  setPreviewOptions((p: any) => ({ ...(p||{}), series: (p?.series||[]).map((s: any) => ({ ...s, stack: isStacked ? "total" : undefined })) }));
                }} />
            )}
            {isLineOrAreaChart && (
              <CheckRow id="smooth-lines" label="Smooth curves" checked={smooth} onChange={handleSmooth} />
            )}
            {isLineOrAreaChart && (
              <CheckRow id="show-markers" label="Show point markers & values" checked={markers} onChange={handleMarkers} />
            )}
          </div>
        </CollapsibleSection>
      )}

      {/* AXES */}
      {hasCartesianAxes && (
        <CollapsibleSection title="Axes" expanded={isAxesExpanded} onToggle={() => setIsAxesExpanded(v => !v)}>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10, marginBottom: 10 }}>
            <div>
              <label className="chart-builder-label" htmlFor="x-axis-title">X axis title</label>
              <input id="x-axis-title" type="text" value={xAxis.name || ""} onChange={e => handleAxis("x","name",e.target.value)} className="chart-builder-input" placeholder="X axis" />
              <label style={{ display:'flex', alignItems:'center', marginTop:6, fontSize:12, color:'#475569', cursor:'pointer' }}>
                <input type="checkbox" checked={xAxis.show !== false} onChange={e => handleAxis("x","show",e.target.checked)} style={{ marginRight:5 }} />
                Show axis
              </label>
            </div>
            <div>
              <label className="chart-builder-label" htmlFor="y-axis-title">Y axis title</label>
              <input id="y-axis-title" type="text" value={yAxis.name || ""} onChange={e => handleAxis("y","name",e.target.value)} className="chart-builder-input" placeholder="Y axis" />
              <label style={{ display:'flex', alignItems:'center', marginTop:6, fontSize:12, color:'#475569', cursor:'pointer' }}>
                <input type="checkbox" checked={yAxis.show !== false} onChange={e => handleAxis("y","show",e.target.checked)} style={{ marginRight:5 }} />
                Show axis
              </label>
            </div>
          </div>
          {(hasCategoryXAxis || hasCategoryYAxis || hasValueYAxis || hasValueXAxis) && (
            <div style={{ display:'grid', gridTemplateColumns:'1fr 1fr', gap:10 }}>
              {hasCategoryXAxis && timeColumn ? (
                <div>
                  <label className="chart-builder-label" htmlFor="x-axis-date-format">X date format</label>
                  <select id="x-axis-date-format" className="chart-builder-select" value={previewOptions?.xAxisDateFormat || "auto"}
                    onChange={e => { setAdvancedOptions((p:any) => ({...(p||{}), xAxisDateFormat:e.target.value})); setPreviewOptions((p:any) => ({...(p||{}), xAxisDateFormat:e.target.value})); }}>
                    <option value="auto">Auto</option>
                    <option value="MM/DD/YYYY">MM/DD/YYYY</option>
                    <option value="YYYY-MM-DD">YYYY-MM-DD</option>
                    <option value="MMM YYYY">Jan 2025</option>
                    <option value="MMM D, YYYY">Jan 1, 2025</option>
                    <option value="YYYY">Year only</option>
                  </select>
                </div>
              ) : hasValueXAxis ? (
                <div>
                  <label className="chart-builder-label" htmlFor="x-axis-num-format">X number format</label>
                  <select id="x-axis-num-format" className="chart-builder-select" value={previewOptions?.xAxisFormat || "none"} onChange={e => handleXAxisFormat(e.target.value)}>
                    {numberFormatOptions.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
                  </select>
                </div>
              ) : <div />}

              {hasCategoryYAxis && timeColumn ? (
                <div>
                  <label className="chart-builder-label" htmlFor="y-axis-date-format">Y date format</label>
                  <select id="y-axis-date-format" className="chart-builder-select" value={previewOptions?.yAxisDateFormat || "auto"} onChange={e => handleYAxisDateFormat(e.target.value)}>
                    <option value="auto">Auto</option>
                    <option value="MM/DD/YYYY">MM/DD/YYYY</option>
                    <option value="YYYY-MM-DD">YYYY-MM-DD</option>
                    <option value="MMM YYYY">Jan 2025</option>
                    <option value="MMM D, YYYY">Jan 1, 2025</option>
                    <option value="YYYY">Year only</option>
                  </select>
                </div>
              ) : hasValueYAxis ? (
                <div>
                  <label className="chart-builder-label" htmlFor="y-axis-format">Y number format</label>
                  <select id="y-axis-format" className="chart-builder-select"
                    value={isShareChart ? "none" : yAxisFormat}
                    onChange={e => handleYAxisFormat(e.target.value)}
                    disabled={isShareChart}
                    style={isShareChart ? { backgroundColor:'#f1f5f9', color:'#94a3b8', cursor:'not-allowed', opacity:0.6 } : {}}>
                    {numberFormatOptions.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
                  </select>
                </div>
              ) : <div />}

              {isScatterLike && (
                <div>
                  <label className="chart-builder-label" htmlFor="y-axis-format-scatter">Y number format</label>
                  <select id="y-axis-format-scatter" className="chart-builder-select" value={yAxisFormat} onChange={e => handleYAxisFormat(e.target.value)}>
                    {numberFormatOptions.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
                  </select>
                </div>
              )}
            </div>
          )}
        </CollapsibleSection>
      )}

      {/* REFERENCE LINES */}
      {hasCartesianAxes && (
        <CollapsibleSection title="Reference lines" expanded={isRefLinesExpanded} onToggle={() => setIsRefLinesExpanded(v => !v)}>
          {(() => {
            const refLines: any[] = advancedOptions?.referenceLines || [];
            const updateLines = (lines: any[]) => {
              setAdvancedOptions((prev: any) => ({ ...(prev || {}), referenceLines: lines }));
              setPreviewOptions((prev: any) => ({ ...(prev || {}), referenceLines: lines }));
            };
            const addLine = () => updateLines([...refLines, { value: '', label: '', color: '#ef4444', style: 'dashed' }]);
            const removeLine = (idx: number) => updateLines(refLines.filter((_: any, i: number) => i !== idx));
            const updateLine = (idx: number, key: string, val: any) => {
              const next = refLines.map((l: any, i: number) => i === idx ? { ...l, [key]: val } : l);
              updateLines(next);
            };
            return (
              <>
                {refLines.map((rl: any, idx: number) => (
                  <div key={idx} style={{ background: '#f8fafc', border: '1px solid #e2e8f0', borderRadius: 6, padding: '10px 10px 8px', marginBottom: 8 }}>
                    <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8, marginBottom: 8 }}>
                      <div>
                        <label className="chart-builder-label">Value</label>
                        <input type="number" className="chart-builder-input" value={rl.value} placeholder="e.g. 1000"
                          onChange={e => updateLine(idx, 'value', e.target.value)} />
                      </div>
                      <div>
                        <label className="chart-builder-label">Label</label>
                        <input type="text" className="chart-builder-input" value={rl.label} placeholder="e.g. Target"
                          onChange={e => updateLine(idx, 'label', e.target.value)} />
                      </div>
                    </div>
                    <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 32px', gap: 8, alignItems: 'end' }}>
                      <div>
                        <label className="chart-builder-label">Color</label>
                        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                          <input type="color" value={rl.color || '#ef4444'} onChange={e => updateLine(idx, 'color', e.target.value)}
                            style={{ width: 32, height: 28, border: '1px solid #e2e8f0', borderRadius: 4, cursor: 'pointer', padding: 2 }} />
                          <span style={{ fontSize: 12, color: '#64748b' }}>{rl.color || '#ef4444'}</span>
                        </div>
                      </div>
                      <div>
                        <label className="chart-builder-label">Style</label>
                        <select className="chart-builder-select" value={rl.style || 'dashed'} onChange={e => updateLine(idx, 'style', e.target.value)}>
                          <option value="dashed">Dashed</option>
                          <option value="solid">Solid</option>
                          <option value="dotted">Dotted</option>
                        </select>
                      </div>
                      <button type="button" onClick={() => removeLine(idx)}
                        style={{ padding: '5px 8px', borderRadius: 5, border: '1px solid #fca5a5', background: '#fff', color: '#ef4444', cursor: 'pointer', fontSize: 12, alignSelf: 'flex-end' }}
                        title="Remove line">
                        <i className="fas fa-times" />
                      </button>
                    </div>
                  </div>
                ))}
                <button type="button" onClick={addLine}
                  style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '6px 12px', border: '1px dashed #94a3b8', borderRadius: 6, background: 'none', color: '#475569', fontSize: 12, cursor: 'pointer', width: '100%', justifyContent: 'center' }}>
                  <i className="fas fa-plus" style={{ fontSize: 10 }} /> Add reference line
                </button>
              </>
            );
          })()}
        </CollapsibleSection>
      )}

      {/* TOOLTIPS */}
      <CollapsibleSection title="Tooltips" expanded={isTooltipsExpanded} onToggle={() => setIsTooltipsExpanded(v => !v)}>
        <CheckRow id="tooltip-enable" label="Enable tooltips" checked={tooltip.show !== false} onChange={handleTooltip} />
        {tooltip.show !== false && (
          <div style={{ marginTop: 6 }}>
            <div style={{ display:'grid', gridTemplateColumns: isShareChart ? '1fr' : '1fr 1fr', gap:10, marginBottom:10 }}>
              <div>
                <label className="chart-builder-label" htmlFor="tooltip-date-format">Date format</label>
                <select id="tooltip-date-format" className="chart-builder-select" value={tooltip.dateFormat || "auto"} onChange={e => handleTooltipOption("dateFormat", e.target.value)}>
                  <option value="auto">Auto</option>
                  <option value="MM/DD/YYYY">MM/DD/YYYY</option>
                  <option value="YYYY-MM-DD">YYYY-MM-DD</option>
                  <option value="MMM YYYY">Jan 2025</option>
                  <option value="MMM D, YYYY">Jan 1, 2025</option>
                  <option value="YYYY">Year only</option>
                </select>
              </div>
              {!isShareChart && (
                <div>
                  <label className="chart-builder-label" htmlFor="tooltip-number-format">Number format</label>
                  <select id="tooltip-number-format" className="chart-builder-select" value={tooltip.numberFormat || "none"} onChange={e => handleTooltipOption("numberFormat", e.target.value)}>
                    {numberFormatOptions.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
                  </select>
                </div>
              )}
            </div>
            <div style={{ display:'grid', gridTemplateColumns: isShareChart ? '1fr' : '1fr 1fr', gap:10 }}>
              <div>
                <label className="chart-builder-label" htmlFor="tooltip-decimal-places">Decimal places</label>
                <select id="tooltip-decimal-places" className="chart-builder-select" value={tooltip.decimalPlaces ?? 2} onChange={e => handleTooltipOption("decimalPlaces", parseInt(e.target.value))}>
                  <option value="0">0</option>
                  <option value="1">1</option>
                  <option value="2">2</option>
                  <option value="3">3</option>
                  <option value="4">4</option>
                </select>
              </div>
              {!isShareChart && (
                <div style={{ display:'flex', alignItems:'flex-end', paddingBottom:2 }}>
                  <CheckRow id="tooltip-commas" label="Use commas" checked={tooltip.useCommas ?? true} onChange={v => handleTooltipOption("useCommas", v)} />
                </div>
              )}
            </div>
          </div>
        )}
      </CollapsibleSection>

      {/* LEGEND */}
      <CollapsibleSection title="Legend" expanded={isLegendExpanded} onToggle={() => setIsLegendExpanded(v => !v)}>
        <div style={{ display:'flex', alignItems:'center', gap:10, flexWrap:'wrap' }}>
          <CheckRow id="legend-show" label="Show legend" checked={legend.show !== false} onChange={handleLegendShow} />
          {legend.show !== false && (
            <>
              <div style={{ flex:1, minWidth:100 }}>
                <label className="chart-builder-label" htmlFor="legend-position">Position</label>
                <select id="legend-position" className="chart-builder-select" value={legend.left || "top"} onChange={e => handleLegendPos(e.target.value)}>
                  {legendPositions.map(p => <option key={p.value} value={p.value}>{p.label}</option>)}
                </select>
              </div>
            </>
          )}
        </div>
      </CollapsibleSection>

      {/* COLORS */}
      <div style={{ marginBottom: 20 }}>
        <div
          style={{ fontSize:13, fontWeight:600, color:'#475569', marginBottom: isColorsExpanded ? 12 : 0, paddingBottom:6, borderBottom:'1px solid #e2e8f0', display:'flex', alignItems:'center', justifyContent:'space-between', cursor:'pointer', userSelect:'none' }}
          onClick={() => setIsColorsExpanded(!isColorsExpanded)}
        >
          <span>Colors</span>
          <i className={isColorsExpanded ? "fas fa-chevron-up" : "fas fa-chevron-down"} style={{ fontSize:11, color:'#94a3b8' }} />
        </div>
        {isColorsExpanded && (
          <div style={{ marginTop: 12 }}>
            <label className="chart-builder-label" htmlFor="palette-select">Color palette</label>
            <div style={{ display:'flex', alignItems:'center', gap:8, marginBottom:8 }}>
              <select id="palette-select" className="chart-builder-select" style={{ flex:1, maxWidth:160 }}
                value={selectedPaletteName} onChange={e => handlePaletteDropdown(e.target.value)}>
                {colorPalettes.map(p => <option key={p.name} value={p.name}>{p.name}</option>)}
                <option value="Custom">Custom</option>
              </select>
              <div style={{ display:'flex', gap:4 }}>
                {(colorPalettes.find(p => p.name === selectedPaletteName)?.colors || palette).map((c:string, i:number) => (
                  <span key={i} style={{ width:18, height:18, borderRadius:3, border:'2px solid #fff', boxShadow:'0 0 0 1px #e2e8f0', background:c, display:'inline-block' }} />
                ))}
              </div>
            </div>
            {selectedPaletteName === "Custom" && (
              <ColorPaletteSelector value={palette} onChange={colors => { setAdvancedOptions((p:any) => ({...(p||{}), color:colors})); setPreviewOptions((p:any) => ({...(p||{}), color:colors})); }} />
            )}
          </div>
        )}
      </div>

      {/* SERIES ORDER */}
      {Array.isArray(localLegendOrder) && localLegendOrder.length > 1 && (
        <div style={{ marginBottom: 20 }}>
          <div
            style={{ fontSize:13, fontWeight:600, color:'#475569', marginBottom: isSeriesOrderExpanded ? 12 : 0, paddingBottom:6, borderBottom:'1px solid #e2e8f0', display:'flex', alignItems:'center', justifyContent:'space-between', cursor:'pointer', userSelect:'none' }}
            onClick={() => setIsSeriesOrderExpanded(!isSeriesOrderExpanded)}
          >
            <span>Series order</span>
            <i className={isSeriesOrderExpanded ? "fas fa-chevron-up" : "fas fa-chevron-down"} style={{ fontSize:11, color:'#94a3b8' }} />
          </div>
          {isSeriesOrderExpanded && (
            <div style={{ background:'#f8fafc', border:'1px solid #e2e8f0', borderRadius:6, padding:8, marginTop:12 }}>
              {localLegendOrder.map((item, idx) => (
                <div key={item} style={{ display:'flex', alignItems:'center', gap:8, padding:'6px 8px', background:'#fff', border:'1px solid #e2e8f0', borderRadius:4, marginBottom: idx < localLegendOrder.length - 1 ? 6 : 0, fontSize:13 }}>
                  <button type="button" onClick={() => openColorPicker(idx)} title="Change color"
                    style={{ display:'inline-block', width:22, height:22, borderRadius:4, background:palette[idx % palette.length], border:'2px solid #fff', boxShadow:'0 0 0 1px #e2e8f0', cursor:'pointer', padding:0 }}
                    onMouseOver={e => e.currentTarget.style.transform='scale(1.1)'} onMouseOut={e => e.currentTarget.style.transform='scale(1)'} />
                  <span style={{ flex:1, color:'#334155', fontWeight:500 }}>{item}</span>
                  <button type="button" onClick={() => moveLegendItem(idx, -1)} disabled={idx === 0}
                    style={{ padding:'3px 7px', fontSize:12, border:'1px solid #e2e8f0', borderRadius:4, background: idx === 0 ? '#f1f5f9' : '#fff', color: idx === 0 ? '#cbd5e1' : '#475569', cursor: idx === 0 ? 'not-allowed' : 'pointer', fontWeight:600 }}>↑</button>
                  <button type="button" onClick={() => moveLegendItem(idx, 1)} disabled={idx === localLegendOrder.length - 1}
                    style={{ padding:'3px 7px', fontSize:12, border:'1px solid #e2e8f0', borderRadius:4, background: idx === localLegendOrder.length - 1 ? '#f1f5f9' : '#fff', color: idx === localLegendOrder.length - 1 ? '#cbd5e1' : '#475569', cursor: idx === localLegendOrder.length - 1 ? 'not-allowed' : 'pointer', fontWeight:600 }}>↓</button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Color Picker Modal */}
      {colorPickerOpen !== null && (
        <div style={{ position:'fixed', top:0, left:0, right:0, bottom:0, zIndex:1000, display:'flex', alignItems:'center', justifyContent:'center', background:'rgba(0,0,0,0.3)' }}
          onClick={e => { if (e.target === e.currentTarget) setColorPickerOpen(null); }}>
          <div style={{ background:'#fff', borderRadius:12, padding:24, boxShadow:'0 8px 32px rgba(0,0,0,0.18)', minWidth:240 }}>
            <div style={{ fontSize:14, fontWeight:600, color:'#1e293b', marginBottom:16 }}>Pick a color</div>
            <input type="color" value={tempColor} onChange={e => setTempColor(e.target.value)}
              style={{ width:'100%', height:48, border:'none', borderRadius:8, cursor:'pointer', marginBottom:16 }} />
            <div style={{ display:'flex', gap:8, justifyContent:'flex-end' }}>
              <button type="button" onClick={() => setColorPickerOpen(null)}
                style={{ padding:'7px 16px', borderRadius:7, border:'1px solid #e2e8f0', background:'#fff', color:'#475569', fontSize:13, cursor:'pointer' }}>Cancel</button>
              <button type="button" onClick={applyColor}
                style={{ padding:'7px 16px', borderRadius:7, border:'none', background:'var(--loomx-primary)', color:'#fff', fontSize:13, fontWeight:600, cursor:'pointer' }}>Apply</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

export default AdvancedChartOptions;