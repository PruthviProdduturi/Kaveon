# Building Charts

Charts are reusable visualizations built on top of a dataset. Once saved, a chart can be embedded in any number of dashboards or shared as a standalone view.

---

## Chart Types

Kaveon ships 37 built-in chart types across 18 categories. All are powered by [Apache ECharts](https://echarts.apache.org/) via `echarts-for-react`, except the world map which uses a custom WebGL globe renderer.

| Category | Chart types |
|----------|-------------|
| **Line** | Time-series line, Time-series line share, Multi-series line, Time-series area, Time-series area share, Stacked area |
| **Bar** | Vertical bar, Horizontal bar, Grouped bar, Stacked bar, Stacked horizontal bar, Mixed line + bar, Waterfall, Histogram |
| **Pie** | Pie chart, Donut chart, Nightingale rose |
| **Scatter** | Scatter, Bubble |
| **Heatmap** | Heatmap, Calendar heatmap |
| **Treemap** | Treemap |
| **Sunburst** | Sunburst |
| **Funnel** | Funnel |
| **Radar** | Radar |
| **Gauge** | Gauge |
| **Boxplot** | Boxplot |
| **Candlestick** | Candlestick |
| **Pictorial** | Pictorial bar |
| **Stream** | Theme river |
| **Flow** | Sankey |
| **Map** | World map (WebGL globe) |
| **Custom** | Big number, Big number with trend, Parallel coordinates |
| **Table** | Table, Pivot table |

Chart types are registered in `TEMPLATES` in `components/charts/ChartBuilderContext.tsx`. Custom chart types can be added via the plugin registry without modifying that file — see [Extending with Plugins](#extending-with-plugins).

---

## Creating a Chart

1. Navigate to **Workspace → Charts** in the sidebar.
2. Click **+ New Chart**.
3. Select a **dataset** — the data source and schema are determined by the dataset.
4. Pick a **chart type** from the picker. Categories are shown on the left; chart thumbnails (where available) are shown on the right.
5. Configure the chart in the **Configure** tab:
   - Select dimension columns (X axis, group by)
   - Select metrics (Y axis)
   - Apply filters and date range
6. Click **Run** to preview the result.
7. Adjust appearance in the **Customize** tab (colors, labels, axes, legend).
8. Click **Save Chart** and give it a name.

---

## Chart Builder Interface

### Dataset Selector (`DatasetSelector.tsx`)

Shows available datasets. Changing the dataset resets column and metric selections. Displays the dataset's fact table, schema, and database.

### Chart Type Picker (`ChartTypePicker.tsx`)

Organized by category. Each entry shows a name, description, and an inline SVG icon rendered by the `ChartIcon` component. The search box filters all chart types by name or description.

### Column Browser (`ColumnBrowser.tsx`)

Lists all columns and metrics from the selected dataset. Columns are tagged as dimensions (string, date) or measures (numeric). Drag a column into a dimension slot, or click to assign it.

### Advanced Options (`AdvancedChartOptions.tsx`)

The **Customize** tab exposes:

| Option | Description |
|--------|-------------|
| Title | Chart title, font, size |
| Color palette | 12-color default or custom hex list |
| Legend | Show/hide, position, series order |
| Tooltip | Number format, decimal places, comma separators |
| X / Y axis | Label, show/hide, date format, number format |
| Series settings | Smooth lines, markers, stacking |
| Show as percentage | Normalize stacked charts to 100% |
| Chart-type options | Type-specific controls (pie hole size, KPI format, etc.) |

Default color palette:
```
#6366f1  #ec4899  #14b8a6  #f59e0b  #8b5cf6  #06b6d4
#ef4444  #10b981  #f97316  #3b82f6  #a855f7  #84cc16
```

---

## ECharts Integration

Charts are rendered with `echarts-for-react` (`ReactECharts`). All rendering is client-side (`"use client"`).

### Theme application

Before passing an ECharts option object to `ReactECharts`, call `applyChartTheme` from `utils/echartsTheme.ts`:

```typescript
import { applyChartTheme } from "../../utils/echartsTheme";

const rawOption = buildEChartsOptions(/* ... */);
const option = applyChartTheme(rawOption, isDark);

return <ReactECharts option={option} style={{ height: 400 }} notMerge lazyUpdate />;
```

`applyChartTheme` merges the following into any ECharts option:

```typescript
// Dark mode values (isDark = true)
backgroundColor: "transparent"
textStyle.color:    "#e2e8f0"
legend.textStyle:   "#64748b"
tooltip.background: "#1a1a2e"
tooltip.border:     "rgba(255,255,255,0.06)"
xAxis.axisLine:     "rgba(255,255,255,0.06)"
xAxis.axisLabel:    "#64748b"
xAxis.splitLine:    "rgba(255,255,255,0.06)"
// (and equivalent light-mode values when isDark = false)
```

It handles both single-axis and array-axis (`xAxis: [...]`) options, and applies `axisName`, `axisLine`, and `splitLine` to radar charts.

### SQL generation

The chart builder does not require the user to write SQL. It builds a query from:
- The dataset's fact table (`schema.table`)
- Selected dimension columns
- Selected metrics (pre-defined expressions like `SUM(revenue)` or column aggregations)
- Applied filters and date ranges
- `LIMIT`, `ORDER BY`, `GROUP BY` clauses

The generated SQL is previewed in the **SQL** tab of the chart builder before execution.

---

## Inline Charts in Chat

When the homepage NL→SQL engine generates a query, the result is rendered as an inline chart inside the conversation using `InlineChart` from `components/chat/InlineChart.tsx`.

Inline charts support five types: `bar`, `line`, `pie`, `kpi`, and `table`. They are intentionally lightweight — no axis configuration, no filters, no drill-down. Their purpose is a quick visual answer in context.

Each inline chart shows:
- A title and chart type badge
- The ECharts visualization (280 px tall)
- An expandable SQL footer
- An "Open in SQL Lab →" link

The brand color `#4A9EE8` is used as the primary series color in inline charts.

---

## Dark Mode

Dark mode is driven by CSS variables set on the `<html>` element by `ThemeContext`. ECharts charts use `applyChartTheme(option, isDark)` to receive the correct colors at render time.

The `isDark` flag comes from `useTheme()`:

```typescript
const { theme } = useTheme();
const isDark = theme === "dark";
const option = applyChartTheme(rawOption, isDark);
```

When the user toggles the theme, charts re-render because `ReactECharts` receives a new `option` with `notMerge` set to `true`.

---

## DLM-Powered Chart Serving

Dashboard charts attempt to resolve data through the DLM (Data Language Model) before falling back to raw SQL execution. When a chart is rendered on a dashboard, the frontend issues `POST /dlm/serve-chart` with the chart's metric and dimension configuration.

Two request shapes are supported:

- **Single-metric** — sends `metric_column`, `aggregation`, and `group_by` fields.
- **Multi-metric** — sends a `metrics` array, each entry containing its own column and aggregation.

If the DLM can answer from its learned context, the response is returned directly. If the context is insufficient, the endpoint returns a fallback signal and the chart runner issues the generated SQL query instead. This path keeps dashboard loads fast for common metrics while preserving full SQL flexibility for complex or ad-hoc charts.

---

## Client-Side Query Cache

Chart query results are cached in the browser using a SHA-based content-addressed cache. The cache key is derived from the SQL text and parameters. Entries expire after a 5-minute TTL and the cache holds a maximum of 200 entries, evicting least-recently-used entries when full. This avoids redundant server round-trips when switching between dashboard tabs or re-rendering charts that share the same underlying query.

---

## Cross-Filtering

`ChartPreview` accepts an `onCrossFilter` callback prop. When a user clicks a data point in a chart, the callback fires with the selected dimension value, allowing parent components (such as dashboards) to propagate the selection as a filter to other charts on the same page.

---

## Number Formatting

Numeric values in charts are abbreviated using K/M/B/T formatting (e.g., 1,500 becomes "1.5K", 2,300,000 becomes "2.3M"). This applies to axis labels, tooltips, and KPI displays.

---

## Chart Downloads

From the chart view, users can download the current visualization as a **PNG** image or export the underlying data as a **CSV** file.

---

## Extending with Plugins

New chart types can be registered at module load time without editing the built-in template list.

```typescript
import { registerChartPlugin } from "./chartPluginRegistry";

registerChartPlugin({
  id: "waterfall_custom",
  name: "Custom Waterfall",
  description: "Cumulative bar chart with running totals.",
  category: "Bar",         // must match an existing CHART_CATEGORIES id
  previewKind: "bar",
  buildOptions({ rows, columns, advancedOptions }) {
    // Return an ECharts option object, or null to show empty placeholder.
    return { /* ... */ };
  },
  // Optional: replace ReactECharts with a custom React component
  Renderer: MyCustomRenderer,
  // Optional: additional controls in the Customize tab
  CustomizePanel: MyCustomizePanel,
});
```

Plugin ids must not clash with the built-in `TEMPLATES` ids. If a plugin registers with the same id as a built-in, it replaces the built-in.

The chart builder merges plugins into the available template list in `ChartBuilderContext.tsx`:

```typescript
const allTemplates = [...TEMPLATES.filter(t => !pluginIds.has(t.id)), ...plugins];
```
