"use client";

import dynamic from "next/dynamic";
import { useState } from "react";
import { applyChartTheme } from "../../utils/echartsTheme";
import { useTheme } from "../../contexts/ThemeContext";

const ReactECharts = dynamic(() => import("echarts-for-react"), { ssr: false });

/** The series palette. Literals rather than tokens because ECharts resolves
 *  colours itself and never sees a `var()` — the same reason `echartsTheme`
 *  remaps chart text. These are data colours, not chrome: the first is the
 *  brand accent and the rest carry the product's status hues. */
const COLORS = ["#4A9EE8", "#10b981", "#f59e0b", "#ef4444", "#8b5cf6", "#ec4899", "#06b6d4", "#f97316"];

/** How many rows the table form shows before it says it has stopped. */
const TABLE_ROW_LIMIT = 20;

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

/**
 * The column a chart draws, resolved by name and then by shape.
 *
 * The DLM names the axes from the question, so a statement that aliased the
 * measure differently — or an answer whose axes were never named at all —
 * left the index at -1. The KPI branch already fell back to a column and kept
 * working; bar, line and pie did not, and drew a plot with its category
 * labels in place and no series at all, which reads as the answer having no
 * data rather than as the axis having gone unmatched.
 */
function resolveValueIndex(rows: InlineChartProps["rows"], columns: string[], name: string | null): number {
  const named = getColIndex(columns, name);
  if (named >= 0) return named;
  const numeric = columns.findIndex((_, i) =>
    rows.some((r) => r[i] !== null && r[i] !== "" && Number.isFinite(Number(r[i]))));
  return numeric >= 0 ? numeric : columns.length - 1;
}

/** The category column, by name and then as the first column that is not the
 *  measure. -1 only where there is no second column to use. */
function resolveLabelIndex(columns: string[], name: string | null, valueIndex: number): number {
  const named = getColIndex(columns, name);
  if (named >= 0) return named;
  return columns.findIndex((_, i) => i !== valueIndex);
}

function buildOption(
  rows: InlineChartProps["rows"],
  columns: string[],
  chartType: "bar" | "line" | "pie",
  xAxis: string | null,
  yAxis: string | null,
): object {
  const yIdx = resolveValueIndex(rows, columns, yAxis);
  const xIdx = resolveLabelIndex(columns, xAxis, yIdx);

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
          // No `color: "inherit"` here. It drew each slice's label in that
          // slice's own colour, so an amber label measured 2.15:1 against the
          // card. Left unset, `applyChartTheme` hands labels the theme's text
          // tone — 17.06:1 in light, 13.52:1 in dark.
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

  // An answer can carry a chart specification and no rows. Every branch below
  // then drew its own shape of nothing — a blank canvas, a header row with no
  // body, an em dash — and each of those reads as a chart that failed to
  // render. This says what the component was actually given, and keeps the
  // statement reachable so a reader can see what was asked.
  if (rows.length === 0 || columns.length === 0) {
    return (
      <div style={containerStyle}>
        <TitleBar title={title} chartType={chartType} />
        <p style={{ margin: "14px 0 2px", fontSize: 13, lineHeight: 1.6, color: "var(--text-secondary)" }}>
          This answer carried no rows, so there is nothing to chart.
        </p>
        <Footer sql={sql} title={title} sqlOpen={sqlOpen} setSqlOpen={setSqlOpen} />
      </div>
    );
  }

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
        <Footer sql={sql} title={title} sqlOpen={sqlOpen} setSqlOpen={setSqlOpen} />
      </div>
    );
  }

  // Table
  if (chartType === "table") {
    const displayRows = rows.slice(0, TABLE_ROW_LIMIT);
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
        {/* The slice above was silent, so twenty rows of a thousand read as the
            whole answer. Say what was cut; the statement below reproduces the
            rest. */}
        {rows.length > displayRows.length && (
          <p style={{ margin: "8px 0 0", fontSize: 11.5, color: "var(--text-secondary)" }}>
            Showing the first {displayRows.length} of {rows.length.toLocaleString()} rows.
          </p>
        )}
        <Footer sql={sql} title={title} sqlOpen={sqlOpen} setSqlOpen={setSqlOpen} />
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
      <Footer sql={sql} title={title} sqlOpen={sqlOpen} setSqlOpen={setSqlOpen} />
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
          background: "var(--bg-elevated)",
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
  title,
  sqlOpen,
  setSqlOpen,
}: {
  sql?: string;
  title: string;
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
          type="button"
          onClick={() => setSqlOpen(!sqlOpen)}
          style={{
            background: "none",
            border: "none",
            cursor: "pointer",
            fontSize: 12,
            // Both controls in this row are `--text-secondary` rather than
            // `--text-muted`: on the card's `--bg-surface` ground muted
            // measures 4.40:1 in light, just under the 4.5:1 body-text
            // threshold, where secondary measures 7.34:1 and 6.38:1.
            color: "var(--text-secondary)",
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
        {/* The Lab reads `query`, not `sql` — this link had been handing it a
            parameter it ignores, so the statement never arrived and the Lab
            opened empty. `run` carries the rest of the intent: the answer on
            screen came from this statement, so arriving at a Run button you
            have to press again makes the hand-off a dead end. */}
        <a
          href={
            "/lab?query=" + encodeURIComponent(sql)
            + "&name=" + encodeURIComponent(title || "Chat query")
            + "&run=1"
          }
          style={{
            fontSize: 12,
            color: "var(--text-secondary)",
            textDecoration: "none",
          }}
          // The brightening was bound to the pointer alone, so a reader moving
          // by keyboard got none of it; focus now does what hover does. The
          // handlers read `currentTarget` so the anchor is what changes,
          // whatever the event started on.
          onMouseEnter={(e) => (e.currentTarget.style.color = "var(--text-primary)")}
          onMouseLeave={(e) => (e.currentTarget.style.color = "var(--text-secondary)")}
          onFocus={(e) => (e.currentTarget.style.color = "var(--text-primary)")}
          onBlur={(e) => (e.currentTarget.style.color = "var(--text-secondary)")}
        >
          Open in SQL Lab →
        </a>
      </div>
      {sqlOpen && (
        <pre
          style={{
            marginTop: 8,
            padding: "10px 12px",
            background: "var(--bg-elevated)",
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
