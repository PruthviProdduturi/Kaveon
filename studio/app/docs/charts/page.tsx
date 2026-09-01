import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Chart Builder" };

export default function ChartsDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Features"
        title="Chart Builder"
        lead="Charts are reusable visualizations built on top of a dataset. Save one and embed it in any number of dashboards, or share it as a standalone view. 37 built-in types, all interactive and themeable."
      />

      <h2>Chart types</h2>
      <p>
        Kaveon ships <strong>37 chart types across 18 categories</strong>, powered by{" "}
        <a href="https://echarts.apache.org/" target="_blank" rel="noreferrer">Apache ECharts</a> via
        <code>echarts-for-react</code> — including the world map, which is rendered via ECharts plus the
        <code> echarts-gl</code> extension on the same ECharts instance.
      </p>
      <table>
        <thead><tr><th>Category</th><th>Types</th></tr></thead>
        <tbody>
          <tr><td><strong>Line</strong></td><td>Time-series line (+ share), multi-series line, area (+ share), stacked area</td></tr>
          <tr><td><strong>Bar</strong></td><td>Vertical, horizontal, grouped, stacked (+ horizontal), mixed line+bar, waterfall</td></tr>
          <tr><td><strong>Pie</strong></td><td>Pie, donut</td></tr>
          <tr><td><strong>Distribution</strong></td><td>Scatter, bubble, heatmap, calendar heatmap, boxplot</td></tr>
          <tr><td><strong>Hierarchy</strong></td><td>Treemap, sunburst, funnel, radar, gauge</td></tr>
          <tr><td><strong>Flow</strong></td><td>Sankey, parallel coordinates, theme river</td></tr>
          <tr><td><strong>Map</strong></td><td>World map (echarts-gl globe)</td></tr>
          <tr><td><strong>Custom / Table</strong></td><td>Big number (+ trend), candlestick, pictorial bar, table, pivot table</td></tr>
        </tbody>
      </table>
      <Callout type="note">
        Types are registered in <code>TEMPLATES</code> in <code>components/charts/ChartBuilderContext.tsx</code>.
        New types can be added through the plugin registry without editing that file.
      </Callout>

      <h2>Creating a chart</h2>
      <ol>
        <li>Go to <strong>Charts → + New Chart</strong>.</li>
        <li>Pick a <strong>dataset</strong> — it determines the data source and schema.</li>
        <li>Choose a <strong>chart type</strong> from the picker (categories left, thumbnails right, searchable).</li>
        <li>In the <strong>Configure</strong> tab, drag dimensions onto the X / group-by slots and metrics onto the Y slots; add filters and a date range.</li>
        <li>Click <strong>Run</strong> to preview.</li>
        <li>Tune the <strong>Customize</strong> tab (colors, labels, axes, legend), then <strong>Save Chart</strong>.</li>
      </ol>

      <h2>The builder interface</h2>
      <h3>Column browser</h3>
      <p>
        Lists every column and metric from the dataset, tagged as dimensions (string, date) or measures (numeric).
        Drag a column into a slot, or click to assign it.
      </p>
      <h3>Customize options</h3>
      <table>
        <thead><tr><th>Option</th><th>What it controls</th></tr></thead>
        <tbody>
          <tr><td>Title</td><td>Text, font, size</td></tr>
          <tr><td>Color palette</td><td>12-color default or a custom hex list</td></tr>
          <tr><td>Legend / Tooltip</td><td>Visibility, position, number format, decimals, separators</td></tr>
          <tr><td>X / Y axis</td><td>Labels, visibility, date &amp; number formats</td></tr>
          <tr><td>Series</td><td>Smooth lines, markers, stacking</td></tr>
          <tr><td>Show as percentage</td><td>Normalize stacked charts to 100%</td></tr>
        </tbody>
      </table>

      <h2>ECharts integration</h2>
      <p>
        Charts render client-side with <code>ReactECharts</code>. Before an option object is handed to ECharts, it&rsquo;s
        run through <code>applyChartTheme</code> (<code>utils/echartsTheme.ts</code>), which injects theme-aware colors
        for text, legend, tooltip, axes, and split lines — so every chart respects light/dark mode automatically.
      </p>
      <Code lang="tsx">{`import { applyChartTheme } from "../../utils/echartsTheme";

const option = applyChartTheme(buildEChartsOptions(/* … */), isDark);
return <ReactECharts option={option} style={{ height: 400 }} notMerge lazyUpdate />;`}</Code>

      <Pager
        prev={{ href: "/docs/freshness", title: "Freshness Algorithm" }}
        next={{ href: "/docs/dashboards", title: "Dashboards" }}
      />
    </div>
  );
}
