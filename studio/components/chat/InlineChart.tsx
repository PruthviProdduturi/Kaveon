"use client";

import dynamic from "next/dynamic";
import { useState } from "react";
import { applyChartTheme } from "../../utils/echartsTheme";
import { useTheme } from "../../contexts/ThemeContext";

const ReactECharts = dynamic(() => import("echarts-for-react"), { ssr: false });

const COLORS = ["#4A9EE8", "#10b981", "#f59e0b", "#ef4444", "#8b5cf6", "#ec4899", "#06b6d4", "#f97316"];

export interface InlineChartProps {
  rows: (string | number | null)[][];
  columns: string[];
  chartType: "bar" | "line" | "pie" | "kpi" | "table";
  xAxis: string | null;
  yAxis: string | null;
  title: string;
  sql?: string;
}

function getColIndex(columns: string[], name: string | null): number {
  if (!name) return -1;
  return columns.findIndex((c) => c === name);
}

function buildOption(
  rows: InlineChartProps["rows"],
  columns: string[],
  chartType: "bar" | "line" | "pie",
  xAxis: string | null,
  yAxis: string | null,
): object {
  const xIdx = getColIndex(columns, xAxis);
  const yIdx = getColIndex(columns, yAxis);

  if (chartType === "pie") {
    const data = rows.map((row) => ({
      name: xIdx >= 0 ? String(row[xIdx] ?? "") : "",
      value: yIdx >= 0 ? Number(row[yIdx] ?? 0) : 0,
    }));
    return {
      color: COLORS,
      tooltip: { trigger: "item" },
      legend: { orient: "horizontal", bottom: 0 },
      series: [
        {
          type: "pie",
          radius: ["35%", "65%"],
          data,
          label: { color: "inherit" },
        },
      ],
    };
  }

  const xData = xIdx >= 0 ? rows.map((r) => String(r[xIdx] ?? "")) : rows.map((_, i) => String(i));
  const yData = yIdx >= 0 ? rows.map((r) => Number(r[yIdx] ?? 0)) : [];

  return {
    color: COLORS,
    tooltip: { trigger: "axis" },
    xAxis: { type: "category", data: xData },
    yAxis: { type: "value" },
    series: [
      {
        type: chartType,
        data: yData,
        ...(chartType === "line"
          ? { smooth: true, areaStyle: { opacity: 0.12 } }
          : { barMaxWidth: 48 }),
      },
    ],
  };
}

