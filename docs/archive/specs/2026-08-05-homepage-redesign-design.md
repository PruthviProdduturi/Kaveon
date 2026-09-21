# Kaveon Homepage & Layout Redesign — Design Spec

**Date:** 2026-08-05
**Status:** Approved
**Mockup:** `.superpowers/brainstorm/43640-1785971542/content/homepage-rich-v2.html`

---

## Overview

Kaveon's identity shifted from "self-hosted analytics platform" (Superset clone) to "Talk to your data" (conversational data product). The homepage, layout, and About page need to reflect this. The current top-nav + stats-grid homepage is wrong for a product where the primary interaction is a conversation.

## Brand Assets (Finalized)

- **Logo mark:** Open arc (~280deg) with chat bubble tail at bottom-left gap + center dot
- **Primary color:** #4A9EE8
- **Wordmark:** KAVE + O-mark + N (geometric sans, crossbar-less A)
- **Tagline:** "Talk to your data."
- **Hero text:** "Your data has answers."
- **Sidebar collapsed state:** O mark only, with breathing glow animation

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Layout paradigm | Sidebar-owns-navigation (Option C) | Modern app pattern (VS Code, Linear, Teams). Scales to future features. Center area sacred for primary interaction. |
| Homepage hero | Chat input front and center | Tagline is "Talk to your data" — if the first screen doesn't invite conversation, the brand is lying. |
| Sidebar content | Nav + search + pinned + recent + user profile | Full navigation without a top header bar. Collapsible to icon rail. |
| Sidebar collapse | Icon rail (64px) with tooltips | Always one-click access to nav. O mark stays visible with breathing glow. |
| Suggestion chips | Schema-generated, 4 chips | Reduces blank-page problem. Demonstrates capability. Must come from real data. |
| Theme system | Light + dark only. No color picker. | One brand color (#4A9EE8). Arbitrary themes destroy brand recognition. |
| Theme toggle | SVG line icons (sun/crescent) | Clean, high-end. No emoji. |
| Old homepage content | Moves to `/workspace` | Dashboards, charts, datasets, saved queries, activity — still accessible, not the landing. |
| Workspace layout | Horizontal tabs | Dashboards \| Charts \| Datasets \| Saved Queries. Fast, scannable, familiar mental model. |
| About page | Stripped down (Option B) | Hero + 3-4 feature highlights + CTA. Kill comparison tables, RBAC breakdown, user journeys. Move to docs. |
| Hero text | "Your data has answers." | Confident, positions data as protagonist, tells user what to expect. |

---

## 1. Global Layout — Sidebar Navigation

**Replaces:** Current top header nav (`components/Layout.tsx`)

### Expanded State (260px)

```
┌──────────────────────────┐
│ [O] Kaveon               │  ← Logo mark + wordmark
├──────────────────────────┤
│ ⌕ Search everything...   │  ← Global search (Cmd+K)
├──────────────────────────┤
│ ▌💬 Chat           NEW   │  ← Active indicator (left bar)
│   📊 Workspace           │
│   ⌨️  SQL Lab             │
│   🔌 Data Sources        │
│   ⚙️  Settings            │
├──────────────────────────┤
│ PINNED                   │
│ ● Revenue Dashboard      │  ← Color-coded dots
│ ● Monthly Trends         │
│ ● Customer Segments      │
│                          │
│ RECENT                   │
│ ● Sales by Region        │
│ ● Q3 Pipeline Analysis   │
│ ● Churn Prediction Query │
│ ● Top Products Chart     │
│ ● Weekly Active Users    │
├──────────────────────────┤
│ [PP] Pruthvi Prodduturi  │  ← Avatar + name + role
│      Admin               │
└──────────────────────────┘
```

### Collapsed State (64px)

```
┌────────┐
│  [O]   │  ← O mark only, breathing glow animation
├────────┤
│  ⌕     │
├────────┤
│  💬    │  ← Tooltip on hover: "Chat"
│  📊    │  ← Tooltip on hover: "Workspace"
│  ⌨️    │
│  🔌    │
│  ⚙️    │
├────────┤
│  ●     │  ← Dots only (pinned)
│  ●     │
│  ●     │
│        │
│  ●     │  ← Dots only (recent)
│  ●     │
│  ●     │
├────────┤
│  [PP]  │  ← Avatar only
└────────┘
```

### Behavior
- Toggle via chevron button on sidebar edge (`‹` / `›`)
- Keyboard shortcut: `Cmd+\`
- State persists in localStorage
- Transition: 250ms cubic-bezier(0.4, 0, 0.2, 1)
- Collapsed: logo scales up 1.15x, breathing glow animation (3s ease-in-out infinite)
- Collapsed nav items show tooltip on hover (dark pill, positioned right of icon)

### Styling
- **Light mode:** white background (#ffffff), #eaecf0 borders, #5a6577 text
- **Dark mode:** #0c0c0f background, rgba(255,255,255,0.06) borders, #94a3b8 text
- Active nav item: left blue bar (3px), blue tinted background, blue text
- Pinned/Recent items: 6px color-coded dots (blue, emerald, amber, purple)
- User avatar: gradient blue rounded square with initials

---

## 2. Homepage (`/`)

**Replaces:** Current `app/page.tsx` (1058 lines of stats grid, favorites, activity)

### Layout

```
┌─────────┬──────────────────────────────────────────┐
│         │                                          │
│         │           [O watermark, faded]           │
│         │                                          │
│ SIDEBAR │     Your data has answers.               │
│         │     3 sources · 847 tables · 12 datasets │
│         │                                          │
│         │  ┌──────────────────────────────────┐↑│  │
│         │  │ Ask anything about your data...  │  │  │
│         │  └──────────────────────────────────┘  │  │
│         │                                          │
│         │  [📊 Total sales?] [📈 Revenue trend?]   │
│         │  [🏆 Top 10 customers?] [🔍 Q2 vs Q3?]  │
│         │                                          │
│         │     ● Fabric SQL  ● PostgreSQL  ● Azure  │
│         │                                          │
│         │         ⌘K search · ⌘J SQL Lab           │
└─────────┴──────────────────────────────────────────┘
```

### Components

**Hero Mark (watermark)**
- Kaveon O mark SVG, centered, 52px
- Opacity: 0.25 (light), 0.35 (dark)
- No animation on the hero mark (sidebar has the animation)

**Hero Text**
- "Your data has answers." — 28px, font-weight 600
- Light: #111827. Dark: #f0f0f2.

**Contextual Meta**
- Dynamic: "{n} sources connected · {n} tables · {n} datasets"
- Pulled from existing API endpoints (data sources list, metadata)
- 13px, muted color, dot separators

**Chat Input**
- Max-width: 640px, centered
- 14px, 16px padding, 14px border-radius
- Focus state: blue border glow + subtle shadow
- Send button: 36px blue gradient square, right-aligned inside input
- On submit: navigate to `/ai` with the query pre-filled (or open chat inline — TBD with inline chart rendering workstream)

**Suggestion Chips**
- 4 chips, flex-wrap, centered, max-width 640px
- Generated from real schema: scan connected data sources, pick common tables/columns, generate natural language questions
- Fallback (no sources connected): generic prompts or "Connect a data source to get started"
- 12px, pill-shaped (20px radius), subtle border
- Click: fills the chat input and submits

**Connected Sources Strip**
- Horizontal list of connected data sources
- Green pulse dot for live connections
- 11px, muted text

**Keyboard Hints**
- Bottom-center, absolute positioned
- `⌘K` to search · `⌘J` SQL Lab
- Styled kbd elements, very subtle

### Data Flow
- On mount: fetch data sources count, table count, dataset count (parallel)
- Suggestion chips: generated server-side from schema metadata (new API endpoint)
- Page renders immediately with the static hero; counts fill in async
- No loading spinners — counts show "—" until resolved

### Empty State (no data sources)
- Hero text changes to: "Connect your first data source."
- Input placeholder: "Set up a connection to get started..."
- Chips replaced with: [Connect Fabric SQL] [Connect PostgreSQL] [Connect Azure SQL]
- Sources strip hidden

---

## 3. Workspace Page (`/workspace`)

**New page.** Absorbs content from current homepage.

### Layout

```
┌─────────┬──────────────────────────────────────────┐
│         │ Workspace                    [Search] [+] │
│         ├──────────────────────────────────────────┤
│         │ Dashboards │ Charts │ Datasets │ Queries  │
│ SIDEBAR ├──────────────────────────────────────────┤
│         │                                          │
│         │  Name          Owner    Updated    ★     │
│         │  ─────────────────────────────────────── │
│         │  Revenue Q3    PP       2h ago     ★     │
│         │  Sales Dash    PP       1d ago     ☆     │
│         │  Pipeline...   PP       3d ago     ☆     │
│         │                                          │
└─────────┴──────────────────────────────────────────┘
```

### Tabs
- **Dashboards** — list of dashboards (name, owner, updated, favorite toggle)
- **Charts** — list of charts
- **Datasets** — list of datasets
- **Saved Queries** — list of saved SQL queries

### Features
- Search bar within each tab (filters current list)
- Sort by: name, updated, created
- Filter: Mine / All toggle
- Favorite toggle (star)
- New button (+) — creates new item for current tab type
- Click row → navigates to item

### Behavior
- Default tab: Dashboards
- Tab state persisted in URL query param (`/workspace?tab=charts`)
- Each tab fetches its own data independently
- Existing list components can be reused from current pages (`/dashboards`, `/charts`, etc.)

---

## 4. About Page (`/about`)

**Replaces:** Current 950-line marketing page.

### Layout

```
┌────────────────────────────────────────────────────┐
│                                                    │
│              [Kaveon O mark, large]                │
│              KAVEON                                │
│              Talk to your data.                    │
│                                                    │
│         [Get Started]  [GitHub →]                  │
│                                                    │
├────────────────────────────────────────────────────┤
│                                                    │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐        │
│  │ AI Chat  │  │ 20+      │  │ SQL Lab  │        │
│  │ NL→SQL   │  │ Charts   │  │ Monaco   │        │
│  └──────────┘  └──────────┘  └──────────┘        │
│                                                    │
│  ┌──────────┐                                     │
│  │ Multi-DB │                                     │
│  │ Connect  │                                     │
│  └──────────┘                                     │
│                                                    │
├────────────────────────────────────────────────────┤
│                                                    │
│  Open source · Self-hosted · MIT License           │
│                                                    │
│              [Get Started]                         │
│                                                    │
└────────────────────────────────────────────────────┘
```

### Sections (3 only)
1. **Hero** — Large O mark, KAVEON wordmark, tagline, dual CTAs (Get Started + GitHub)
2. **Features** — 3-4 cards max, no emoji icons (use clean SVG line icons):
   - AI Chat: Natural language to SQL, inline chart rendering
   - Charts: 20+ chart types, ECharts, interactive
   - SQL Lab: Monaco editor, multi-tab, history, caching
   - Multi-Source: Fabric, Azure SQL, PostgreSQL, MySQL
3. **Footer CTA** — "Open source · Self-hosted · MIT License" + Get Started button

### Killed Content (move to docs if needed)
- Architecture diagram → `docs/architecture.md` or `ARCHITECTURE.md` (already exists)
- User journeys (4 flow cards) → delete
- Comparison table (vs Superset/Power BI/Redash) → delete
- RBAC & Security section → `docs/security.md` or keep in existing docs
- Database status list (live/soon/planned) → delete
- Setup instructions → README.md (already exists)

---

## 5. Theme System

### Changes
- **Delete:** Theme color picker from Settings page
- **Delete:** `ThemeContext.tsx` color customization logic (keep dark/light toggle only)
- **Keep:** CSS variables for light/dark mode
- **Keep:** localStorage persistence of light/dark preference
- **Add:** Theme toggle in sidebar footer (or keep in top-right corner)

### CSS Variables (simplified)
```css
/* Light */
:root {
  --bg-primary: #fafafa;
  --bg-surface: #ffffff;
  --bg-elevated: #ffffff;
  --border: #eaecf0;
  --text-primary: #1a1a2e;
  --text-secondary: #5a6577;
  --text-muted: #94a3b8;
  --accent: #4a9ee8;
  --accent-dark: #2d7dd2;
  --accent-bg: rgba(74, 158, 232, 0.06);
}

/* Dark */
[data-theme="dark"] {
  --bg-primary: #09090b;
  --bg-surface: #0c0c0f;
  --bg-elevated: #111827;
  --border: rgba(255, 255, 255, 0.06);
  --text-primary: #e2e8f0;
  --text-secondary: #94a3b8;
  --text-muted: #475569;
  --accent: #4a9ee8;
  --accent-dark: #1f6fc0;
  --accent-bg: rgba(74, 158, 232, 0.08);
}
```

---

## 6. Logo SVG Specification

Based on finalized brand sheet. Used in sidebar, hero watermark, About page, favicon.

### O Mark (Icon)
```svg
<svg viewBox="0 0 100 100" fill="none">
  <!-- Open arc (~280deg), gap at bottom-left -->
  <path d="M 30 72 A 34 34 0 1 1 42 80"
        stroke="#4A9EE8" stroke-width="8"
        fill="none" stroke-linecap="round"/>
  <!-- Chat tail -->
  <path d="M 30 72 L 24 84 L 42 80"
        stroke="#4A9EE8" stroke-width="8"
        fill="none" stroke-linecap="round"
        stroke-linejoin="round"/>
  <!-- Center dot -->
  <circle cx="50" cy="48" r="7" fill="#4A9EE8"/>
</svg>
```

### Usage
| Context | Size | Stroke | Opacity |
|---------|------|--------|---------|
| Sidebar expanded | 28px | 8 | 1.0 |
| Sidebar collapsed | 32px (scaled 1.15x) | 8 | 1.0 + breathing glow |
| Homepage watermark | 52px | 6 | 0.25 (light) / 0.35 (dark) |
| About page hero | 80px | 8 | 1.0 |
| Favicon | 16px | simplified (just arc + dot) | 1.0 |

---

## 7. Files Affected

### Full Rewrite
- `app/page.tsx` — homepage (1058 lines → ~200 lines)
- `app/about/page.tsx` — about page (950 lines → ~200 lines)
- `components/Layout.tsx` — top nav → sidebar layout

### New Files
- `app/workspace/page.tsx` — new workspace page (absorbs old homepage content)
- `components/Sidebar.tsx` — sidebar component (nav, search, pinned, recent, user)
- `components/KaveonMark.tsx` — O mark SVG component (replaces KaveonLogo.tsx aperture)

### Modify
- `components/ClientLayout.tsx` — update to use new Sidebar layout
- `app/globals.css` — simplified CSS variables (light/dark only), remove theme color vars
- `contexts/ThemeContext.tsx` — strip color picker, keep light/dark toggle only

### Delete
- Theme color picker UI in Settings
- `--lens-primary`, `--lens-secondary` etc. CSS variables (replace with new system)

### Unchanged
- All feature pages (`/dashboards/*`, `/charts/*`, `/datasets/*`, `/lab/*`, `/ai`)
- API layer
- Auth system
- Chart builder, dashboard builder

---

## 8. New API Endpoint

### `GET /api/kaveon/suggestions`

Returns schema-aware suggestion prompts for the homepage chips.

**Response:**
```json
{
  "suggestions": [
    "What were total sales last month?",
    "Show revenue trend by quarter",
    "Top 10 customers by revenue",
    "Compare Q2 vs Q3 performance"
  ]
}
```

**Logic:**
1. Get all connected data sources
2. For each source, scan schema metadata (tables, columns)
3. Identify common patterns (date columns → trend questions, numeric columns → aggregation questions, categorical → top-N questions)
4. Return 4 natural language questions

**Fallback:** If no sources connected, return empty array. Homepage handles this with the empty state.

---

## 9. Out of Scope

- **Inline chart rendering in AI chat** — separate workstream, will be its own spec
- **Chat conversation persistence** — the homepage input is an entry point, not a chat UI itself
- **Mobile responsive** — desktop-first for now
- **Animations beyond sidebar** — page transitions, skeleton loading, etc. can be added later
- **Keyboard shortcut system** — `⌘K` and `⌘J` are hints only; implementing the command palette is a separate task
# Archived design specification

> Historical design record. Paths, line counts, and implementation-state statements may no longer match the current repository. Use `README.md`, `STATUS.md`, and `ARCHITECTURE.md` as the authoritative sources.
