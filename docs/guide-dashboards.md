# Building Dashboards

Dashboards are collections of charts arranged on a canvas. They support drag-and-drop layout, cross-filtering, dark mode, and auto-refresh.

---

## Creating a Dashboard

1. Navigate to **Workspace → Dashboards** in the sidebar.
2. Click **+ New Dashboard**.
3. An empty canvas opens in edit mode.
4. Add charts using the sidebar (see [Adding Charts](#adding-charts)).
5. Arrange and resize tiles on the canvas.
6. Click **Save** to save as a draft, or **Publish** to make the dashboard visible to all users.

---

## Edit Mode vs View Mode

| Feature | Edit mode | View mode |
|---------|-----------|-----------|
| Drag tiles | Yes | No |
| Resize tiles | Yes | No |
| Add / remove charts | Yes | No |
| Cross-filter clicks | Yes | Yes |
| Auto-refresh | Yes | Yes |
| Chart interactions (zoom, tooltip) | Yes | Yes |

The builder enters edit mode automatically when you create or edit a dashboard. Published dashboards open in view mode for non-owners; the owner sees an **Edit** button to re-enter edit mode.

---

## Canvas Layout

The canvas is powered by [react-grid-layout v2](https://github.com/react-grid-layout/react-grid-layout) (`DashboardCanvas.tsx`).

Layout properties:

| Property | Value |
|----------|-------|
| Grid columns | 12 (desktop), 6 (tablet), 2 (mobile), 1 (small mobile) |
| Default tile size | 6 cols × 4 rows |
| Vertical compaction | Enabled — tiles fall to fill gaps |
| Responsive reflow | Auto at breakpoints |

Each tile in the layout has an `i` (unique id), `x`, `y`, `w`, `h`. These are persisted in the dashboard JSON.

### Dragging

Click and hold any tile to drag it. Other tiles slide out of the way automatically.

### Resizing

Drag the resize handle (bottom-right corner, south edge, or east edge) to resize width and height independently.

---

## Adding Charts

In edit mode, click **Add Chart** in the sidebar or the empty state button. A chart picker modal opens showing all saved charts. You can:

- Search by chart name
- Filter by owner (`Mine` / `All`)
- Pick multiple charts at once

Selected charts are added as new tiles at the bottom of the canvas.

### Text blocks

Click **Add Text** in the sidebar to insert a markdown text block. Use text blocks for section headers, annotations, or static narrative.

---

## Dashboard Sidebar Tabs

| Tab | Contents |
|-----|----------|
| **Items** | Chart picker, text block button |
| **Layout** | Canvas settings, theme selector |
| **Filters** | Dashboard-level filter configuration |

---

## Filters

The **Filters** tab in edit mode lets you add dashboard-level filters. A filter bar appears above the canvas in both edit and view mode.

Filters are applied to all charts that share a matching column. Clicking a filter chip narrows every chart simultaneously.

### Cross-filtering

Clicking a data point in a chart (a bar, a slice, a map marker) sets a filter on that dimension value. All other charts on the canvas that query the same column update in real time.

Cross-filtering is additive — clicking multiple values in different charts stacks the filters.

---

## Publishing

A dashboard starts as a draft — visible only to you. When ready:

1. Click **Publish** in the toolbar.
2. Confirm the publish dialog.
3. The dashboard is now visible to all authenticated users with the appropriate role.

To unpublish, open the dashboard settings and set status back to draft.

---

## Auto-Refresh

Each chart tile can be configured to refresh its data automatically. Available intervals: 30 s, 1 min, 5 min, 15 min, 30 min, 1 hr.

When an interval is set, the tile re-runs its underlying SQL query against the live data source. The chart updates in place without a full page reload.

Auto-refresh is disabled in edit mode to avoid layout disruption while dragging.

---

## Dashboard Properties

The **Properties** panel (accessible via the settings icon) exposes:

| Property | Description |
|----------|-------------|
| Name | Dashboard title |
| Description | Optional subtitle shown in the list view |
| Theme | Light / Dark / System (follows OS preference) |
| Slug | URL-friendly identifier (auto-generated, editable) |

---

## Thumbnails

After a dashboard renders in view mode, a thumbnail is automatically captured and stored. Thumbnails appear in the dashboard list card view. They update each time the dashboard is opened in view mode.

---

## Dark Mode

Dashboard dark mode is controlled by the theme selector in the sidebar Layout tab. Setting a theme on a dashboard overrides the user's global preference for that dashboard.

ECharts charts inside dashboard tiles respect the dashboard theme via the same `applyChartTheme(option, isDark)` call used in standalone charts.

The CSS cascade for dashboard dark mode is defined in `styles/dashboard.css`. Key selectors:

```css
.dashboard-dark .dashboard-tile { background: var(--bg-surface); }
.dashboard-dark .chart-title    { color: var(--text-primary); }
```

---

## State Management

Dashboard state lives in `DashboardContext` (`components/dashboards/DashboardContext.tsx`). `DashboardBuilder` is the top-level component that orchestrates the sidebar, canvas, filter bar, and save/publish actions.

Key context values:

```typescript
layout          // Array of LayoutItem (id, x, y, w, h, type, componentId)
setLayout       // Setter used by react-grid-layout drag/resize callbacks
isEditMode      // Boolean
addLayoutItem   // Add a chart or text tile
saveDashboard   // POST/PATCH to /api/v1/dashboards/:id
theme           // "light" | "dark" | "system"
hasUnsavedChanges
```

---

## Legacy Dashboards

Dashboards built before the react-grid-layout migration used nested row/column containers. `DashboardCanvas` detects these by checking `hasContainers` (any tile with `type === "row"` or `type === "column"`). Legacy dashboards render in the original vertical stack layout and are not editable in the new canvas — they display correctly in view mode.