export function InlineChart({ rows, columns, chartType, xAxis, yAxis, title, sql }: InlineChartProps) {
  const { theme } = useTheme();
  const isDark = theme === "dark";
  const [sqlOpen, setSqlOpen] = useState(false);

  const containerStyle: React.CSSProperties = {
    background: "var(--bg-surface)",
    border: "1px solid var(--border)",
    borderRadius: 12,
    padding: 16,
    boxShadow: "var(--shadow-sm)",
    maxWidth: 560,
    width: "100%",
  };

  // KPI
  if (chartType === "kpi") {
    const yIdx = getColIndex(columns, yAxis);
    const value = rows[0]?.[yIdx >= 0 ? yIdx : 0] ?? null;
    return (
      <div style={containerStyle}>
        <TitleBar title={title} chartType={chartType} />
        <div style={{ textAlign: "center", padding: "24px 0 16px" }}>
          <div
            style={{
              fontSize: 32,
              fontWeight: 700,
              color: "var(--text-primary)",
              lineHeight: 1,
            }}
          >
            {value !== null ? String(value) : "—"}
          </div>
          <div
            style={{
              fontSize: 12,
              color: "var(--text-muted)",
              textTransform: "uppercase",
              marginTop: 8,
              letterSpacing: "0.08em",
            }}
          >
            {yAxis ?? title}
          </div>
        </div>
        <Footer sql={sql} sqlOpen={sqlOpen} setSqlOpen={setSqlOpen} />
      </div>
    );
  }

  // Table
  if (chartType === "table") {
    const displayRows = rows.slice(0, 20);
    return (
      <div style={containerStyle}>
        <TitleBar title={title} chartType={chartType} />
        <div style={{ overflowX: "auto", marginTop: 12 }}>
          <table
            style={{
              width: "100%",
              borderCollapse: "collapse",
              fontSize: 13,
              color: "var(--text-primary)",
            }}
          >
            <thead>
              <tr>
                {columns.map((col) => (
                  <th
                    key={col}
                    style={{
                      background: "var(--bg-surface)",
                      borderBottom: "1px solid var(--border)",
                      padding: "6px 10px",
                      textAlign: "left",
                      fontWeight: 600,
                      whiteSpace: "nowrap",
                    }}
                  >
                    {col}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {displayRows.map((row, ri) => (
                <tr key={ri}>
                  {row.map((cell, ci) => (
                    <td
                      key={ci}
                      style={{
                        borderBottom: "1px solid var(--border)",
                        padding: "6px 10px",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {cell !== null ? String(cell) : ""}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <Footer sql={sql} sqlOpen={sqlOpen} setSqlOpen={setSqlOpen} />
      </div>
    );
  }

  // Bar / Line / Pie
  const rawOption = buildOption(rows, columns, chartType, xAxis, yAxis);
  const option = applyChartTheme(rawOption, isDark);

  return (
    <div style={containerStyle}>
      <TitleBar title={title} chartType={chartType} />
      <ReactECharts
        option={option}
        style={{ height: 280, width: "100%" }}
        notMerge
        lazyUpdate
      />
      <Footer sql={sql} sqlOpen={sqlOpen} setSqlOpen={setSqlOpen} />
    </div>
  );
}

// Internal sub-components

function TitleBar({ title, chartType }: { title: string; chartType: string }) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        marginBottom: 4,
      }}
    >
      <span
        style={{
          fontSize: 14,
          fontWeight: 600,
          color: "var(--text-primary)",
          flexGrow: 1,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {title}
      </span>
      <span
        style={{
          fontSize: 10,
          textTransform: "uppercase",
          letterSpacing: "0.08em",
          color: "var(--text-muted)",
          background: "var(--bg-elevated, rgba(255,255,255,0.06))",
          border: "1px solid var(--border)",
          borderRadius: 999,
          padding: "2px 8px",
          flexShrink: 0,
        }}
      >
        {chartType}
      </span>
    </div>
  );
}

function Footer({
  sql,
  sqlOpen,
  setSqlOpen,
}: {
  sql?: string;
  sqlOpen: boolean;
  setSqlOpen: (v: boolean) => void;
}) {
  if (!sql) return null;

  return (
    <div style={{ marginTop: 12 }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 8,
        }}
      >
        <button
          onClick={() => setSqlOpen(!sqlOpen)}
          style={{
            background: "none",
            border: "none",
            cursor: "pointer",
            fontSize: 12,
            color: "var(--text-muted)",
            padding: 0,
            display: "flex",
            alignItems: "center",
            gap: 4,
          }}
          aria-expanded={sqlOpen}
          aria-label="Toggle SQL query"
        >
          <span>{sqlOpen ? "▾" : "▸"}</span>
          <span>SQL</span>
        </button>
        <a
          href={"/lab?sql=" + encodeURIComponent(sql)}
          style={{
            fontSize: 12,
            color: "var(--text-muted)",
            textDecoration: "none",
          }}
          onMouseEnter={(e) => ((e.target as HTMLAnchorElement).style.color = "var(--text-primary)")}
          onMouseLeave={(e) => ((e.target as HTMLAnchorElement).style.color = "var(--text-muted)")}
        >
          Open in SQL Lab →
        </a>
      </div>
      {sqlOpen && (
        <pre
          style={{
            marginTop: 8,
            padding: "10px 12px",
            background: "var(--bg-elevated, rgba(0,0,0,0.2))",
            border: "1px solid var(--border)",
            borderRadius: 8,
            fontSize: 12,
            color: "var(--text-muted)",
            overflowX: "auto",
            whiteSpace: "pre-wrap",
            wordBreak: "break-all",
          }}
        >
          {sql}
        </pre>
      )}
    </div>
  );
}
