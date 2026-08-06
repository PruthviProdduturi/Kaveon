# Homepage & Layout Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Superset-style top-nav + stats-grid homepage with a chat-first sidebar layout matching the "Talk to your data" brand identity.

**Architecture:** Sidebar component owns all navigation (replaces top header). Homepage becomes a centered chat input with schema-generated suggestions. Old homepage content moves to `/workspace` with horizontal tabs. Theme system simplified to light/dark only.

**Tech Stack:** Next.js 15 App Router, React 19, TypeScript, CSS variables, existing API proxy layer

**Spec:** `docs/superpowers/specs/2026-08-05-homepage-redesign-design.md`
**Mockup:** `.superpowers/brainstorm/43640-1785971542/content/homepage-rich-v2.html`

---

## File Structure

### New Files
| File | Responsibility |
|------|----------------|
| `components/Sidebar.tsx` | Full sidebar: nav, search, pinned, recent, user profile, collapse/expand |
| `components/KaveonMark.tsx` | New O-mark SVG (open arc + chat tail + center dot) — replaces aperture |
| `app/workspace/page.tsx` | Tab-based workspace (dashboards, charts, datasets, queries) |

### Full Rewrites
| File | Lines Now | Reason |
|------|-----------|--------|
| `app/page.tsx` | 1057 | Stats grid → chat-first hero |
| `app/about/page.tsx` | 951 | Massive marketing page → lean hero + 3 features + CTA |
| `components/Layout.tsx` | 231 | Top nav → sidebar wrapper |
| `app/globals.css` | 127 | Old `--lens-*` vars → new `--bg-*`/`--text-*`/`--accent` system |

### Modify
| File | Change |
|------|--------|
| `contexts/ThemeContext.tsx` | Strip color picker logic, keep light/dark only |
| `app/layout.tsx` | Simplify preload script (light/dark, not color picker) |
| `components/ClientLayout.tsx` | Use new layout wrapper |
| `app/providers.tsx` | Keep as-is (ThemeProvider stays, just simplified) |
| `constants/branding.ts` | Update tagline if different |

### Delete
| File | Reason |
|------|--------|
| `components/SettingsModal.tsx` | Color picker UI — no longer needed |
| `components/KaveonLogo.tsx` | Old aperture logo — replaced by KaveonMark |

---

## Task 1: Create KaveonMark component (new O-mark logo)

**Files:**
- Create: `apps/kaveon-web/components/KaveonMark.tsx`

This is the foundational brand element used everywhere (sidebar, hero watermark, about page, favicon). Must match the finalized brand sheet: open arc (~280deg) with chat bubble tail at bottom-left + center dot. Color: #4A9EE8.

- [ ] **Step 1: Create KaveonMark.tsx**

```tsx
"use client";

import React, { useId } from "react";

interface KaveonMarkProps {
  size?: number;
  className?: string;
  opacity?: number;
  /** Use brand blue directly instead of CSS variable */
  useDirectColor?: boolean;
}

/**
 * KaveonMark — the Kaveon O-mark.
 * Open arc (~280°) with chat-bubble tail at bottom-left + center dot.
 * Matches the finalized brand sheet (2026-08-05).
 */
export function KaveonMark({
  size = 28,
  className = "",
  opacity = 1,
  useDirectColor = false,
}: KaveonMarkProps) {
  const uid = useId();
  const id = `kaveon-mark-${uid.replace(/:/g, "")}`;
  const color = useDirectColor ? "#4A9EE8" : "var(--accent, #4A9EE8)";

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 100 100"
      fill="none"
      className={className}
      role="img"
      aria-label="Kaveon"
      style={{ opacity }}
    >
      {/* Open arc — ~280deg, gap at bottom-left */}
      <path
        d="M 30 72 A 34 34 0 1 1 42 80"
        stroke={color}
        strokeWidth="8"
        fill="none"
        strokeLinecap="round"
      />
      {/* Chat tail */}
      <path
        d="M 30 72 L 24 84 L 42 80"
        stroke={color}
        strokeWidth="8"
        fill="none"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      {/* Center dot */}
      <circle cx="50" cy="48" r="7" fill={color} />
    </svg>
  );
}

/**
 * KaveonWordmark — KAVE[O-mark]N displayed inline.
 * For sidebar expanded state. The O is replaced by the mark SVG.
 */
export function KaveonWordmark({
  height = 18,
  className = "",
}: {
  height?: number;
  className?: string;
}) {
  const markSize = height;
  return (
    <span
      className={className}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 0,
        fontFamily: "'Inter', system-ui, sans-serif",
        fontWeight: 600,
        fontSize: height,
        letterSpacing: "2.5px",
        textTransform: "uppercase" as const,
        lineHeight: 1,
        whiteSpace: "nowrap" as const,
      }}
    >
      KAVE
      <KaveonMark
        size={markSize}
        useDirectColor
        className=""
      />
      N
    </span>
  );
}

export default KaveonMark;
```

- [ ] **Step 2: Verify it renders**

Run: `cd D:/Repos/PruthviProdduturi/Kaveon && pnpm --filter kaveon-web build 2>&1 | head -20`

Expected: No TypeScript errors for the new component.

- [ ] **Step 3: Commit**

```bash
git add apps/kaveon-web/components/KaveonMark.tsx
git commit -m "feat: add KaveonMark component — new O-mark logo (open arc + chat tail)"
```

---

## Task 2: Simplify theme system (light/dark only)

**Files:**
- Modify: `apps/kaveon-web/app/globals.css`
- Modify: `apps/kaveon-web/contexts/ThemeContext.tsx`
- Modify: `apps/kaveon-web/app/layout.tsx` (preload script)
- Delete: `apps/kaveon-web/components/SettingsModal.tsx`

