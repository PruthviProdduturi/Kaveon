import React from "react";
import dynamic from "next/dynamic";
import { useChartBuilder } from "./ChartBuilderContext";

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

const ChartPreview: React.FC = () => {
  const { selectedTemplate, selectedDatasetId, previewOptions, sqlPreview, description } = useChartBuilder();

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
        // Use order as data if present
        data: order && Array.isArray(order) && order.length > 0 ? order : legendProps.data,
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

  // Preserve label settings when explicitly configured (e.g., when markers are enabled)
  // Note: Overlap handling is done via top-level labelLayout option (ECharts native API)
  if (chartOptions.series && Array.isArray(chartOptions.series)) {
    chartOptions = {
      ...chartOptions,
      series: chartOptions.series.map((series: any) => ({
        ...series,
        label: {
          ...(series.label || {}),
          // Preserve show setting if explicitly set
          show: series.label?.show !== undefined ? series.label.show : false,
        },
      })),
    };
  }

  // Ensure labelLayout is preserved and properly configured for overlap handling
  if (chartOptions.series?.some((s: any) => s.label?.show)) {
    chartOptions = {
      ...chartOptions,
      labelLayout: {
        hideOverlap: true,
      },
    };
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

  return (
    <div className="chart-builder-preview-card">
      <div className="chart-builder-preview-header">
        <div className="chart-builder-preview-meta">
          {subtitle && <div>{subtitle}</div>}
          {/* Timer removed as requested */}
        </div>
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
      <div
        className={
          hasOption && !sqlPreview.isRunning
            ? "chart-builder-preview-inner"
            : "chart-builder-preview-inner chart-builder-preview-inner-empty"
        }
      >
        {hasOption && !sqlPreview.isRunning ? (
          <ReactECharts
            option={chartOptions}
            style={{ width: "100%", height: "100%" }}
            key={
              // Force re-render when xAxis date format changes
              (chartOptions.xAxis && chartOptions.xAxis.axisLabel && chartOptions.xAxis.axisLabel.formatter ? chartOptions.xAxisDateFormat || chartOptions.xAxis.axisLabel.formatter.toString() : 'default')
            }
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
