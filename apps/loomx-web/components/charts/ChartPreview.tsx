import React, { useRef, useCallback } from "react";
import dynamic from "next/dynamic";
import { useChartBuilder } from "./ChartBuilderContext";
import { getPlugin } from "./chartPluginRegistry";

const ReactECharts = dynamic(() => import("echarts-for-react"), { ssr: false });

const formatDuration = (ms: number): string => {
  if (!Number.isFinite(ms) || ms <= 0) return "";

  const totalSeconds = ms / 1000;
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;

  const minutesStr = String(minutes).padStart(2, "0");
  const secondsStr = seconds.toFixed(2).padStart(5, "0");

  if (hours > 0) {
    const hoursStr = String(hours).padStart(2, "0");
    return `${hoursStr}:${minutesStr}:${secondsStr}`;
  }

  return `${minutesStr}:${secondsStr}`;
};

interface BigNumberKpiCardProps {
  options: any;
  rows: (string | number | null)[][];
  columns: string[];
}

const BigNumberKpiCard: React.FC<BigNumberKpiCardProps> = ({ options, rows, columns }) => {
  const lastRow = rows[rows.length - 1];
  const kpiOpts = options?.kpiOptions || {};

  // Find the metric column (last numeric column)
  let value: number | null = null;
  let label = "";
  let trend: number | null = null;

  if (lastRow) {
    for (let i = columns.length - 1; i >= 0; i--) {
      const v = lastRow[i];
      if (v !== null && v !== undefined && !isNaN(Number(v))) {
        value = Number(v);
        label = columns[i] || "";
        break;
      }
    }
  }

  // Trend: compare last two rows
  if (rows.length >= 2) {
    const prevRow = rows[rows.length - 2];
    const colIdx = columns.findIndex((c) => c === label);
    if (colIdx >= 0 && prevRow) {
      const prev = Number(prevRow[colIdx]);
      if (!isNaN(prev) && prev !== 0 && value !== null) {
        trend = ((value - prev) / Math.abs(prev)) * 100;
      }
    }
  }

  const numFmt = kpiOpts.numberFormat || "auto";
  const prefix = kpiOpts.prefix || "";
  const suffix = kpiOpts.suffix || "";

  const formatKpiValue = (v: number): string => {
    if (numFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
    if (numFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
    if (numFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
    if (numFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
    if (numFmt === "fixed") return v.toFixed(kpiOpts.decimalPlaces ?? 2);
    // auto: pick best unit
    if (Math.abs(v) >= 1e9) return `${(v / 1e9).toFixed(1)}B`;
    if (Math.abs(v) >= 1e6) return `${(v / 1e6).toFixed(1)}M`;
    if (Math.abs(v) >= 1e3) return `${(v / 1e3).toFixed(1)}K`;
    return v.toLocaleString(undefined, { maximumFractionDigits: 2 });
  };

  const formatted = value === null ? "—" : `${prefix}${formatKpiValue(value)}${suffix}`;

  const trendColor = trend === null ? "#6b7280" : trend >= 0 ? "#16a34a" : "#dc2626";
  const trendIcon = trend === null ? null : trend >= 0 ? "▲" : "▼";

  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", height: "100%", gap: 8, padding: 24 }}>
      {options?.title?.text && (
        <div style={{ fontSize: 13, color: "#6b7280", fontWeight: 500, textAlign: "center" }}>{options.title.text}</div>
      )}
      <div style={{ fontSize: 52, fontWeight: 700, color: "#0f172a", lineHeight: 1, letterSpacing: "-2px" }}>{formatted}</div>
      {label && <div style={{ fontSize: 13, color: "#6b7280", textTransform: "uppercase", letterSpacing: "0.05em" }}>{label}</div>}
      {trend !== null && (
        <div style={{ fontSize: 14, color: trendColor, fontWeight: 600, display: "flex", alignItems: "center", gap: 4 }}>
          <span>{trendIcon}</span>
          <span>{Math.abs(trend).toFixed(1)}% vs prior period</span>
        </div>
      )}
    </div>
  );
};

interface TableRendererProps {
  rows: (string | number | null)[][];
  columns: string[];
  isPivot?: boolean;
}

const TableRenderer: React.FC<TableRendererProps> = ({ rows, columns, isPivot }) => {
  const [sortCol, setSortCol] = React.useState<number | null>(null);
  const [sortDir, setSortDir] = React.useState<"asc" | "desc">("asc");

  const handleSort = (idx: number) => {
    if (sortCol === idx) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortCol(idx);
      setSortDir("asc");
    }
  };

  let displayRows = [...rows];
  if (sortCol !== null) {
    displayRows.sort((a, b) => {
      const av = a[sortCol];
      const bv = b[sortCol];
      const an = Number(av);
      const bn = Number(bv);
      const cmp = !isNaN(an) && !isNaN(bn) ? an - bn : String(av ?? "").localeCompare(String(bv ?? ""));
      return sortDir === "asc" ? cmp : -cmp;
    });
  }

  // Pivot: rotate so row[0] values become column headers
  if (isPivot && columns.length >= 3) {
    const dimCol = columns[0];
    const catCol = columns[1];
    const valCol = columns[2];
    const pivotCats = [...new Set(rows.map((r) => String(r[1] ?? "")))];
    const pivotDims = [...new Set(rows.map((r) => String(r[0] ?? "")))];
    const pivotMap = new Map<string, Map<string, string | number | null>>();
    rows.forEach((r) => {
      const dim = String(r[0] ?? "");
      const cat = String(r[1] ?? "");
      if (!pivotMap.has(dim)) pivotMap.set(dim, new Map());
      pivotMap.get(dim)!.set(cat, r[2]);
    });
    const pivotCols = [dimCol, ...pivotCats];
    const pivotRows = pivotDims.map((dim) => {
      const row: (string | number | null)[] = [dim];
      pivotCats.forEach((cat) => row.push(pivotMap.get(dim)?.get(cat) ?? null));
      return row;
    });
    displayRows = pivotRows;
    columns = pivotCols;
  }

  const isNumeric = (v: string | number | null) => v !== null && v !== "" && !isNaN(Number(v));

  return (
    <div style={{ width: "100%", height: "100%", overflow: "auto" }}>
      <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 12, tableLayout: "auto" }}>
        <thead>
          <tr>
            {columns.map((col, i) => (
              <th
                key={i}
                onClick={() => handleSort(i)}
                style={{
                  padding: "6px 10px",
                  background: "#f8fafc",
                  borderBottom: "2px solid #e2e8f0",
                  textAlign: "left",
                  fontWeight: 600,
                  color: "#374151",
                  whiteSpace: "nowrap",
                  cursor: "pointer",
                  userSelect: "none",
                  position: "sticky",
                  top: 0,
                  zIndex: 1,
                }}
              >
                {col.split(".").pop() ?? col}
                {sortCol === i ? (sortDir === "asc" ? " ▲" : " ▼") : ""}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {displayRows.map((row, ri) => (
            <tr key={ri} style={{ background: ri % 2 === 0 ? "#fff" : "#f9fafb" }}>
              {row.map((cell, ci) => (
                <td
                  key={ci}
                  style={{
                    padding: "5px 10px",
                    borderBottom: "1px solid #e2e8f0",
                    textAlign: isNumeric(cell) ? "right" : "left",
                    color: "#1f2937",
                    whiteSpace: "nowrap",
                    fontVariantNumeric: "tabular-nums",
                  }}
                >
                  {cell === null || cell === undefined ? (
                    <span style={{ color: "#9ca3af" }}>null</span>
                  ) : isNumeric(cell) ? (
                    Number(cell).toLocaleString()
                  ) : (
                    String(cell)
                  )}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
};

interface ChartPreviewProps {
  /** Called when user clicks a data point in dashboard view — enables cross-filtering */
  onCrossFilter?: (value: string) => void;
}

const ChartPreview: React.FC<ChartPreviewProps> = ({ onCrossFilter }) => {
  const { selectedTemplate, selectedDatasetId, previewOptions, sqlPreview, description, chartType, advancedOptions } = useChartBuilder();
  const echartsInstanceRef = useRef<any>(null);

  // ── Download helpers ─────────────────────────────────────────────────────────
  const downloadPng = useCallback(() => {
    if (!echartsInstanceRef.current) return;
    const url = echartsInstanceRef.current.getDataURL({ type: 'png', pixelRatio: 2, backgroundColor: '#fff' });
    const a = document.createElement('a');
    a.href = url;
    a.download = 'chart.png';
    a.click();
  }, []);

  const downloadCsv = useCallback(() => {
    const cols = sqlPreview.dataColumns;
    const rows = sqlPreview.dataRows;
    if (!cols?.length || !rows?.length) return;
    const escape = (v: any) => {
      const s = v === null || v === undefined ? '' : String(v);
      return s.includes(',') || s.includes('"') || s.includes('\n') ? `"${s.replace(/"/g, '""')}"` : s;
    };
    const csv = [cols.map(escape).join(','), ...rows.map(r => r.map(escape).join(','))].join('\n');
    const blob = new Blob([csv], { type: 'text/csv' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'chart.csv';
    a.click();
    URL.revokeObjectURL(url);
  }, [sqlPreview.dataColumns, sqlPreview.dataRows]);

  const hasOption = Boolean(previewOptions);

  const subtitle = (() => {
    if (!selectedDatasetId) return "Choose a dataset";
    return "";
  })();

  const runtimeLabel = (() => {
    if (!hasOption || sqlPreview.durationMs == null || sqlPreview.durationMs <= 0) return "";
    return formatDuration(sqlPreview.durationMs);
  })();

  // Map legend position values to ECharts positions
  const legendPosMap: Record<string, { left: string; top?: string }> = {
    top: { left: "center", top: "top" },
    topCenter: { left: "center", top: "top" },
    bottom: { left: "center", top: "bottom" },
    bottomCenter: { left: "center", top: "bottom" },
    left: { left: "left", top: "middle" },
    right: { left: "right", top: "middle" },
  };

  let chartOptions = previewOptions || {};
  // Always ensure chartOptions.title is an object with a text property
  let titleText = "";
  if (typeof chartOptions.title === "object" && chartOptions.title !== null && "text" in chartOptions.title) {
    titleText = chartOptions.title.text;
  } else if (typeof chartOptions.title === "string") {
    titleText = chartOptions.title;
  }
  chartOptions = {
    ...chartOptions,
    title: {
      text: titleText,
      left: "center",
      top: 5,
      textStyle: {
        fontSize: Number(chartOptions.titleSize) || 20,
        fontWeight: "bold",
        fontFamily: chartOptions.titleFont || "sans-serif",
      },
    },
  };
  // Set legend position and clean up non-standard properties
  if (chartOptions.legend) {
    const { order, ...legendProps } = chartOptions.legend;
    const pos = legendProps.left ? (legendPosMap[legendProps.left] || { left: "center", top: "top" }) : { left: "center", top: "top" };
    chartOptions = {
      ...chartOptions,
      legend: {
        ...legendProps,
        left: pos.left,
        top: pos.top,
        data: order && Array.isArray(order) && order.length > 0 ? order : legendProps.data,
        textStyle: {
          fontSize: 12,
          color: "#475569",
          fontFamily: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
          ...(legendProps.textStyle || {}),
        },
        itemGap: 16,
        itemWidth: 12,
        itemHeight: 12,
        icon: legendProps.icon || "roundRect",
      },
    };
  }

  // Prevent label overlapping - handle both single and array xAxis/yAxis
  if (chartOptions.xAxis) {
    if (Array.isArray(chartOptions.xAxis)) {
      chartOptions = {
        ...chartOptions,
        xAxis: chartOptions.xAxis.map((axis: any) => ({
          ...axis,
          axisLabel: {
            ...(axis.axisLabel || {}),
            hideOverlap: true,
            overflow: 'truncate',
            rotate: 0,
            margin: 12,
          },
        })),
      };
    } else {
      chartOptions = {
        ...chartOptions,
        xAxis: {
          ...chartOptions.xAxis,
          axisLabel: {
            ...(chartOptions.xAxis.axisLabel || {}),
            hideOverlap: true,
            overflow: 'truncate',
            rotate: 0,
            margin: 12,
          },
        },
      };
    }
  }

  if (chartOptions.yAxis) {
    if (Array.isArray(chartOptions.yAxis)) {
      chartOptions = {
        ...chartOptions,
        yAxis: chartOptions.yAxis.map((axis: any) => ({
          ...axis,
          axisLabel: {
            ...(axis.axisLabel || {}),
            hideOverlap: true,
            overflow: 'truncate',
            margin: 10,
          },
        })),
      };
    } else {
      chartOptions = {
        ...chartOptions,
        yAxis: {
          ...chartOptions.yAxis,
          axisLabel: {
            ...(chartOptions.yAxis.axisLabel || {}),
            hideOverlap: true,
            overflow: 'truncate',
            margin: 10,
          },
        },
      };
    }
  }

  // ── Global aesthetics ────────────────────────────────────────────────────────
  // Tooltip — polished card style applied to every chart type
  if (chartOptions.tooltip) {
    const tooltipBase = {
      appendToBody: true,
      confine: true,
      backgroundColor: "rgba(255, 255, 255, 0.97)",
      borderColor: "#e2e8f0",
      borderWidth: 1,
      padding: [10, 14],
      textStyle: {
        color: "#1e293b",
        fontSize: 12,
        lineHeight: 20,
        fontFamily: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
      },
      extraCssText:
        "box-shadow: 0 8px 24px rgba(0,0,0,0.12); border-radius: 8px; pointer-events: none; z-index: 99999;",
    };
    if (Array.isArray(chartOptions.tooltip)) {
      chartOptions = {
        ...chartOptions,
        tooltip: chartOptions.tooltip.map((t: any) => ({ ...tooltipBase, ...t })),
      };
    } else {
      chartOptions = {
        ...chartOptions,
        tooltip: { ...tooltipBase, ...chartOptions.tooltip },
      };
    }
  }

  // Data labels — professional typography with position-aware contrast
  const labelStep: number = (advancedOptions as any)?.chartTypeOptions?.labelStep ?? 1;
  const labelNumFmt: string = (advancedOptions as any)?.chartTypeOptions?.labelNumberFormat || "none";
  const isShareChart = chartType?.endsWith("_share") || false;
  const fmtLabelNum = (v: number): string => {
    if (isShareChart) return `${v.toFixed(1)}%`;
    if (labelNumFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
    if (labelNumFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
    if (labelNumFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
    if (labelNumFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
    return v.toLocaleString();
  };

  if (chartOptions.series && Array.isArray(chartOptions.series)) {
    chartOptions = {
      ...chartOptions,
      series: chartOptions.series.map((series: any) => {
        if (!series.label?.show) {
          return {
            ...series,
            label: { ...(series.label || {}), show: false },
          };
        }

        const pos = series.label.position || "top";
        const isInside =
          pos === "inside" || pos === "insideTop" || pos === "insideBottom" ||
          pos === "insideLeft" || pos === "insideRight" || pos === "insideTopLeft" ||
          pos === "insideTopRight" || pos === "insideBottomLeft" || pos === "insideBottomRight";

        const font = 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
        const existingFormatter = series.label.formatter;

        // Shared pill style — same values whether applied directly or via rich text
        const pillStyle = {
          backgroundColor: "rgba(255,255,255,0.92)",
          borderRadius: 4,
          padding: [3, 6] as [number, number],
          borderColor: "#e2e8f0",
          borderWidth: 1,
          color: "#334155",
          fontSize: 11,
          fontWeight: "600",
          fontFamily: font,
        };

        const styledLabel: any = {
          ...series.label,
          fontSize: 11,
          fontWeight: isInside ? "700" : "600",
          fontFamily: font,
          color: series.label.color || (isInside ? "#ffffff" : pillStyle.color),
          textShadowColor: isInside ? "rgba(0,0,0,0.45)" : "rgba(255,255,255,0.9)",
          textShadowBlur: isInside ? 4 : 3,
          ...(!isInside && { ...pillStyle }),
        };

        if (labelStep > 1 && !isInside && typeof existingFormatter !== "string") {
          // Density + outside: move pill into rich text so shown labels keep the
          // container while hidden labels return "" with no label-level background
          // (no background on label = no ghost dot artifact).
          delete styledLabel.backgroundColor;
          delete styledLabel.borderRadius;
          delete styledLabel.padding;
          delete styledLabel.borderColor;
          delete styledLabel.borderWidth;
          styledLabel.rich = { pill: { ...pillStyle, padding: [3, 6] } };
          styledLabel.formatter = (params: any) => {
            if (params.dataIndex % labelStep !== 0) return "";
            const val = typeof existingFormatter === "function"
              ? String(existingFormatter(params))
              : fmtLabelNum(Number(params.value ?? 0));
            return `{pill|${val}}`;
          };
        } else if (labelStep > 1) {
          // Inside or string-template: "" is safe, no background to ghost
          styledLabel.formatter = (params: any) => {
            if (params.dataIndex % labelStep !== 0) return "";
            if (typeof existingFormatter === "function") return existingFormatter(params);
            if (typeof existingFormatter === "string") return existingFormatter;
            return fmtLabelNum(Number(params.value ?? 0));
          };
        }

        // When density > 1, sync marker dots to the same step so labels and dots align
        const showSymbolOverride = labelStep > 1 && series.showSymbol !== false && series.showSymbol !== undefined
          ? (dataIndex: number) => dataIndex % labelStep === 0
          : series.showSymbol;

        return {
          ...series,
          labelLayout: { hideOverlap: true },
          label: styledLabel,
          ...(showSymbolOverride !== series.showSymbol && { showSymbol: showSymbolOverride }),
          emphasis: {
            ...(series.emphasis || {}),
            label: { show: false },
          },
        };
      }),
    };
  }

  // ── Reference lines (markLine) ───────────────────────────────────────────────
  const referenceLines: any[] = (advancedOptions as any)?.referenceLines || [];
  if (referenceLines.length > 0 && Array.isArray(chartOptions.series) && chartOptions.series.length > 0) {
    const markLineData = referenceLines
      .filter((rl: any) => rl.value !== '' && rl.value !== null && rl.value !== undefined)
      .map((rl: any) => ({
        name: rl.label || '',
        yAxis: Number(rl.value),
        lineStyle: {
          color: rl.color || '#ef4444',
          type: rl.style || 'dashed',
          width: 2,
        },
        label: {
          show: Boolean(rl.label),
          formatter: rl.label || '',
          position: 'end',
          color: rl.color || '#ef4444',
          fontSize: 11,
          fontWeight: 600,
        },
      }));

    if (markLineData.length > 0) {
      chartOptions = {
        ...chartOptions,
        series: chartOptions.series.map((s: any, i: number) => {
          if (i !== 0) return s; // Only attach to first series to avoid duplicate lines
          return {
            ...s,
            markLine: {
              silent: true,
              animation: false,
              symbol: ['none', 'none'],
              data: markLineData,
            },
          };
        }),
      };
    }
  }

  // Configure grid spacing to reduce top space and add bottom space
  if (!chartOptions.grid) {
    chartOptions = {
      ...chartOptions,
      grid: {
        left: '10%',
        right: '10%',
        top: titleText ? '12%' : '8%',
        bottom: '12%',
        containLabel: true,
      },
    };
  } else {
    chartOptions = {
      ...chartOptions,
      grid: {
        ...chartOptions.grid,
        top: chartOptions.grid.top || (titleText ? '12%' : '8%'),
        bottom: chartOptions.grid.bottom || '12%',
        containLabel: true,
      },
    };
  }

  const hasData = hasOption && !sqlPreview.isRunning && !!sqlPreview.dataRows?.length;

  return (
    <div className="chart-builder-preview-card">
      <div className="chart-builder-preview-header">
        <div className="chart-builder-preview-meta">
          {subtitle && <div>{subtitle}</div>}
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          {hasData && (
            <>
              <button
                type="button"
                className="chart-preview-action-btn"
                title="Download PNG"
                onClick={downloadPng}
                style={{ padding: '4px 8px', fontSize: 12, background: 'none', border: '1px solid #e2e8f0', borderRadius: 5, cursor: 'pointer', color: '#64748b', display: 'flex', alignItems: 'center', gap: 4 }}
              >
                <i className="fas fa-image" style={{ fontSize: 11 }} />
                <span>PNG</span>
              </button>
              <button
                type="button"
                className="chart-preview-action-btn"
                title="Download CSV"
                onClick={downloadCsv}
                style={{ padding: '4px 8px', fontSize: 12, background: 'none', border: '1px solid #e2e8f0', borderRadius: 5, cursor: 'pointer', color: '#64748b', display: 'flex', alignItems: 'center', gap: 4 }}
              >
                <i className="fas fa-file-csv" style={{ fontSize: 11 }} />
                <span>CSV</span>
              </button>
            </>
          )}
          {description && description.trim() && (
            <div className="chart-info-icon-container">
              <i
                className="fas fa-info-circle chart-info-icon"
                title={description}
              />
              <div className="chart-info-tooltip">
                {description}
              </div>
            </div>
          )}
        </div>
      </div>
      <div
        className={
          hasOption && !sqlPreview.isRunning
            ? "chart-builder-preview-inner"
            : "chart-builder-preview-inner chart-builder-preview-inner-empty"
        }
      >
        {hasOption && !sqlPreview.isRunning && (chartType === "table" || chartType === "pivot_table") ? (
          <TableRenderer
            rows={(previewOptions as any)?._rows ?? sqlPreview.dataRows}
            columns={(previewOptions as any)?._columns ?? sqlPreview.dataColumns}
            isPivot={chartType === "pivot_table"}
          />
        ) : hasOption && !sqlPreview.isRunning && (chartType === "big_number" || chartType === "big_number_trend") ? (
          <BigNumberKpiCard options={chartOptions} rows={sqlPreview.dataRows} columns={sqlPreview.dataColumns} />
        ) : hasOption && !sqlPreview.isRunning && chartType && getPlugin(chartType)?.Renderer ? (
          (() => {
            const PluginRenderer = getPlugin(chartType)!.Renderer!;
            return <PluginRenderer options={chartOptions} rows={sqlPreview.dataRows} columns={sqlPreview.dataColumns} />;
          })()
        ) : hasOption && !sqlPreview.isRunning ? (
          <ReactECharts
            option={chartOptions}
            style={{ width: "100%", height: "100%", cursor: onCrossFilter ? 'pointer' : undefined }}
            key={
              // Force re-render when xAxis date format changes
              (chartOptions.xAxis && chartOptions.xAxis.axisLabel && chartOptions.xAxis.axisLabel.formatter ? chartOptions.xAxisDateFormat || chartOptions.xAxis.axisLabel.formatter.toString() : 'default')
            }
            onChartReady={(instance: any) => { echartsInstanceRef.current = instance; }}
            onEvents={onCrossFilter ? {
              click: (params: any) => {
                const val = params.name || (Array.isArray(params.value) ? String(params.value[0]) : String(params.value ?? ''));
                if (val) onCrossFilter(val);
              },
            } : undefined}
          />
        ) : sqlPreview.isRunning ? (
          <div className="chart-preview-loading-state">
            <div className="chart-loading-animation">
              <div className="chart-loading-bars">
                <div className="chart-loading-bar" style={{ animationDelay: '0s' }}></div>
                <div className="chart-loading-bar" style={{ animationDelay: '0.1s' }}></div>
                <div className="chart-loading-bar" style={{ animationDelay: '0.2s' }}></div>
                <div className="chart-loading-bar" style={{ animationDelay: '0.3s' }}></div>
                <div className="chart-loading-bar" style={{ animationDelay: '0.4s' }}></div>
              </div>
              <div className="chart-loading-spinner"></div>
            </div>
            <div className="chart-loading-text">Rendering your chart...</div>
          </div>
        ) : (
          <div className="chart-preview-empty-state">
            <svg className="chart-preview-placeholder-icon" width="120" height="120" viewBox="0 0 120 120" fill="none" xmlns="http://www.w3.org/2000/svg">
              <rect x="10" y="70" width="15" height="40" rx="2" fill="#cbd5e1" opacity="0.6">
                <animate attributeName="height" values="40;60;40" dur="1.5s" repeatCount="indefinite" />
                <animate attributeName="y" values="70;50;70" dur="1.5s" repeatCount="indefinite" />
              </rect>
              <rect x="35" y="50" width="15" height="60" rx="2" fill="#cbd5e1" opacity="0.7">
                <animate attributeName="height" values="60;80;60" dur="1.5s" begin="0.2s" repeatCount="indefinite" />
                <animate attributeName="y" values="50;30;50" dur="1.5s" begin="0.2s" repeatCount="indefinite" />
              </rect>
              <rect x="60" y="40" width="15" height="70" rx="2" fill="#94a3b8" opacity="0.8">
                <animate attributeName="height" values="70;90;70" dur="1.5s" begin="0.4s" repeatCount="indefinite" />
                <animate attributeName="y" values="40;20;40" dur="1.5s" begin="0.4s" repeatCount="indefinite" />
              </rect>
              <rect x="85" y="55" width="15" height="55" rx="2" fill="#cbd5e1" opacity="0.7">
                <animate attributeName="height" values="55;75;55" dur="1.5s" begin="0.6s" repeatCount="indefinite" />
                <animate attributeName="y" values="55;35;55" dur="1.5s" begin="0.6s" repeatCount="indefinite" />
              </rect>
            </svg>
            <div className="chart-preview-empty-title">Configure your chart to see preview</div>
          </div>
        )}
      </div>
    </div>
  );
};

export default ChartPreview;