Kill the color picker. One brand color (#4A9EE8). Light + dark mode only.

- [ ] **Step 1: Rewrite globals.css with new variable system**

Replace entire `apps/kaveon-web/app/globals.css` with:

```css
@import "tailwindcss";

/* ─── Kaveon Design Tokens ─── */
:root {
  /* Brand — never changes */
  --accent: #4A9EE8;
  --accent-dark: #2d7dd2;
  --accent-rgb: 74, 158, 232;

  /* Surfaces */
  --bg-primary: #fafafa;
  --bg-surface: #ffffff;
  --bg-elevated: #ffffff;
  --bg-hover: #f4f5f7;

  /* Borders */
  --border: #eaecf0;
  --border-hover: rgba(74, 158, 232, 0.4);

  /* Text */
  --text-primary: #1a1a2e;
  --text-secondary: #5a6577;
  --text-muted: #94a3b8;
  --text-faint: #c0c8d0;

  /* Semantic */
  --success: #10b981;
  --warning: #f59e0b;
  --error: #ef4444;
}

[data-theme="dark"] {
  --bg-primary: #09090b;
  --bg-surface: #0c0c0f;
  --bg-elevated: #111827;
  --bg-hover: rgba(255, 255, 255, 0.04);

  --border: rgba(255, 255, 255, 0.06);
  --border-hover: rgba(74, 158, 232, 0.3);

  --text-primary: #e2e8f0;
  --text-secondary: #94a3b8;
  --text-muted: #475569;
  --text-faint: #334155;
}

/* ─── Legacy compat — gradually remove ─── */
:root {
  --lens-primary: #4A9EE8;
  --lens-secondary: #6db3ed;
  --lens-accent: #90c7f2;
  --lens-dark: #2d7dd2;
  --lens-light: #b3dbf7;
  --lens-primary-rgb: 74, 158, 232;
  --lens-secondary-rgb: 109, 179, 237;
  --lens-accent-rgb: 144, 199, 242;
  --lens-dark-rgb: 45, 125, 210;
  --theme-primary: #4A9EE8;
  --theme-light: #6db3ed;
  --theme-dark: #2d7dd2;
}

/* ─── Animations ─── */
@keyframes fadeIn {
  from { opacity: 0; transform: translateY(10px); }
  to { opacity: 1; transform: translateY(0); }
}

@keyframes slideIn {
  from { opacity: 0; transform: translateX(-20px); }
  to { opacity: 1; transform: translateX(0); }
}

@keyframes pulse-glow {
  0%, 100% { opacity: 1; transform: scale(1); }
  50% { opacity: 0.8; transform: scale(1.05); }
}

@keyframes spin {
  0% { transform: rotate(0deg); }
  100% { transform: rotate(360deg); }
}

@keyframes kaveon-breathe {
  0%, 100% { filter: drop-shadow(0 0 0px rgba(74, 158, 232, 0)); }
  50% { filter: drop-shadow(0 0 8px rgba(74, 158, 232, 0.35)); }
}

@keyframes pulse-ring {
  0% { opacity: 1; transform: scale(1); }
  100% { opacity: 0; transform: scale(1.8); }
}

.animate-fade-in { animation: fadeIn 0.6s ease-out; }
.animate-slide-in { animation: slideIn 0.5s ease-out; }
.animate-pulse-glow { animation: pulse-glow 2s ease-in-out infinite; }

/* ─── Utility classes ─── */
.btn-primary {
  background: linear-gradient(135deg, var(--accent), var(--accent-dark));
  color: white;
  padding: 0.75rem 1.5rem;
  border-radius: 0.5rem;
  font-weight: 600;
  transition: all 0.3s ease;
  border: none;
  cursor: pointer;
}
.btn-primary:hover {
  transform: translateY(-2px);
  box-shadow: 0 8px 16px rgba(var(--accent-rgb), 0.3);
}
.btn-primary:active { transform: translateY(0); }

.spinner {
  border: 3px solid rgba(var(--accent-rgb), 0.1);
  border-top: 3px solid var(--accent);
  border-radius: 50%;
  width: 40px;
  height: 40px;
  animation: spin 1s linear infinite;
}

/* Glass morphism */
.glass {
  background: rgba(255, 255, 255, 0.1);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
  border: 1px solid rgba(255, 255, 255, 0.2);
}
```

- [ ] **Step 2: Simplify ThemeContext.tsx — light/dark toggle only**

Replace entire `apps/kaveon-web/contexts/ThemeContext.tsx` with:

```tsx
"use client";

import React, { createContext, useContext, useState, useEffect, ReactNode } from "react";

type Theme = "light" | "dark";

interface ThemeContextType {
  theme: Theme;
  toggleTheme: () => void;
  setTheme: (theme: Theme) => void;
  /** Legacy compat — always returns brand blue */
  primaryColor: string;
  gradientColors: { lighter: string; light: string; base: string; dark: string };
  isLoading: boolean;
  resetTheme: () => Promise<void>;
}

const BRAND_BLUE = "#4A9EE8";
const STORAGE_KEY = "kaveon-theme";

const ThemeContext = createContext<ThemeContextType | undefined>(undefined);

export function useTheme(): ThemeContextType {
  const context = useContext(ThemeContext);
  if (!context) throw new Error("useTheme must be used within ThemeProvider");
  return context;
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(() => {
    if (typeof window !== "undefined") {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved === "dark" || saved === "light") return saved;
    }
    return "light";
  });

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem(STORAGE_KEY, theme);
  }, [theme]);

  const toggleTheme = () => setThemeState((t) => (t === "light" ? "dark" : "light"));
  const setTheme = (t: Theme) => setThemeState(t);

  const value: ThemeContextType = {
    theme,
    toggleTheme,
    setTheme,
    primaryColor: BRAND_BLUE,
    gradientColors: { lighter: "#90c7f2", light: "#6db3ed", base: BRAND_BLUE, dark: "#2d7dd2" },
    isLoading: false,
    resetTheme: async () => setThemeState("light"),
  };

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}
```

- [ ] **Step 3: Simplify the preload script in layout.tsx**

In `apps/kaveon-web/app/layout.tsx`, replace the entire `dangerouslySetInnerHTML` script (lines 22-102) with:

```tsx
<script dangerouslySetInnerHTML={{ __html: `
  (function() {
    var t = localStorage.getItem('kaveon-theme');
    if (t === 'dark') document.documentElement.setAttribute('data-theme', 'dark');
  })();
` }} />
```

This prevents flash on dark mode without any color-picker logic.

- [ ] **Step 4: Delete SettingsModal.tsx**

```bash
git rm apps/kaveon-web/components/SettingsModal.tsx
```

- [ ] **Step 5: Commit**

```bash
git add apps/kaveon-web/app/globals.css apps/kaveon-web/contexts/ThemeContext.tsx apps/kaveon-web/app/layout.tsx
git commit -m "refactor: simplify theme to light/dark only — kill color picker, one brand blue"
```

---

## Task 3: Build the Sidebar component

**Files:**
- Create: `apps/kaveon-web/components/Sidebar.tsx`

The sidebar owns all navigation. Replaces the top header bar from `Layout.tsx`.

- [ ] **Step 1: Create Sidebar.tsx**

```tsx
"use client";

import React, { useState, useEffect } from "react";
import { usePathname, useRouter } from "next/navigation";
import { useAuth } from "../auth/useAuth";
import { useTheme } from "../contexts/ThemeContext";
import { useRole } from "../hooks/useRole";
import { KaveonMark, KaveonWordmark } from "./KaveonMark";

const STORAGE_KEY = "kaveon-sidebar-collapsed";

interface NavItem {
  icon: string;
  label: string;
  href: string;
  badge?: string;
  adminOnly?: boolean;
}

const NAV_ITEMS: NavItem[] = [
  { icon: "💬", label: "Chat", href: "/", badge: "NEW" },
  { icon: "📊", label: "Workspace", href: "/workspace" },
  { icon: "⌨️", label: "SQL Lab", href: "/lab" },
  { icon: "🔌", label: "Data Sources", href: "/data-sources" },
  { icon: "⚙️", label: "Settings", href: "/settings/system", adminOnly: true },
];

// Color-coded dots for pinned/recent items
const DOT_COLORS = ["#4a9ee8", "#10b981", "#f59e0b", "#8b5cf6"];

export function Sidebar({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  const router = useRouter();
  const { account } = useAuth();
  const { theme, toggleTheme } = useTheme();
  const { isAdmin } = useRole();

  const [collapsed, setCollapsed] = useState(() => {
    if (typeof window !== "undefined") {
      return localStorage.getItem(STORAGE_KEY) === "true";
    }
    return false;
  });

  useEffect(() => {
    localStorage.setItem(STORAGE_KEY, String(collapsed));
  }, [collapsed]);

  const isActive = (href: string) => {
    if (href === "/") return pathname === "/";
    return pathname?.startsWith(href) ?? false;
  };

  const initials = account?.name
    ? account.name.split(" ").map((n: string) => n[0]).join("").slice(0, 2).toUpperCase()
    : "?";

  const filteredNav = NAV_ITEMS.filter((item) => !item.adminOnly || isAdmin);

  return (
    <div style={{ display: "flex", height: "100vh", background: "var(--bg-primary)" }}>
      {/* ─── SIDEBAR ─── */}
      <aside
        className="kaveon-sidebar"
        style={{
          width: collapsed ? 64 : 260,
          background: "var(--bg-surface)",
          borderRight: "1px solid var(--border)",
          display: "flex",
          flexDirection: "column",
          flexShrink: 0,
          transition: "width 0.25s cubic-bezier(0.4, 0, 0.2, 1)",
          overflow: "hidden",
          position: "relative",
        }}
      >
        {/* Collapse toggle */}
        <button
          onClick={() => setCollapsed(!collapsed)}
          title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          style={{
            position: "absolute",
            top: 16,
            right: -12,
            width: 24,
            height: 24,
            background: "var(--bg-surface)",
            border: "1px solid var(--border)",
            borderRadius: "50%",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            cursor: "pointer",
            fontSize: 12,
            color: "var(--text-muted)",
            zIndex: 10,
            boxShadow: "0 1px 3px rgba(0,0,0,0.06)",
            transition: "all 0.2s",
          }}
        >
          {collapsed ? "›" : "‹"}
        </button>

        {/* Brand */}
        <div
          style={{
            padding: collapsed ? "16px 8px" : "16px",
            display: "flex",
            alignItems: "center",
            gap: 12,
            borderBottom: "1px solid var(--border)",
            minHeight: 56,
            justifyContent: collapsed ? "center" : "flex-start",
          }}
        >
          <div style={collapsed ? { animation: "kaveon-breathe 3s ease-in-out infinite" } : {}}>
            <KaveonMark size={collapsed ? 32 : 28} useDirectColor />
          </div>
          {!collapsed && (
            <KaveonWordmark height={15} className="" />
          )}
        </div>

        {/* Search */}
        <div
          onClick={() => {/* TODO: Cmd+K handler */}}
          style={{
            margin: collapsed ? "12px 8px 4px" : "12px 12px 4px",
            padding: collapsed ? "9px" : "9px 12px",
            background: "var(--bg-hover)",
            border: "1px solid var(--border)",
            borderRadius: 8,
            color: "var(--text-muted)",
            fontSize: 12,
            display: "flex",
            alignItems: "center",
            gap: 8,
            cursor: "pointer",
            transition: "border-color 0.2s",
            justifyContent: collapsed ? "center" : "flex-start",
            whiteSpace: "nowrap",
            overflow: "hidden",
          }}
        >
          <span style={{ fontSize: 13, flexShrink: 0 }}>⌕</span>
          {!collapsed && <span>Search everything...</span>}
        </div>

        {/* Navigation */}
        <nav style={{ padding: "6px 8px" }}>
          {filteredNav.map((item) => {
            const active = isActive(item.href);
            return (
              <div
                key={item.href}
                onClick={() => router.push(item.href)}
                title={collapsed ? item.label : undefined}
                style={{
                  padding: collapsed ? "9px" : "9px 12px",
                  borderRadius: 8,
                  fontSize: 13,
                  color: active ? "var(--accent)" : "var(--text-secondary)",
                  display: "flex",
                  alignItems: "center",
                  gap: 10,
                  cursor: "pointer",
                  transition: "all 0.15s",
                  marginBottom: 1,
                  position: "relative",
                  fontWeight: active ? 500 : 400,
                  background: active ? "rgba(var(--accent-rgb), 0.06)" : "transparent",
                  justifyContent: collapsed ? "center" : "flex-start",
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                }}
              >
                {active && (
                  <span
                    style={{
                      position: "absolute",
                      left: 0,
                      top: "50%",
                      transform: "translateY(-50%)",
                      width: 3,
                      height: 18,
                      background: "var(--accent)",
                      borderRadius: "0 3px 3px 0",
                    }}
                  />
                )}
                <span style={{ width: 18, textAlign: "center", fontSize: 14, flexShrink: 0, opacity: active ? 1 : 0.65 }}>
                  {item.icon}
                </span>
                {!collapsed && <span>{item.label}</span>}
                {!collapsed && item.badge && (
                  <span
                    style={{
                      marginLeft: "auto",
                      fontSize: 9,
                      fontWeight: 600,
                      letterSpacing: "0.5px",
                      padding: "2px 7px",
                      borderRadius: 4,
                      background: "rgba(var(--accent-rgb), 0.1)",
                      color: "var(--accent)",
                    }}
                  >
                    {item.badge}
                  </span>
                )}
              </div>
            );
          })}
        </nav>

        <div style={{ height: 1, background: "var(--border)", margin: "6px 16px" }} />

        {/* Pinned + Recent placeholder — will be populated from API/state later */}
        <div style={{ flex: 1, overflow: "hidden" }} />

        {/* Theme toggle + User */}
        <div
          style={{
            padding: collapsed ? "12px 8px" : "8px 16px",
            borderTop: "1px solid var(--border)",
            display: "flex",
            flexDirection: "column",
            gap: 8,
          }}
        >
          {/* Theme toggle */}
          <button
            onClick={toggleTheme}
            title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: collapsed ? "center" : "flex-start",
              gap: 10,
              padding: collapsed ? "8px" : "8px 12px",
              borderRadius: 8,
              border: "none",
              background: "transparent",
              color: "var(--text-muted)",
              cursor: "pointer",
              fontSize: 13,
              width: "100%",
              transition: "all 0.15s",
            }}
          >
            {theme === "dark" ? (
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <circle cx="12" cy="12" r="5"/>
                <line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/>
                <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/>
                <line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/>
                <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/>
              </svg>
            ) : (
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>
              </svg>
            )}
            {!collapsed && <span>{theme === "dark" ? "Light mode" : "Dark mode"}</span>}
          </button>

          {/* User */}
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 10,
              overflow: "hidden",
              justifyContent: collapsed ? "center" : "flex-start",
            }}
          >
            <div
              style={{
                width: 32,
                height: 32,
                borderRadius: 8,
                background: "linear-gradient(135deg, #4a9ee8, #1f6fc0)",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                fontSize: 12,
                fontWeight: 600,
                color: "#fff",
                flexShrink: 0,
              }}
            >
              {initials}
            </div>
            {!collapsed && (
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 12, fontWeight: 500, color: "var(--text-primary)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                  {account?.name ?? "User"}
                </div>
                <div style={{ fontSize: 10, color: "var(--text-muted)" }}>
                  {isAdmin ? "Admin" : "Viewer"}
                </div>
              </div>
            )}
          </div>
        </div>
      </aside>

      {/* ─── MAIN CONTENT ─── */}
      <main
        style={{
          flex: 1,
          overflow: "auto",
          background: "var(--bg-primary)",
          position: "relative",
        }}
      >
        {children}
      </main>
    </div>
  );
}

export default Sidebar;
```

- [ ] **Step 2: Commit**

```bash
git add apps/kaveon-web/components/Sidebar.tsx
git commit -m "feat: add Sidebar component — full nav, collapsible icon rail, theme toggle"
```

---

## Task 4: Rewrite Layout.tsx to use Sidebar

**Files:**
- Modify: `apps/kaveon-web/components/Layout.tsx`
- Modify: `apps/kaveon-web/components/ClientLayout.tsx`

Replace the top-nav Layout with the new Sidebar wrapper.

- [ ] **Step 1: Rewrite Layout.tsx**

Replace entire `apps/kaveon-web/components/Layout.tsx` with:

```tsx
"use client";

import React from "react";
import { Sidebar } from "./Sidebar";

interface LayoutProps {
  children: React.ReactNode;
}

export function Layout({ children }: LayoutProps) {
  return <Sidebar>{children}</Sidebar>;
}

export default Layout;
```

- [ ] **Step 2: Update ClientLayout.tsx**

In `apps/kaveon-web/components/ClientLayout.tsx`, the component already wraps children in `<Layout>`. Verify it still works — the Layout now renders Sidebar instead of the old header. No changes needed to ClientLayout unless it references SettingsModal.

Remove any SettingsModal import/usage from ClientLayout.tsx if present.

- [ ] **Step 3: Remove SettingsModal import from Layout.tsx references**

Search for any remaining imports of `SettingsModal` and remove them. The old Layout.tsx had `import { SettingsModal }` and state for `settingsOpen`. These are now gone in the rewrite.

- [ ] **Step 4: Verify the app compiles**

Run: `cd D:/Repos/PruthviProdduturi/Kaveon && pnpm --filter kaveon-web build 2>&1 | tail -20`

Fix any import errors (components that imported from Layout.tsx expecting old exports).

- [ ] **Step 5: Commit**

```bash
git add apps/kaveon-web/components/Layout.tsx apps/kaveon-web/components/ClientLayout.tsx
git commit -m "refactor: replace top-nav Layout with Sidebar-based layout"
```

---

## Task 5: Rewrite the Homepage (chat-first)

**Files:**
- Rewrite: `apps/kaveon-web/app/page.tsx`

Replace 1057-line stats grid with the chat-first hero. Keep the existing API call pattern for counts (used in meta line).

- [ ] **Step 1: Rewrite page.tsx**

Replace entire `apps/kaveon-web/app/page.tsx` with:

```tsx
"use client";

import React, { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { useAuth } from "../auth/useAuth";
import { useSetup } from "../components/ClientLayout";
import { msalFetch } from "../utils/msalFetch";
import { KaveonMark } from "../components/KaveonMark";

interface SourceInfo {
  name: string;
  type: string;
}

export default function Home() {
  const router = useRouter();
  const { account } = useAuth();
  const { isSetupOk } = useSetup();

  const [query, setQuery] = useState("");
  const [sourceCount, setSourceCount] = useState<number | null>(null);
  const [tableCount, setTableCount] = useState<number | null>(null);
  const [datasetCount, setDatasetCount] = useState<number | null>(null);
  const [sources, setSources] = useState<SourceInfo[]>([]);
  const [suggestions, setSuggestions] = useState<string[]>([]);

  useEffect(() => {
    if (!isSetupOk) return;
    const email = account?.email ?? "";

    // Fetch metadata counts
    msalFetch("/api/v1/metadata/summary", {
      headers: { "x-user-email": email },
    })
      .then((r) => r.json())
      .then((data) => {
        setDatasetCount(data.datasets_count ?? 0);
      })
      .catch(() => {});

    // Fetch data sources
    msalFetch("/api/v1/data-sources/list")
      .then((r) => r.json())
      .then((data) => {
        const list = Array.isArray(data) ? data : data.sources ?? [];
        setSourceCount(list.length);
        setSources(list.slice(0, 5).map((s: any) => ({ name: s.name || s.type, type: s.type })));

        // Generate simple suggestions from source names
        if (list.length > 0) {
          setSuggestions([
            "What were total sales last month?",
            "Show revenue trend by quarter",
            "Top 10 customers by revenue",
            "Compare this month vs last month",
          ]);
        }
      })
      .catch(() => {});

    // Fetch table count
    msalFetch("/api/v1/data-sources/active")
      .then((r) => r.json())
      .then((data) => {
        const count = data.tables_count ?? data.table_count ?? Object.keys(data.tables ?? {}).length;
        setTableCount(count);
      })
      .catch(() => {});
  }, [isSetupOk, account?.email]);

  const handleSubmit = (text?: string) => {
    const q = text ?? query;
    if (!q.trim()) return;
    router.push(`/ai?q=${encodeURIComponent(q.trim())}`);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  };

  const noSources = sourceCount === 0;

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        padding: "40px 48px 32px",
        position: "relative",
        minHeight: "100vh",
      }}
    >
      {/* Subtle radial glow */}
      <div
        style={{
          position: "absolute",
          top: "18%",
          left: "50%",
          transform: "translateX(-50%)",
          width: 700,
          height: 500,
          borderRadius: "50%",
          background: "radial-gradient(ellipse, rgba(var(--accent-rgb), 0.04) 0%, transparent 70%)",
          pointerEvents: "none",
        }}
      />

      {/* Hero mark watermark */}
      <KaveonMark size={52} opacity={0.25} useDirectColor />

      {/* Hero text */}
      <h1
        style={{
          fontSize: 28,
          fontWeight: 600,
          color: "var(--text-primary)",
          letterSpacing: "-0.5px",
          marginTop: 28,
          marginBottom: 6,
        }}
      >
        {noSources ? "Connect your first data source." : "Your data has answers."}
      </h1>

      {/* Meta */}
      <div
        style={{
          fontSize: 13,
          color: "var(--text-muted)",
          marginBottom: 36,
          display: "flex",
          alignItems: "center",
          gap: 16,
        }}
      >
        {sourceCount !== null && (
          <>
            <span>{sourceCount} source{sourceCount !== 1 ? "s" : ""} connected</span>
            {tableCount !== null && (
              <>
                <span style={{ width: 3, height: 3, borderRadius: "50%", background: "var(--text-faint)" }} />
                <span>{tableCount} tables</span>
              </>
            )}
            {datasetCount !== null && (
              <>
                <span style={{ width: 3, height: 3, borderRadius: "50%", background: "var(--text-faint)" }} />
                <span>{datasetCount} datasets</span>
              </>
            )}
          </>
        )}
      </div>

      {/* Chat input */}
      <div style={{ width: "100%", maxWidth: 640, position: "relative", marginBottom: 20 }}>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={noSources ? "Set up a connection to get started..." : "Ask anything about your data..."}
          autoFocus
          style={{
            width: "100%",
            padding: "16px 52px 16px 20px",
            background: "var(--bg-surface)",
            border: "1px solid var(--border)",
            borderRadius: 14,
            fontSize: 14,
            color: "var(--text-primary)",
            outline: "none",
            fontFamily: "inherit",
            transition: "all 0.2s",
            boxShadow: "0 1px 3px rgba(0,0,0,0.04)",
          }}
        />
        <button
          onClick={() => handleSubmit()}
          style={{
            position: "absolute",
            right: 8,
            top: "50%",
            transform: "translateY(-50%)",
            width: 36,
            height: 36,
            borderRadius: 10,
            border: "none",
            background: "linear-gradient(135deg, #4a9ee8, #2d7dd2)",
            color: "#fff",
            cursor: "pointer",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: 14,
            transition: "all 0.2s",
          }}
        >
          ↑
        </button>
      </div>

      {/* Suggestion chips */}
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: 8,
          justifyContent: "center",
          maxWidth: 640,
          marginBottom: 40,
        }}
      >
        {noSources ? (
          <>
            {["Connect Fabric SQL", "Connect PostgreSQL", "Connect Azure SQL"].map((label) => (
              <button
                key={label}
                onClick={() => router.push("/data-sources")}
                style={{
                  padding: "8px 16px",
                  borderRadius: 20,
                  fontSize: 12,
                  color: "var(--accent)",
                  background: "rgba(var(--accent-rgb), 0.06)",
                  border: "1px solid rgba(var(--accent-rgb), 0.2)",
                  cursor: "pointer",
                  transition: "all 0.2s",
                }}
              >
                {label}
              </button>
            ))}
          </>
        ) : (
          suggestions.map((s) => (
            <button
              key={s}
              onClick={() => handleSubmit(s)}
              style={{
                padding: "8px 16px",
                borderRadius: 20,
                fontSize: 12,
                color: "var(--text-secondary)",
                background: "var(--bg-surface)",
                border: "1px solid var(--border)",
                cursor: "pointer",
                transition: "all 0.2s",
                display: "flex",
                alignItems: "center",
                gap: 6,
                boxShadow: "0 1px 2px rgba(0,0,0,0.02)",
              }}
            >
              {s}
            </button>
          ))
        )}
      </div>

      {/* Connected sources with live dots */}
      {sources.length > 0 && (
        <div style={{ display: "flex", gap: 24, alignItems: "center" }}>
          {sources.map((src) => (
            <div key={src.name} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 11, color: "var(--text-muted)" }}>
              <span
                style={{
                  width: 7,
                  height: 7,
                  borderRadius: "50%",
                  background: "var(--success)",
                  position: "relative",
                  display: "inline-block",
                }}
              />
              {src.name}
            </div>
          ))}
        </div>
      )}

      {/* Keyboard hints */}
      <div
        style={{
          position: "absolute",
          bottom: 20,
          left: "50%",
          transform: "translateX(-50%)",
          fontSize: 11,
          color: "var(--text-faint)",
          display: "flex",
          alignItems: "center",
          gap: 6,
        }}
      >
        <kbd style={{ padding: "2px 6px", borderRadius: 4, fontSize: 10, background: "var(--bg-hover)", border: "1px solid var(--border)", color: "var(--text-muted)" }}>⌘</kbd>
        <kbd style={{ padding: "2px 6px", borderRadius: 4, fontSize: 10, background: "var(--bg-hover)", border: "1px solid var(--border)", color: "var(--text-muted)" }}>K</kbd>
        <span>search</span>
        <span style={{ margin: "0 4px" }}>·</span>
        <kbd style={{ padding: "2px 6px", borderRadius: 4, fontSize: 10, background: "var(--bg-hover)", border: "1px solid var(--border)", color: "var(--text-muted)" }}>⌘</kbd>
        <kbd style={{ padding: "2px 6px", borderRadius: 4, fontSize: 10, background: "var(--bg-hover)", border: "1px solid var(--border)", color: "var(--text-muted)" }}>J</kbd>
        <span>SQL Lab</span>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add apps/kaveon-web/app/page.tsx
git commit -m "feat: chat-first homepage — 'Your data has answers' hero with schema suggestions"
```

---

## Task 6: Create the Workspace page

**Files:**
- Create: `apps/kaveon-web/app/workspace/page.tsx`

Tab-based page that absorbs the old homepage content (dashboards, charts, datasets, saved queries).

- [ ] **Step 1: Create workspace/page.tsx**

```tsx
"use client";

import React, { useEffect, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useAuth } from "../../auth/useAuth";
import { useSetup } from "../../components/ClientLayout";
import { msalFetch } from "../../utils/msalFetch";

type Tab = "dashboards" | "charts" | "datasets" | "queries";

const TABS: { key: Tab; label: string }[] = [
  { key: "dashboards", label: "Dashboards" },
  { key: "charts", label: "Charts" },
  { key: "datasets", label: "Datasets" },
  { key: "queries", label: "Saved Queries" },
];

interface Item {
  id: number | string;
  name: string;
  title?: string;
  created_by?: string;
  updated_at?: string;
  created_at?: string;
}

export default function WorkspacePage() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const { account } = useAuth();
  const { isSetupOk } = useSetup();

  const activeTab = (searchParams.get("tab") as Tab) || "dashboards";
  const [scope, setScope] = useState<"mine" | "all">("mine");
  const [search, setSearch] = useState("");
  const [items, setItems] = useState<Item[]>([]);
  const [loading, setLoading] = useState(true);

  const setTab = (tab: Tab) => {
    router.push(`/workspace?tab=${tab}`);
  };

  useEffect(() => {
    if (!isSetupOk) return;
    setLoading(true);
    const email = account?.email ?? "";

    const endpoints: Record<Tab, string> = {
      dashboards: "/api/v1/dashboards",
      charts: "/api/v1/charts",
      datasets: "/api/v1/datasets",
      queries: "/api/v1/saved-queries",
    };

    msalFetch(endpoints[activeTab], {
      headers: { "x-user-email": email },
    })
      .then((r) => r.json())
      .then((data) => {
        const list = Array.isArray(data) ? data : data.items ?? data.dashboards ?? data.charts ?? data.datasets ?? data.queries ?? [];
        setItems(list);
      })
      .catch(() => setItems([]))
      .finally(() => setLoading(false));
  }, [activeTab, isSetupOk, account?.email]);

  const email = account?.email ?? "";
  const filtered = items
    .filter((item) => {
      if (scope === "mine" && item.created_by && item.created_by !== email) return false;
      if (search) {
        const name = (item.name || item.title || "").toLowerCase();
        if (!name.includes(search.toLowerCase())) return false;
      }
      return true;
    })
    .sort((a, b) => {
      const da = a.updated_at || a.created_at || "";
      const db = b.updated_at || b.created_at || "";
      return db.localeCompare(da);
    });

  const getHref = (tab: Tab, id: number | string) => {
    const routes: Record<Tab, string> = {
      dashboards: `/dashboards/${id}/view`,
      charts: `/charts/${id}`,
      datasets: `/datasets/${id}`,
      queries: `/lab?savedQueryId=${id}`,
    };
    return routes[tab];
  };

  const formatDate = (d?: string) => {
    if (!d) return "—";
    const date = new Date(d);
    const now = new Date();
    const diff = now.getTime() - date.getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 60) return `${mins}m ago`;
    const hrs = Math.floor(mins / 60);
    if (hrs < 24) return `${hrs}h ago`;
    const days = Math.floor(hrs / 24);
    if (days < 30) return `${days}d ago`;
    return date.toLocaleDateString();
  };

  return (
    <div style={{ padding: "32px 40px", maxWidth: 1200 }}>
      {/* Header */}
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 24 }}>
        <h1 style={{ fontSize: 22, fontWeight: 600, color: "var(--text-primary)" }}>Workspace</h1>
        <div style={{ display: "flex", gap: 8 }}>
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search..."
            style={{
              padding: "8px 14px",
              borderRadius: 8,
              border: "1px solid var(--border)",
              background: "var(--bg-surface)",
              color: "var(--text-primary)",
              fontSize: 13,
              outline: "none",
              width: 200,
            }}
          />
          <button
            onClick={() => {
              const routes: Record<Tab, string> = {
                dashboards: "/dashboards/new",
                charts: "/charts/new",
                datasets: "/datasets/new",
                queries: "/lab",
              };
              router.push(routes[activeTab]);
            }}
            style={{
              padding: "8px 16px",
              borderRadius: 8,
              border: "none",
              background: "var(--accent)",
              color: "#fff",
              fontSize: 13,
              fontWeight: 500,
              cursor: "pointer",
            }}
          >
            + New
          </button>
        </div>
      </div>

      {/* Tabs */}
      <div style={{ display: "flex", gap: 0, borderBottom: "1px solid var(--border)", marginBottom: 24 }}>
        {TABS.map((tab) => (
          <button
            key={tab.key}
            onClick={() => setTab(tab.key)}
            style={{
              padding: "10px 20px",
              fontSize: 13,
              fontWeight: activeTab === tab.key ? 500 : 400,
              color: activeTab === tab.key ? "var(--accent)" : "var(--text-secondary)",
              background: "none",
              border: "none",
              borderBottom: activeTab === tab.key ? "2px solid var(--accent)" : "2px solid transparent",
              cursor: "pointer",
              transition: "all 0.15s",
              marginBottom: -1,
            }}
          >
            {tab.label}
          </button>
        ))}
        <div style={{ flex: 1 }} />
        <div style={{ display: "flex", gap: 4, alignItems: "center", padding: "0 4px" }}>
          {(["mine", "all"] as const).map((s) => (
            <button
              key={s}
              onClick={() => setScope(s)}
              style={{
                padding: "4px 12px",
                borderRadius: 6,
                border: "none",
                fontSize: 12,
                fontWeight: scope === s ? 500 : 400,
                color: scope === s ? "var(--accent)" : "var(--text-muted)",
                background: scope === s ? "rgba(var(--accent-rgb), 0.08)" : "transparent",
                cursor: "pointer",
                textTransform: "capitalize",
              }}
            >
              {s}
            </button>
          ))}
        </div>
      </div>

      {/* Table */}
      {loading ? (
        <div style={{ textAlign: "center", padding: 60, color: "var(--text-muted)" }}>Loading...</div>
      ) : filtered.length === 0 ? (
        <div style={{ textAlign: "center", padding: 60, color: "var(--text-muted)" }}>
          No {activeTab} found. Create one to get started.
        </div>
      ) : (
        <table style={{ width: "100%", borderCollapse: "collapse" }}>
          <thead>
            <tr style={{ borderBottom: "1px solid var(--border)" }}>
              <th style={{ textAlign: "left", padding: "8px 12px", fontSize: 11, fontWeight: 600, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.5px" }}>Name</th>
              <th style={{ textAlign: "left", padding: "8px 12px", fontSize: 11, fontWeight: 600, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.5px" }}>Owner</th>
              <th style={{ textAlign: "left", padding: "8px 12px", fontSize: 11, fontWeight: 600, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.5px" }}>Updated</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((item) => (
              <tr
                key={item.id}
                onClick={() => router.push(getHref(activeTab, item.id))}
                style={{ borderBottom: "1px solid var(--border)", cursor: "pointer", transition: "background 0.1s" }}
                onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-hover)")}
                onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
              >
                <td style={{ padding: "12px", fontSize: 13, color: "var(--text-primary)", fontWeight: 500 }}>
                  {item.name || item.title || `Untitled ${activeTab.slice(0, -1)}`}
                </td>
                <td style={{ padding: "12px", fontSize: 12, color: "var(--text-secondary)" }}>
                  {item.created_by?.split("@")[0] ?? "—"}
                </td>
                <td style={{ padding: "12px", fontSize: 12, color: "var(--text-muted)" }}>
                  {formatDate(item.updated_at || item.created_at)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add apps/kaveon-web/app/workspace/page.tsx
git commit -m "feat: add Workspace page — tab-based dashboards/charts/datasets/queries"
```

---

## Task 7: Rewrite the About page (stripped down)

**Files:**
- Rewrite: `apps/kaveon-web/app/about/page.tsx`

Strip from 951 lines to ~150. Hero + 3-4 features + CTA. No comparison tables, no RBAC, no user journeys.

- [ ] **Step 1: Rewrite about/page.tsx**

Replace entire `apps/kaveon-web/app/about/page.tsx` with:

```tsx
"use client";

import React from "react";
import { useRouter } from "next/navigation";
import { KaveonMark } from "../../components/KaveonMark";

const FEATURES = [
  {
    title: "AI Chat",
    description: "Ask questions in plain English. Kaveon writes the SQL, runs it, and shows you the answer — with charts inline.",
    icon: (
      <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
      </svg>
    ),
  },
  {
    title: "20+ Chart Types",
    description: "Lines, bars, pies, heatmaps, treemaps, scatter plots, gauges, and a 3D globe. All interactive, all themeable.",
    icon: (
      <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        <line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/>
      </svg>
    ),
  },
  {
    title: "SQL Lab",
    description: "Full Monaco editor with autocomplete, multi-tab, query history, and result caching. Write SQL like a pro.",
    icon: (
      <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        <polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>
      </svg>
    ),
  },
  {
    title: "Multi-Source",
    description: "Connect Microsoft Fabric, Azure SQL, PostgreSQL, MySQL — query across all of them from one place.",
    icon: (
      <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        <ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/>
      </svg>
    ),
  },
];

export default function AboutPage() {
  const router = useRouter();

  return (
    <div style={{ minHeight: "100vh", background: "var(--bg-primary)" }}>
      {/* Hero */}
      <section
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          padding: "100px 40px 80px",
          textAlign: "center",
        }}
      >
        <KaveonMark size={80} useDirectColor />
        <h1
          style={{
            fontSize: 42,
            fontWeight: 300,
            color: "var(--text-primary)",
            letterSpacing: "-1px",
            marginTop: 32,
            marginBottom: 8,
          }}
        >
          <span style={{ fontWeight: 700 }}>Kaveon</span>
        </h1>
        <p style={{ fontSize: 20, color: "var(--text-secondary)", marginBottom: 40, fontWeight: 300 }}>
          Talk to your data.
        </p>
        <div style={{ display: "flex", gap: 12 }}>
          <button
            onClick={() => router.push("/")}
            style={{
              padding: "12px 28px",
              borderRadius: 10,
              border: "none",
              background: "linear-gradient(135deg, #4a9ee8, #2d7dd2)",
              color: "#fff",
              fontSize: 14,
              fontWeight: 500,
              cursor: "pointer",
              transition: "all 0.2s",
            }}
          >
            Get Started
          </button>
          <a
            href="https://github.com/PruthviProdduturi/Kaveon"
            target="_blank"
            rel="noopener noreferrer"
            style={{
              padding: "12px 28px",
              borderRadius: 10,
              border: "1px solid var(--border)",
              background: "var(--bg-surface)",
              color: "var(--text-secondary)",
              fontSize: 14,
              fontWeight: 500,
              cursor: "pointer",
              textDecoration: "none",
              transition: "all 0.2s",
            }}
          >
            GitHub →
          </a>
        </div>
      </section>

      {/* Features */}
      <section style={{ padding: "0 40px 80px", maxWidth: 1000, margin: "0 auto" }}>
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(2, 1fr)",
            gap: 20,
          }}
        >
          {FEATURES.map((f) => (
            <div
              key={f.title}
              style={{
                padding: 28,
                borderRadius: 12,
                border: "1px solid var(--border)",
                background: "var(--bg-surface)",
              }}
            >
              <div style={{ color: "var(--accent)", marginBottom: 16 }}>{f.icon}</div>
              <h3 style={{ fontSize: 16, fontWeight: 600, color: "var(--text-primary)", marginBottom: 8 }}>
                {f.title}
              </h3>
              <p style={{ fontSize: 13, color: "var(--text-secondary)", lineHeight: 1.6 }}>
                {f.description}
              </p>
            </div>
          ))}
        </div>
      </section>

      {/* Footer CTA */}
      <section
        style={{
          padding: "60px 40px",
          textAlign: "center",
          borderTop: "1px solid var(--border)",
        }}
      >
        <p style={{ fontSize: 14, color: "var(--text-muted)", marginBottom: 20 }}>
          Open source · Self-hosted · MIT License
        </p>
        <button
          onClick={() => router.push("/")}
          style={{
            padding: "12px 28px",
            borderRadius: 10,
            border: "none",
            background: "linear-gradient(135deg, #4a9ee8, #2d7dd2)",
            color: "#fff",
            fontSize: 14,
            fontWeight: 500,
            cursor: "pointer",
          }}
        >
          Get Started
        </button>
      </section>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add apps/kaveon-web/app/about/page.tsx
git commit -m "refactor: strip About page — hero + 4 features + CTA (was 951 lines)"
```

---

## Task 8: Clean up old logo and fix remaining imports

**Files:**
- Delete: `apps/kaveon-web/components/KaveonLogo.tsx`
- Modify: any file importing `KaveonLogo` or `KaveonMark` from old path

- [ ] **Step 1: Find all imports of old KaveonLogo**

Run: `grep -rn "KaveonLogo\|from.*KaveonLogo" apps/kaveon-web/ --include="*.tsx" --include="*.ts"`

- [ ] **Step 2: Update each import to use new KaveonMark**

For each file found:
- Replace `import { KaveonLogo } from "../components/KaveonLogo"` → `import { KaveonMark } from "../components/KaveonMark"`
- Replace `<KaveonLogo ... />` usage with `<KaveonMark size={28} useDirectColor />`
- If the component used `KaveonMark` from the old file, update the import path

Key files likely affected:
- `components/KaveonLoading.tsx` — loading spinner that uses the logo
- Any other component referencing the old aperture

- [ ] **Step 3: Delete old KaveonLogo.tsx**

```bash
git rm apps/kaveon-web/components/KaveonLogo.tsx
```

- [ ] **Step 4: Full build check**

Run: `cd D:/Repos/PruthviProdduturi/Kaveon && pnpm --filter kaveon-web build 2>&1 | tail -30`

Fix any remaining TypeScript/import errors.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: remove old KaveonLogo aperture, update all imports to KaveonMark"
```

---

## Task 9: Update layout.tsx preload and clean up remaining theme references

**Files:**
- Modify: `apps/kaveon-web/app/layout.tsx`
- Modify: any files still referencing `useTheme().setTheme(color)` or color picker APIs

- [ ] **Step 1: Simplify layout.tsx preload script**

In `apps/kaveon-web/app/layout.tsx`, replace the entire `dangerouslySetInnerHTML` script block with:

```tsx
<script dangerouslySetInnerHTML={{ __html: `(function(){var t=localStorage.getItem('kaveon-theme');if(t==='dark')document.documentElement.setAttribute('data-theme','dark')})();` }} />
```

Remove the old hex-to-HSL color conversion code, gradient generation, and `lens-theme-color` localStorage reads.

- [ ] **Step 2: Search for remaining `setTheme` color calls**

Run: `grep -rn "setTheme\|resetTheme\|primaryColor\|lens-theme-color" apps/kaveon-web/ --include="*.tsx" --include="*.ts" | grep -v node_modules | grep -v ThemeContext`

Update each usage:
- `setTheme(someColor)` → remove or replace with `setTheme("dark")`
- `primaryColor` → replace with `"#4A9EE8"` or `"var(--accent)"`
- `lens-theme-color` localStorage key → `kaveon-theme`

- [ ] **Step 3: Remove react-colorful dependency if no longer used**

```bash
cd D:/Repos/PruthviProdduturi/Kaveon && pnpm --filter kaveon-web remove react-colorful 2>/dev/null || true
```

- [ ] **Step 4: Final build check**

Run: `cd D:/Repos/PruthviProdduturi/Kaveon && pnpm --filter kaveon-web build`

Expected: Clean build, no errors.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: clean up theme system — remove color picker refs, simplify preload"
```

---

## Task 10: Smoke test and final commit

- [ ] **Step 1: Start dev server and verify key pages**

```bash
cd D:/Repos/PruthviProdduturi/Kaveon && pnpm --filter kaveon-web dev
```

Verify:
- `/` — chat-first hero with "Your data has answers", sidebar visible, collapsible
- `/workspace` — tabs work, lists load
- `/about` — clean hero + 4 features
- Dark mode toggle works
- Sidebar collapse/expand works with O-mark animation
- Navigation between pages works

- [ ] **Step 2: Fix any runtime issues found**

Address any console errors, broken styles, or navigation issues.

- [ ] **Step 3: Final commit**

```bash
git add -A
git commit -m "feat: complete homepage & layout redesign — chat-first, sidebar nav, light/dark"
```
