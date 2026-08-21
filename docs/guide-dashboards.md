# Building Dashboards

Dashboards are collections of charts and layout components arranged on a canvas. They support drag-and-drop layout, DLM-powered filters with auto-discovery, cross-filtering, color palette themes, and auto-refresh.

---

## Creating a Dashboard

1. Navigate to **Workspace → Dashboards** in the sidebar.
2. Click **+ New Dashboard**.
3. An empty canvas opens in edit mode.
4. Add charts from the **Items** sidebar tab, or add layout components from the **Layout** tab.
5. Arrange and resize tiles on the canvas.
6. Click **Save** to persist the dashboard, or **Publish** to make it visible to all users.

---

## Edit Mode vs View Mode

| Feature | Edit mode | View mode |
|---------|-----------|-----------|
| Drag tiles | Yes | No |
| Resize tiles | Yes | No |
| Add / remove components | Yes | No |
| Cross-filter clicks | Yes | Yes |
| Auto-refresh | No | Yes |
| Chart actions overlay | Yes | Yes |
| Filter bar | Yes (full editor) | Yes (value editing only) |
| Color palette picker | Yes | No |

The builder enters edit mode automatically when you create or edit a dashboard. Published dashboards open in view mode for all users; the owner sees an **Edit** button to re-enter edit mode.

---

## Canvas Layout

The canvas is powered by [react-grid-layout](https://github.com/react-grid-layout/react-grid-layout) (`DashboardCanvas.tsx`).

Layout properties:

| Property | Value |
|----------|-------|
| Grid columns | 12 |
| Row height | 30 px |
| Margin | 16 px (horizontal and vertical) |
| Container padding | 16 px |
| Default chart tile size | 6 cols × 8 rows |
| Vertical compaction | Enabled — tiles fall to fill gaps |

Each tile in the layout has an `i` (unique id), `x`, `y`, `w`, `h`, and `type`. These are persisted in the dashboard JSON.

### Dragging

Click and hold any tile to drag it. Other tiles slide out of the way automatically.

### Resizing

Drag the resize handle (bottom-right corner, south edge, or east edge) to resize width and height independently. Each component type has minimum and maximum dimension constraints.

---

## Component Types

Dashboards support 7 component types:

| Type | Description | Default size | Min size |
|------|-------------|-------------|----------|
| `chart` | A saved chart rendered with ECharts | 6 × 8 | 2 × 3 |
| `text` | Markdown text block for annotations | 4 × 3 | 2 × 2 |
| `header` | Section header with configurable size/alignment | 12 × 1 | 2 × 1 |
| `divider` | Horizontal or vertical separator line | 12 × 1 | 1 × 1 |
| `row` | Container that arranges children horizontally | 12 × 4 | 4 × 2 |
| `column` | Container that arranges children vertically | 3 × 8 | 2 × 4 |
| `tabs` | Tabbed container with independent layouts per tab | 8 × 12 | 4 × 8 |

### Adding Charts

In edit mode, the **Items** sidebar tab lists all saved charts. You can:

- Search by chart name, type, or dataset
- Filter by owner (`Mine only` toggle)
- Click a chart to add it as a new tile at the bottom of the canvas

There is also a full-screen chart picker modal (via container-targeted "Add chart" buttons) with a tile grid view.

### Adding Layout Components

The **Layout** sidebar tab provides buttons to add: Row, Column, Header, Text Block, and Divider.

### Nesting Rules

- Rows can contain charts, text, headers, dividers, columns, and tabs — but not other rows.
- Columns can contain charts, text, headers, dividers, and tabs — but not rows or other columns.
- Maximum nesting depth is 2 levels.
- When children are added to a row, the 12-column grid is redistributed evenly among all children.

---

## Dashboard Sidebar Tabs

| Tab | Contents |
|-----|----------|
| **Items** | Chart picker with search/filter, inline chart list |
| **Layout** | Layout component buttons (row, column, header, text, divider), canvas settings |
| **Filters** | Dashboard-level filter configuration (`DashboardFilterBarEnhanced`) |

---

## Filters

### DLM-Powered Filter Bar

The filter bar appears above the canvas in both edit and view mode. In edit mode, the **Filters** tab provides a full editor for adding and configuring filters. In view mode, a read-only filter bar (`DashboardFilterBarReadOnly`) allows changing filter values but not adding or removing filters.

### Auto-Discovery

When a dashboard has no saved filters, the view page automatically discovers filters from dataset dimensions. It inspects all charts on the dashboard, fetches their dataset columns, and creates filters for every dimension column and date/time column. Dimension filters default to `AllUp`; date columns become date-range filters.

### Filter Value Loading

When a filter chip is clicked, distinct values are fetched from the `/api/v1/dlm/filter-values` endpoint (primary path). If that endpoint returns no results, it falls back to `/api/v1/sql/distinct-filter-values`. Values are displayed as a searchable multi-select checkbox dropdown.

### Multi-Select

Selecting multiple values automatically switches the operator to `IN`. The chip label shows `{name}: {count} selected` when more than one value is chosen. Single selections use `=`.

### Cascading Filters

Filters on the same dataset narrow each other. When you set a value on one filter, the options for sibling filters are re-fetched with the current selection as a narrowing constraint. The narrowing signature is tracked so options are only refetched when the upstream selection changes.

### AND/OR Logic Toggle

Filter logic is configurable as `AND` or `OR` via `filterLogic` in the dashboard context. This controls how multiple active filters combine when resolving effective filters for each chart.

### Date Range Filters

Date filters expand into two filter configs (`>=` from-date and `<=` to-date) so downstream chart execution needs no special handling.

### Click-Outside-to-Close

The filter popover closes when clicking outside both the popover and the filter bar.

### AllUp Defaults

Auto-discovered dimension filters default to `AllUp`. Filters with an empty value are skipped during resolution to avoid wiping out all data.

### Per-Chart Filter Overrides

Each chart tile can have its own component-level filters that override dashboard filters on the same column. Charts can also ignore specific dashboard filters via an ignore list. Both are managed through the chart actions overlay in edit mode.

---

## Cross-Filtering

Clicking a data point in a chart (a bar, a slice, a map marker) sets a cross-filter on that dimension value. All other charts on the canvas update in real time with the cross-filter applied.

Cross-filtering is additive — clicking values in different charts stacks the filters. Each chart excludes cross-filters that originated from itself to avoid circular filtering. Cross-filters can be cleared individually per source chart or all at once.

---

## Chart Rendering

### Context-Powered Rendering

When a chart runs inside a dashboard, it first tries the DLM context path (`/api/v1/dlm/serve-chart`). This serves single-metric breakdowns instantly from precomputed `dlm_answers` with zero database trip. The chart builder detects it is in a dashboard context (`runContext` starts with `"dashboard"`) and posts the dataset, metric, group-by, and active filters to the serve-chart endpoint. If the endpoint returns `served: true`, the chart renders immediately from the response.

If serve-chart does not return a result (multi-metric, unsupported aggregation, etc.), the chart falls back to generating and executing SQL against the live database.

### Client-Side Query Cache

A module-level `Map` cache (`_CLIENT_CACHE`) stores query results keyed by a hash of database + SQL. The cache has a 5-minute TTL (matching the server cache) and a 200-entry cap with LRU eviction. Dashboard charts check this cache before hitting the API, making repeat views instant. Context-served results are also cached.

### Parallel Chart Preloading

On dashboard load, `preloadAllCharts` fetches all chart configurations in parallel using `Promise.all`. The canvas does not render until all configs are cached, so each chart mounts once with its config already available and runs exactly one query instead of flashing through multiple loading states.

### Query Semaphore

A global semaphore (`querySemaphore.ts`) limits concurrent dashboard chart queries to 6 (tuned for Azure Postgres's connection pool). Charts beyond the limit queue and execute as slots free up.

---

## Chart Actions Overlay

Every chart tile has a `...` button (top-right corner) that opens an actions menu with:

| Action | Description |
|--------|-------------|
| View chart | Navigate to the standalone chart page |
| Refresh | Re-run this chart's query |
| Full screen | Open the chart in a full-screen modal with its own filter sandbox |
| Edit chart | (edit mode only) Navigate to the chart editor |
| Chart filters | (edit mode only) Add per-chart filters or ignore dashboard filters |
| View query | Show the generated SQL in a modal |
| View as table | Show the raw data rows in a table modal |
| Download CSV | Export the chart data as CSV |
| Download PNG | Export the chart as a PNG image |
| Share dashboard | Copy the dashboard URL to clipboard |
| Duplicate | (edit mode only) Duplicate this tile |
| Remove | (edit mode only) Remove this tile (with confirmation dialog) |

### Full-Screen Sandbox

The full-screen view includes its own copy of the dashboard filters. Changing filters in full screen only affects the isolated chart — the dashboard is unaffected. The sandbox uses a separate `ChartHydrator` instance and resolves filters locally via `resolveEffectiveFilters`.

---

## Publishing

A dashboard starts as a draft — visible only to you. When ready:

1. Click **Publish** in the toolbar.
2. The dashboard is immediately visible to all authenticated users with the appropriate role.

To unpublish, update the dashboard's `is_published` field back to `false`.

---

## Auto-Refresh

In view mode, a refresh control group in the header toolbar provides:

- A manual refresh button that triggers `globalRefreshTick` (all charts re-run their queries).
- A dropdown to set an auto-refresh interval: Off, 30s, 1m, 5m, 10m, 30m.

When an interval is set, all charts on the dashboard refresh simultaneously at that cadence. The last refresh timestamp is shown next to the dropdown.

Auto-refresh is available only in view mode.

---

## Dashboard Properties

| Property | Description |
|----------|-------------|
| Name | Dashboard title (1–255 characters) |
| Description | Optional subtitle (max 1000 characters), shown in the header and list view |
| Theme | Color palette — see [Color Palette Themes](#color-palette-themes) |

---

## Color Palette Themes

The theme selector is a color palette picker in the edit-mode toolbar. It recolors all charts on the dashboard simultaneously. Available palettes:

| Key | Label | Description |
|-----|-------|-------------|
| `default` | Default | Each chart keeps its own colors |
| `vibrant` | Vibrant | Indigo, pink, teal, amber, violet, cyan, red, emerald, orange, blue |
| `ocean` | Ocean | Sky blue, dark blue, cyan, blue, indigo, light blue |
| `sunset` | Sunset | Amber, red, pink, orange, rose, light orange |
| `forest` | Forest | Emerald, teal, lime, green, dark green |
| `slate` | Slate | Dark slate, sky blue, gray, light blue |

The selected palette is applied to all ECharts chart instances via `themePalette` from the dashboard context.

---

## Thumbnails

After a dashboard renders in view mode, a thumbnail is captured client-side using `html-to-image` (`toJpeg`). The capture waits until all chart queries have finished (tracked via the query semaphore), then runs during an idle callback to avoid blocking interaction.

Thumbnails are stored in theme-specific slots (`thumbnail` for light, `thumbnail_dark` for dark) so the dashboard list can show a preview matching the viewer's theme. If the user switches themes while viewing, a fresh capture is taken for the new theme.

The capture uses low quality (0.55) and pixel ratio (0.4) to keep the data URL under 3.5 MB. If oversized, it retries at even lower settings.

---

## State Management

Dashboard state lives in `DashboardContext` (`components/dashboards/DashboardContext.tsx`). `DashboardBuilder` is the top-level component that orchestrates the sidebar, canvas, filter bar, and save actions.

Key context values:

```typescript
// Metadata
dashboardId, name, description

// Layout
layout              // Array of DashboardLayoutItem (i, x, y, w, h, type, chartId, children, ...)
setLayout           // Setter used by react-grid-layout drag/resize callbacks
isEditMode          // Boolean
addLayoutItem       // Add any component type
updateLayoutItem    // Update a tile's properties (recursive for nested items)
removeLayoutItem    // Remove a tile
duplicateLayoutItem // Clone a tile

// Nested containers
findItem, getItemDepth, addItemToContainer, removeItemFromContainer
moveItemToContainer, moveItemToRoot

// Filters
dashboardFilters    // Array of DashboardFilter
filterLogic         // "AND" | "OR"
getEffectiveFilters // Merges dashboard + component + cross-filters for a tile

// Cross-filtering
crossFilters, setCrossFilter, clearCrossFilter, clearAllCrossFilters

// Theme
theme               // Palette key ("default", "vibrant", "ocean", "sunset", "forest", "slate")
setTheme
themePalette        // Resolved color array (or null for default)

// Preloading
preloadAllCharts    // Fetch all chart configs in parallel
chartConfigCache    // Map<chartId, config>
getChartConfig      // Retrieve a cached config

// Refresh
globalRefreshTick   // Incremented to trigger all charts to re-query
triggerGlobalRefresh

// Persistence
saveDashboard       // PUT to /api/v1/dashboards/:id
saveDashboardAs     // POST to /api/v1/dashboards (creates a copy)
hasUnsavedChanges   // Change tracking via snapshot comparison
```

---

## API Endpoints

All endpoints are in `routers/dashboards.py` under `/api/v1/dashboards`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/dashboards` | List all dashboards (filtered by role) |
| `GET` | `/dashboards/summary` | Dashboard count + list |
| `GET` | `/dashboards/{id}` | Get a single dashboard (includes `is_favorite`) |
| `POST` | `/dashboards` | Create a new dashboard (requires Analyst role) |
| `PUT` | `/dashboards/{id}` | Update a dashboard (owner or admin only) |
| `DELETE` | `/dashboards/{id}` | Delete a dashboard and purge from recents |
| `PUT` | `/dashboards/{id}/favorite` | Toggle favorite status |

### Permissions

- Creating a dashboard requires the `Analyst` role minimum.
- Updating or deleting requires ownership (`can_write` check) or admin role.
- Publishing requires `can_publish` permission; without it, visibility falls back to `internal`.

---

## Save As

In edit mode, the save modal supports **Save As** — creating a new dashboard copy with a different name. The copy gets a new ID and opens in the editor. The original dashboard is unchanged.
