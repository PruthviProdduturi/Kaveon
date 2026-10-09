"use client";

import React, { useState, useEffect, useId, useMemo, useRef, useCallback, ReactNode } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { KaveonMark, KaveonWordmark } from "./KaveonMark";
import { useAuth } from "../auth/useAuth";
import { useTheme } from "../contexts/ThemeContext";
import { useGuardedNavigate } from "../contexts/NavigationGuardContext";
import { useRole } from "../hooks/useRole";
import { useRecents, RecentItem } from "../hooks/useRecents";
import { msalFetch } from "../utils/msalFetch";

const SIDEBAR_COLLAPSED_KEY = "kaveon-sidebar-collapsed";
const EXPANDED_WIDTH = 250;
const COLLAPSED_WIDTH = 56;
const TRANSITION = "250ms cubic-bezier(0.4, 0, 0.2, 1)";

interface NavItem {
  label: string;
  href: string;
  icon: ReactNode;
  exact?: boolean;
  badge?: ReactNode;
  adminOnly?: boolean;
  /**
   * Other route prefixes this section owns.
   *
   * A section is not always the only path under it. The Library lists
   * dashboards, charts and datasets, but each opens at its own top-level
   * route, so matching on `/workspace` alone left nothing in the sidebar
   * selected the moment you opened one of the things the Library is for.
   */
  owns?: string[];
}

function ChatIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
    </svg>
  );
}

function WorkspaceIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="3" width="7" height="7" /><rect x="14" y="3" width="7" height="7" />
      <rect x="14" y="14" width="7" height="7" /><rect x="3" y="14" width="7" height="7" />
    </svg>
  );
}

function CatalogIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <ellipse cx="12" cy="6" rx="8" ry="3" /><path d="M4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6" /><path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3" />
    </svg>
  );
}

function SqlLabIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="16 18 22 12 16 6" /><polyline points="8 6 2 12 8 18" />
    </svg>
  );
}

function DataSourcesIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <ellipse cx="12" cy="5" rx="9" ry="3" />
      <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
      <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
    </svg>
  );
}

function SettingsIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function PanelLeftCloseIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <line x1="9" y1="3" x2="9" y2="21" />
      <polyline points="15 9 13 12 15 15" />
    </svg>
  );
}

function PanelLeftOpenIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <line x1="9" y1="3" x2="9" y2="21" />
      <polyline points="14 9 16 12 14 15" />
    </svg>
  );
}

function SunIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="5" />
      <line x1="12" y1="1" x2="12" y2="3" />
      <line x1="12" y1="21" x2="12" y2="23" />
      <line x1="4.22" y1="4.22" x2="5.64" y2="5.64" />
      <line x1="18.36" y1="18.36" x2="19.78" y2="19.78" />
      <line x1="1" y1="12" x2="3" y2="12" />
      <line x1="21" y1="12" x2="23" y2="12" />
      <line x1="4.22" y1="19.78" x2="5.64" y2="18.36" />
      <line x1="18.36" y1="5.64" x2="19.78" y2="4.22" />
    </svg>
  );
}

function MoonIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
    </svg>
  );
}

function NewBadge() {
  return (
    <span style={{
      fontSize: 9,
      fontWeight: 700,
      padding: "1px 5px",
      borderRadius: 8,
      background: "linear-gradient(135deg, var(--accent), #6366f1)",
      color: "white",
      letterSpacing: "0.3px",
      lineHeight: 1.4,
      flexShrink: 0,
    }}>
      NEW
    </span>
  );
}

function getInitials(name: string | undefined): string {
  if (!name) return "U";
  const parts = name.trim().split(" ");
  if (parts.length === 1) return parts[0].charAt(0).toUpperCase();
  return (parts[0].charAt(0) + parts[parts.length - 1].charAt(0)).toUpperCase();
}

interface SidebarProps {
  children: ReactNode;
}

/* ─── User Menu Popup ─── */
function UserMenu({
  account,
  collapsed,
  theme,
  toggleTheme,
  logout,
  navigate,
  isAdmin,
}: {
  account: { name?: string; email?: string } | null;
  collapsed: boolean;
  theme: string;
  toggleTheme: () => void;
  logout: () => Promise<void>;
  /** Navigates, asking first when an editor holds unsaved work. */
  navigate: (href: string) => void;
  isAdmin: boolean;
}) {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handleClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [open]);

  const menuItem = (label: string, onClick: () => void, icon?: ReactNode, danger?: boolean) => (
    <button
      type="button"
      key={label}
      onClick={() => { onClick(); setOpen(false); }}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        width: "100%",
        padding: "9px 14px",
        border: "none",
        background: "transparent",
        color: danger ? "var(--error)" : "var(--text-secondary)",
        fontSize: 13,
        cursor: "pointer",
        borderRadius: 6,
        textAlign: "left",
        transition: "background 0.1s",
      }}
      onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-hover)")}
      onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
    >
      {icon && <span style={{ display: "flex", alignItems: "center", flexShrink: 0, opacity: 0.7 }}>{icon}</span>}
      {label}
    </button>
  );

  return (
    <div ref={menuRef} style={{ borderTop: "1px solid var(--border)", padding: collapsed ? "10px 0" : "10px 8px", flexShrink: 0, position: "relative" }}>
      {/* Popup menu */}
      {open && (
        <div
          style={{
            position: collapsed ? "fixed" : "absolute",
            bottom: collapsed ? 16 : "100%",
            left: collapsed ? 64 : 8,
            right: collapsed ? undefined : 8,
            width: collapsed ? 220 : undefined,
            marginBottom: collapsed ? 0 : 6,
            background: "var(--bg-surface)",
            border: "1px solid var(--border)",
            borderRadius: 10,
            padding: "6px",
            boxShadow: "var(--shadow-lg)",
            zIndex: 200,
          }}
        >
          {/* About */}
          {menuItem("About Kaveon", () => window.open("/", "_blank"),
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>
          )}

          {/* Docs */}
          {menuItem("Documentation", () => window.open("/docs", "_blank"),
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/><path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/></svg>
          )}

          <div style={{ height: 1, background: "var(--border)", margin: "4px 8px" }} />

          {/* Appearance */}
          {menuItem(
            theme === "dark" ? "Light mode" : "Dark mode",
            toggleTheme,
            theme === "dark" ? <SunIcon /> : <MoonIcon />,
          )}

          {/* Data Sources */}
          {menuItem("Data Sources", () => navigate("/data-sources"),
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>
          )}

          {/* Engine console (admin only) */}
          {isAdmin && menuItem("KaveonDB", () => navigate("/engine"),
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M13 2 4 14h7l-1 8 9-12h-7l1-8z"/></svg>
          )}

          {/* Configurations (admin only) */}
          {menuItem("Settings", () => navigate(isAdmin ? "/settings/connections" : "/settings/preferences"),
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
          )}

          <div style={{ height: 1, background: "var(--border)", margin: "4px 8px" }} />

          {/* Sign Out */}
          {menuItem("Sign Out", logout,
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>,
            true,
          )}
        </div>
      )}

      {/* User card — clickable */}
      <button
        type="button"
        onClick={() => setOpen(!open)}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: collapsed ? "6px 0" : "8px 10px",
          justifyContent: collapsed ? "center" : "flex-start",
          width: "100%",
          border: "none",
          background: open ? "var(--bg-hover)" : "transparent",
          borderRadius: 8,
          cursor: "pointer",
          transition: "background 0.1s",
        }}
        title={collapsed ? (account?.name ?? "User") : undefined}
      >
        <div
          style={{
            width: 30,
            height: 30,
            borderRadius: "50%",
            background: "color-mix(in srgb, var(--text-primary) 10%, var(--bg-surface))",
            border: "1px solid var(--border)",
            color: "var(--text-primary)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: 13,
            fontWeight: 600,
            letterSpacing: "0.5px",
            flexShrink: 0,
          }}
        >
          {getInitials(account?.name)}
        </div>
        {!collapsed && (
          <div style={{ overflow: "hidden", flex: 1, textAlign: "left" }}>
            <div style={{
              fontSize: 16,
              fontWeight: 500,
              color: "var(--text-primary)",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}>
              {(account?.name ?? "User").replace(/\w\S*/g, w => w[0].toUpperCase() + w.slice(1).toLowerCase())}
            </div>
          </div>
        )}
        {!collapsed && (
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0 }}>
            <path d="M7 15l5 5 5-5"/><path d="M7 9l5-5 5 5"/>
          </svg>
        )}
      </button>
    </div>
  );
}

const RECENT_TYPES: { key: RecentItem["type"] | "all"; label: string }[] = [
  { key: "all", label: "All" },
  { key: "dashboard", label: "Dashboards" },
  { key: "chart", label: "Charts" },
  { key: "dataset", label: "Datasets" },
  { key: "query", label: "Queries" },
  { key: "chat", label: "Chats" },
];

export function Sidebar({ children }: SidebarProps) {
  const { account, logout } = useAuth();
  const { theme, toggleTheme } = useTheme();
  const { recents, addRecent, removeRecent, clearRecents } = useRecents();
  const [recentFilter, setRecentFilter] = useState<RecentItem["type"] | "all">("all");
  const [recentMenuOpen, setRecentMenuOpen] = useState(false);
  const [recentsCollapsed, setRecentsCollapsed] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  // Every programmatic push from the sidebar goes through the navigation guard,
  // so an editor with unsaved work gets a say before the page changes.
  const navigate = useGuardedNavigate();
  const { isAdmin } = useRole();
  const pathname = usePathname();

  const [collapsed, setCollapsed] = useState<boolean>(() => {
    if (typeof window !== "undefined") {
      return localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true";
    }
    return false;
  });

  const [isMobile, setIsMobile] = useState(false);
  const [mobileOpen, setMobileOpen] = useState(false);

  useEffect(() => {
    const mq = window.matchMedia("(max-width: 768px)");
    const handler = (e: MediaQueryListEvent | MediaQueryList) => setIsMobile(e.matches);
    handler(mq);
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, []);

  // Close mobile sidebar on navigation
  useEffect(() => { setMobileOpen(false); }, [pathname]);

  // Persist collapse state
  useEffect(() => {
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(collapsed));
  }, [collapsed]);

  // Cmd+K / Ctrl+K shortcut
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") { e.preventDefault(); setSearchOpen(true); }
      if (e.key === "Escape") setSearchOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const width = collapsed ? COLLAPSED_WIDTH : EXPANDED_WIDTH;

  const navItems: NavItem[] = [
    {
      label: "New Chat",
      href: "/home",
      icon: <span style={{ display: "flex", alignItems: "center" }}><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg></span>,
      exact: true,
    },
    {
      label: "Library",
      href: "/workspace",
      icon: <WorkspaceIcon />,
      owns: ["/dashboards", "/charts", "/datasets"],
    },
    {
      label: "Catalog",
      href: "/catalog",
      icon: <CatalogIcon />,
    },
    {
      label: "SQL Lab",
      href: "/lab",
      icon: <SqlLabIcon />,
    },
  ];

  function isActive(item: NavItem): boolean {
    if (item.exact) return pathname === item.href;
    const here = pathname ?? "";
    if (here.startsWith(item.href)) return true;
    // A route the section owns counts as being in it, so opening a dashboard
    // keeps the Library selected rather than clearing the sidebar entirely.
    // Matched on a segment boundary so `/chartsomething` is not `/charts`.
    return (item.owns ?? []).some(
      (prefix) => here === prefix || here.startsWith(prefix + "/"));
  }

  const sidebarStyle: React.CSSProperties = isMobile ? {
    position: "fixed",
    top: 0,
    left: 0,
    bottom: 0,
    width: EXPANDED_WIDTH,
    minWidth: EXPANDED_WIDTH,
    maxWidth: EXPANDED_WIDTH,
    background: "var(--bg-primary)",
    borderRight: "1px solid var(--border)",
    display: "flex",
    flexDirection: "column",
    transform: mobileOpen ? "translateX(0)" : "translateX(-100%)",
    transition: `transform ${TRANSITION}`,
    overflow: "hidden",
    zIndex: 200,
  } : {
    position: "fixed",
    top: 0,
    left: 0,
    bottom: 0,
    width,
    minWidth: width,
    maxWidth: width,
    background: "var(--bg-primary)",
    borderRight: "1px solid var(--border)",
    display: "flex",
    flexDirection: "column",
    transition: `width ${TRANSITION}, min-width ${TRANSITION}, max-width ${TRANSITION}`,
    overflow: "hidden",
    zIndex: 100,
  };

  const collapseButtonStyle: React.CSSProperties = {
    position: "absolute",
    top: 18,
    right: -14,
    width: 28,
    height: 28,
    borderRadius: 8,
    background: "var(--bg-surface)",
    border: "1px solid var(--border)",
    cursor: "pointer",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    color: "var(--text-muted)",
    zIndex: 10,
    boxShadow: "var(--shadow-md)",
    flexShrink: 0,
    transition: `color ${TRANSITION}, background ${TRANSITION}`,
    padding: 0,
  };

  return (
    <div style={{ display: "flex", minHeight: "100vh", background: "var(--bg-primary)" }}>
      {/* Mobile hamburger */}
      {isMobile && !mobileOpen && (
        <button
          type="button"
          onClick={() => setMobileOpen(true)}
          style={{
            position: "fixed", top: 12, left: 12, zIndex: 150,
            width: 36, height: 36, borderRadius: 8,
            background: "var(--bg-surface)", border: "1px solid var(--border)",
            cursor: "pointer", display: "flex", alignItems: "center", justifyContent: "center",
            color: "var(--text-secondary)", boxShadow: "var(--shadow-md)", padding: 0,
          }}
        >
          <i className="fas fa-bars" style={{ fontSize: 14 }} />
        </button>
      )}

      {/* Mobile backdrop */}
      {isMobile && mobileOpen && (
        <div
          onClick={() => setMobileOpen(false)}
          style={{
            position: "fixed", inset: 0, zIndex: 190,
            background: "rgba(0,0,0,0.5)", backdropFilter: "blur(2px)",
          }}
        />
      )}

      {/* Sidebar */}
      <aside style={sidebarStyle}>

        {/* Brand bar — logo + search icon + collapse icon */}
        <div style={{
          display: "flex",
          alignItems: "center",
          justifyContent: collapsed ? "center" : "space-between",
          padding: collapsed ? "14px 0" : "14px 16px",
          minHeight: 56,
          flexShrink: 0,
          transition: `padding ${TRANSITION}`,
        }}>
          {collapsed ? (
            <button type="button" onClick={() => setCollapsed(false)} title="Expand sidebar" style={{ background: "none", border: "none", cursor: "pointer", padding: 4 }}>
              <div style={{ animation: "kaveon-breathe 3s ease-in-out infinite" }}>
                <KaveonMark size={30} />
              </div>
            </button>
          ) : (
            <>
              <Link href="/home" onClick={() => { if (pathname === "/home") window.dispatchEvent(new CustomEvent("kaveon-new-chat")); }} style={{ display: "flex", alignItems: "center", textDecoration: "none" }}>
                <KaveonWordmark height={24} />
              </Link>
              <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                {/* Search icon */}
                <button type="button" title="Search (Ctrl+K)" onClick={() => setSearchOpen(true)} style={{ background: "none", border: "none", cursor: "pointer", padding: 6, borderRadius: 6, color: "var(--text-muted)", display: "flex", alignItems: "center", transition: "color 0.15s" }}
                  onMouseEnter={e => e.currentTarget.style.color = "var(--text-primary)"}
                  onMouseLeave={e => e.currentTarget.style.color = "var(--text-muted)"}>
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
                </button>
                {/* Collapse icon */}
                <button type="button" onClick={() => setCollapsed(true)} title="Collapse sidebar" style={{ background: "none", border: "none", cursor: "pointer", padding: 6, borderRadius: 6, color: "var(--text-muted)", display: "flex", alignItems: "center", transition: "color 0.15s" }}
                  onMouseEnter={e => e.currentTarget.style.color = "var(--text-primary)"}
                  onMouseLeave={e => e.currentTarget.style.color = "var(--text-muted)"}>
                  <PanelLeftCloseIcon />
                </button>
              </div>
            </>
          )}
        </div>


        {/* Navigation */}
        <nav style={{ padding: "4px 8px", flexShrink: 0 }} aria-label="Main navigation">
          {navItems.map((item) => {
            if (item.adminOnly && !isAdmin) return null;
            const active = isActive(item);
            return (
              <Link
                key={item.href}
                href={item.href}
                onClick={() => {
                  if (item.href === "/home" && pathname === "/home") {
                    window.dispatchEvent(new CustomEvent("kaveon-new-chat"));
                  }
                }}
                title={collapsed ? item.label : undefined}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 10,
                  padding: collapsed ? "9px 0" : "9px 10px",
                  borderRadius: 7,
                  justifyContent: collapsed ? "center" : "flex-start",
                  textDecoration: "none",
                  color: active ? "var(--accent)" : "var(--text-secondary)",
                  background: active ? `rgba(var(--accent-rgb), 0.08)` : "transparent",
                  borderLeft: active ? "3px solid var(--accent)" : "3px solid transparent",
                  fontWeight: active ? 600 : 400,
                  fontSize: 14.5,
                  marginBottom: 2,
                  transition: `background ${TRANSITION}, color ${TRANSITION}`,
                  overflow: "hidden",
                  whiteSpace: "nowrap",
                  position: "relative",
                }}
              >
                <span style={{ flexShrink: 0, display: "flex", alignItems: "center" }}>
                  {item.icon}
                </span>
                {!collapsed && (
                  <>
                    <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis" }}>
                      {item.label}
                    </span>
                    {item.badge}
                  </>
                )}
              </Link>
            );
          })}
        </nav>

        {/* Divider */}
        <div style={{ height: 1, background: "var(--border)", margin: "8px 16px", flexShrink: 0 }} />

        {/* Conversations + Pinned */}
        <div style={{ flex: 1, overflow: "auto", padding: collapsed ? "0" : "0 8px" }}>
          {!collapsed && (
            <>
              {/* Recent items */}
              {recents.length > 0 && (
                <>
                  {/* Header row: label + collapse + filter/clear options menu */}
                  <div style={{ position: "relative", display: "flex", alignItems: "center", justifyContent: "space-between", padding: "8px 6px 4px 10px" }}>
                    <button
                      type="button"
                      onClick={() => setRecentsCollapsed(v => !v)}
                      style={{ display: "flex", alignItems: "center", gap: 5, background: "none", border: "none", cursor: "pointer", padding: 0, fontSize: 11, fontWeight: 600, color: "var(--text-muted)", userSelect: "none" }}
                    >
                      <i className={`fas fa-chevron-${recentsCollapsed ? "right" : "down"}`} style={{ fontSize: 8, opacity: 0.5, transition: "transform 0.15s" }} />
                      {recentFilter === "all" ? "Recents" : `Recents · ${RECENT_TYPES.find(t => t.key === recentFilter)?.label}`}
                    </button>
                    <button
                      type="button"
                      title="Filter / clear recents"
                      onClick={() => setRecentMenuOpen((v) => !v)}
                      style={{ display: "flex", alignItems: "center", justifyContent: "center", width: 22, height: 22, borderRadius: 6, border: "none", background: recentMenuOpen ? "var(--bg-hover)" : "transparent", color: "var(--text-muted)", cursor: "pointer" }}
                    >
                      <i className="fas fa-sliders-h" style={{ fontSize: 11 }} />
                    </button>
                    {recentMenuOpen && (
                      <>
                        <div style={{ position: "fixed", inset: 0, zIndex: 40 }} onClick={() => setRecentMenuOpen(false)} />
                        <div style={{ position: "absolute", top: 28, right: 4, zIndex: 41, minWidth: 170, background: "var(--bg-elevated)", border: "1px solid var(--border)", borderRadius: 10, padding: 4, boxShadow: "0 8px 24px rgba(0,0,0,0.35)" }}>
                          <div style={{ fontSize: 10, fontWeight: 600, letterSpacing: "0.5px", textTransform: "uppercase", color: "var(--text-faint)", padding: "6px 10px 4px" }}>Filter</div>
                          {RECENT_TYPES.map((t) => {
                            const count = t.key === "all" ? recents.length : recents.filter((r) => r.type === t.key).length;
                            return (
                              <button key={t.key} type="button"
                                onClick={() => { setRecentFilter(t.key); setRecentMenuOpen(false); }}
                                style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, width: "100%", padding: "7px 10px", border: "none", background: recentFilter === t.key ? "var(--bg-hover)" : "transparent", color: "var(--text-secondary)", fontSize: 13, cursor: "pointer", borderRadius: 6, textAlign: "left", fontFamily: "inherit" }}
                                onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-hover)")}
                                onMouseLeave={(e) => (e.currentTarget.style.background = recentFilter === t.key ? "var(--bg-hover)" : "transparent")}
                              >
                                <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
                                  {recentFilter === t.key && <i className="fas fa-check" style={{ fontSize: 10, color: "var(--accent)" }} />}
                                  <span style={{ marginLeft: recentFilter === t.key ? 0 : 18 }}>{t.label}</span>
                                </span>
                                <span style={{ fontSize: 11, color: "var(--text-faint)" }}>{count}</span>
                              </button>
                            );
                          })}
                          <div style={{ height: 1, background: "var(--border)", margin: "4px 0" }} />
                          <button type="button"
                            onClick={() => { clearRecents(recentFilter === "all" ? undefined : recentFilter); setRecentMenuOpen(false); }}
                            style={{ display: "flex", alignItems: "center", gap: 8, width: "100%", padding: "7px 10px", border: "none", background: "transparent", color: "#f87171", fontSize: 13, cursor: "pointer", borderRadius: 6, textAlign: "left", fontFamily: "inherit" }}
                            onMouseEnter={(e) => (e.currentTarget.style.background = "rgba(220,38,38,0.1)")}
                            onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
                          >
                            <i className="fas fa-trash" style={{ fontSize: 11 }} />
                            {recentFilter === "all" ? "Clear all" : `Clear ${RECENT_TYPES.find(t => t.key === recentFilter)?.label}`}
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                  {!recentsCollapsed && (recentFilter === "all" ? recents : recents.filter((r) => r.type === recentFilter)).map((item) => (
                    <button
                      key={item.id}
                      type="button"
                      onClick={() => navigate(item.href)}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 8,
                        width: "100%",
                        padding: "8px 12px",
                        border: "none",
                        background: "transparent",
                        color: "var(--text-secondary)",
                        fontSize: 13.5,
                        cursor: "pointer",
                        borderRadius: 8,
                        textAlign: "left",
                        transition: "background 0.1s",
                        overflow: "hidden",
                      }}
                      onMouseEnter={(e) => {
                        e.currentTarget.style.background = "var(--bg-hover)";
                        const close = e.currentTarget.querySelector("[data-close]") as HTMLElement;
                        if (close) close.style.display = "flex";
                      }}
                      onMouseLeave={(e) => {
                        e.currentTarget.style.background = "transparent";
                        const close = e.currentTarget.querySelector("[data-close]") as HTMLElement;
                        if (close) close.style.display = "none";
                      }}
                    >
                      <span style={{ flexShrink: 0, display: "flex", alignItems: "center", color: "var(--text-secondary)" }}>
                        {item.type === "chat" ? (
                          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
                        ) : item.type === "dashboard" ? (
                          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>
                        ) : item.type === "chart" ? (
                          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/></svg>
                        ) : item.type === "dataset" ? (
                          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>
                        ) : item.type === "query" ? (
                          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>
                        ) : (
                          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
                        )}
                      </span>
                      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>
                        {item.label}
                      </span>
                      <span
                        data-close
                        role="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          e.preventDefault();
                          // Show context menu
                          const rect = e.currentTarget.getBoundingClientRect();
                          const menu = document.createElement("div");
                          menu.style.cssText = `position:fixed;top:${rect.bottom + 4}px;left:${rect.left - 60}px;background:var(--bg-elevated);border:1px solid var(--border);border-radius:10px;padding:4px;box-shadow:0 8px 24px rgba(0,0,0,0.4);z-index:999;min-width:140px;`;
                          const options = [
                            ...(item.type === "chat" ? [{ label: "Rename", svg: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/></svg>', action: () => {
                              const newName = prompt("Rename:", item.label);
                              if (newName) { removeRecent(item.id); addRecent({ ...item, id: item.id, label: newName, href: item.href, type: item.type }); }
                            }}] : []),
                            { label: item.type === "chat" ? "Delete" : "Close", svg: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>', action: () => { removeRecent(item.id); if (pathname === item.href) navigate("/home"); } },
                          ];
                          options.forEach(opt => {
                            const btn = document.createElement("button");
                            btn.style.cssText = `display:flex;align-items:center;gap:8px;width:100%;padding:8px 12px;border:none;background:transparent;color:var(--text-secondary);font-size:13px;cursor:pointer;border-radius:6px;text-align:left;font-family:inherit;`;
                            btn.onmouseenter = () => btn.style.background = "var(--bg-hover)";
                            btn.onmouseleave = () => btn.style.background = "transparent";
                            btn.innerHTML = `${opt.svg}<span>${opt.label}</span>`;
                            btn.onclick = () => { opt.action(); menu.remove(); };
                            menu.appendChild(btn);
                          });
                          document.body.appendChild(menu);
                          const dismiss = (ev: MouseEvent) => { if (!menu.contains(ev.target as Node)) { menu.remove(); document.removeEventListener("mousedown", dismiss); } };
                          setTimeout(() => document.addEventListener("mousedown", dismiss), 0);
                        }}
                        style={{
                          display: "none",
                          alignItems: "center",
                          justifyContent: "center",
                          width: 20,
                          height: 20,
                          borderRadius: 4,
                          fontSize: 13,
                          color: "var(--text-muted)",
                          flexShrink: 0,
                          cursor: "pointer",
                        }}
                      >
                        ⋯
                      </span>
                    </button>
                  ))}
                  {!recentsCollapsed && recentFilter !== "all" && recents.filter((r) => r.type === recentFilter).length === 0 && (
                    <div style={{ padding: "10px 12px", fontSize: 12, color: "var(--text-faint)" }}>
                      No {RECENT_TYPES.find((t) => t.key === recentFilter)?.label.toLowerCase()} recently
                    </div>
                  )}
                </>
              )}
              {recents.length === 0 && (
                <div style={{
                  padding: "16px 10px",
                  fontSize: 12,
                  color: "var(--text-muted)",
                  opacity: 0.6,
                }}>
                  Recent items will appear here
                </div>
              )}
            </>
          )}
        </div>

        {/* Footer — User card */}
        <UserMenu
          account={account}
          collapsed={collapsed}
          theme={theme}
          toggleTheme={toggleTheme}
          logout={logout}
          navigate={navigate}
          isAdmin={isAdmin}
        />
      </aside>

      {/* Main content */}
      <div style={{
        flex: 1,
        marginLeft: isMobile ? 0 : width,
        transition: isMobile ? "none" : `margin-left ${TRANSITION}`,
        minWidth: 0,
        overflow: "auto",
      }}>
        {children}
      </div>

      {searchOpen && <SpotlightSearch recents={recents} onClose={() => setSearchOpen(false)} onNavigate={(href) => { setSearchOpen(false); navigate(href); }} />}
    </div>
  );
}

/* ─── Spotlight Search (Cmd+K) ─────────────────────────────────────────────── */

/** The kinds of match the search produces. */
type ResultKind = "recent" | "dashboard" | "chart" | "dataset" | "page";

/** A category, or the ranked union of every category. */
type TabId = ResultKind | "all";

/** Result kinds plus the two recent kinds that have no category of their own. */
type GlyphName = ResultKind | "query" | "chat";

interface SearchResult {
  id: string;
  label: string;
  href: string;
  type: ResultKind;
  glyph: GlyphName;
}

interface SearchTab {
  id: TabId;
  label: string;
  count: number;
}

/**
 * Kind order is the cross-category ranking: what the reader opened last, then
 * the content they own, then where they can go. The flat list ranked this way
 * before the categories became tabs, and the All tab, the order of the tab
 * strip and the order within every tab all still follow it.
 */
const KIND_ORDER: ResultKind[] = ["recent", "dashboard", "chart", "dataset", "page"];

const KIND_LABELS: Record<ResultKind, string> = {
  recent: "Recent",
  dashboard: "Dashboards",
  chart: "Charts",
  dataset: "Datasets",
  page: "Navigation",
};

/** A recent keeps its own glyph, so the Recent tab still shows what each row is. */
const RECENT_GLYPHS: Record<RecentItem["type"], GlyphName> = {
  dashboard: "dashboard",
  chart: "chart",
  dataset: "dataset",
  query: "query",
  chat: "chat",
};

const PAGES: SearchResult[] = [
  { id: "p-chat",     label: "New Chat",      href: "/home",                 type: "page", glyph: "chat" },
  { id: "p-library",  label: "Library",       href: "/workspace",            type: "page", glyph: "dashboard" },
  { id: "p-catalog",  label: "Catalog",       href: "/catalog",              type: "page", glyph: "dataset" },
  { id: "p-sql",      label: "SQL Lab",       href: "/lab",                  type: "page", glyph: "query" },
  { id: "p-lineage",  label: "Lineage",       href: "/workspace?tab=lineage", type: "page", glyph: "chart" },
  { id: "p-ds",       label: "Data Sources",  href: "/data-sources",         type: "page", glyph: "dataset" },
  { id: "p-engine",   label: "KaveonDB",      href: "/engine",               type: "page", glyph: "page" },
  { id: "p-settings", label: "Settings",      href: "/settings/connections", type: "page", glyph: "page" },
  { id: "p-about",    label: "About Kaveon",  href: "/",                     type: "page", glyph: "page" },
];

const GLYPH_PATHS: Record<GlyphName, ReactNode> = {
  recent: <><circle cx="12" cy="12" r="9" /><polyline points="12 7 12 12 16 14" /></>,
  dashboard: <><rect x="3" y="3" width="7" height="7" /><rect x="14" y="3" width="7" height="7" /><rect x="14" y="14" width="7" height="7" /><rect x="3" y="14" width="7" height="7" /></>,
  chart: <><line x1="5" y1="20" x2="19" y2="20" /><line x1="8" y1="20" x2="8" y2="12" /><line x1="12" y1="20" x2="12" y2="5" /><line x1="16" y1="20" x2="16" y2="15" /></>,
  dataset: <><rect x="3" y="4" width="18" height="16" rx="2" /><line x1="3" y1="10" x2="21" y2="10" /><line x1="9" y1="10" x2="9" y2="20" /></>,
  query: <><polyline points="5 8 9 12 5 16" /><line x1="12" y1="16" x2="19" y2="16" /></>,
  chat: <path d="M21 14a2 2 0 0 1-2 2H8l-4 4V5a2 2 0 0 1 2-2h13a2 2 0 0 1 2 2z" />,
  page: <><line x1="4" y1="12" x2="19" y2="12" /><polyline points="13 6 19 12 13 18" /></>,
};

function ResultGlyph({ glyph }: { glyph: GlyphName }) {
  return (
    <svg
      width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"
      style={{ flexShrink: 0, opacity: 0.6 }}
    >
      {GLYPH_PATHS[glyph]}
    </svg>
  );
}

function SearchProgress() {
  return (
    <svg
      width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)"
      strokeWidth="2" strokeLinecap="round" aria-hidden="true"
      style={{ animation: "spin 0.8s linear infinite", flexShrink: 0 }}
    >
      <path d="M21 12a9 9 0 1 1-6.22-8.56" />
    </svg>
  );
}

/**
 * Which tab to open for the term that was typed. When every match sits in one
 * category the reader is already where they wanted to be, so open there;
 * otherwise open the ranked union, where the spread across categories — and
 * the ranking between them — is what the reader needs to see first.
 */
function defaultTab(items: SearchResult[]): TabId {
  const kinds = KIND_ORDER.filter((kind) => items.some((item) => item.type === kind));
  return kinds.length === 1 ? kinds[0] : "all";
}

/**
 * A category with no matches is left out of the strip. The strip exists to say
 * where the matches are; a row of zeroes says nothing, and each one would be
 * another keyboard stop onto an empty panel.
 */
function buildTabs(items: SearchResult[]): SearchTab[] {
  const present: SearchTab[] = KIND_ORDER
    .map((kind) => ({ id: kind as TabId, label: KIND_LABELS[kind], count: items.filter((item) => item.type === kind).length }))
    .filter((entry) => entry.count > 0);
  if (present.length < 2) return present;
  return [{ id: "all", label: "All results", count: items.length }, ...present];
}

function SpotlightSearch({ recents, onClose, onNavigate }: {
  recents: RecentItem[];
  onClose: () => void;
  onNavigate: (href: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [tab, setTab] = useState<TabId>("all");
  const [selected, setSelected] = useState(0);
  const [loading, setLoading] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const stripRef = useRef<HTMLDivElement>(null);
  const restoreFocusRef = useRef(true);
  const pinnedTabRef = useRef<TabId | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const domId = useId();

  // The field takes focus on open, and whatever opened the overlay takes it
  // back when the overlay closes without going anywhere.
  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    inputRef.current?.focus();
    return () => { if (restoreFocusRef.current) opener?.focus?.(); };
  }, []);

  // A tab the reader picked themselves survives the next result set, as long as
  // that set still holds something for it. Otherwise the default tab applies.
  const applyResults = useCallback((items: SearchResult[]) => {
    const pinned = pinnedTabRef.current;
    const pinnedHolds = pinned === null
      ? false
      : pinned === "all" ? items.length > 0 : items.some((item) => item.type === pinned);
    setResults(items);
    setTab(pinned !== null && pinnedHolds ? pinned : defaultTab(items));
    setSelected(0);
  }, []);

  // Search API + recents + pages
  useEffect(() => {
    pinnedTabRef.current = null;
    const q = query.trim().toLowerCase();
    if (!q) {
      // Show recents + pages when empty
      const recentResults: SearchResult[] = recents.slice(0, 5).map(r => ({
        id: `r-${r.id}`, label: r.label, href: r.href, type: "recent", glyph: RECENT_GLYPHS[r.type] ?? "recent",
      }));
      applyResults([...recentResults, ...PAGES]);
      return;
    }

    // Filter pages
    const pageMatches = PAGES.filter(p => p.label.toLowerCase().includes(q));

    // Filter recents
    const recentMatches: SearchResult[] = recents
      .filter(r => r.label.toLowerCase().includes(q))
      .slice(0, 3)
      .map(r => ({ id: `r-${r.id}`, label: r.label, href: r.href, type: "recent", glyph: RECENT_GLYPHS[r.type] ?? "recent" }));

    // Debounced API search
    clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(async () => {
      setLoading(true);
      try {
        const [dashRes, chartRes, dsRes] = await Promise.all([
          msalFetch("/api/v1/dashboards").then(r => r.ok ? r.json() : []).catch(() => []),
          msalFetch("/api/v1/charts").then(r => r.ok ? r.json() : []).catch(() => []),
          msalFetch("/api/v1/datasets/summary").then(r => r.ok ? r.json() : []).catch(() => []),
        ]);

        const toArr = (d: any) => Array.isArray(d) ? d : d?.result || d?.items || [];

        const dashboards: SearchResult[] = toArr(dashRes)
          .filter((d: any) => (d.name || "").toLowerCase().includes(q))
          .slice(0, 5)
          .map((d: any) => ({ id: `d-${d.id}`, label: d.name, href: `/dashboards/${d.id}/view`, type: "dashboard" as const, glyph: "dashboard" as const }));

        const charts: SearchResult[] = toArr(chartRes)
          .filter((c: any) => (c.name || "").toLowerCase().includes(q))
          .slice(0, 5)
          .map((c: any) => ({ id: `c-${c.id}`, label: c.name, href: `/charts/${c.id}`, type: "chart" as const, glyph: "chart" as const }));

        const datasets: SearchResult[] = toArr(dsRes)
          .filter((d: any) => (d.name || "").toLowerCase().includes(q))
          .slice(0, 3)
          .map((d: any) => ({ id: `ds-${d.id}`, label: d.name, href: `/datasets/${d.id}`, type: "dataset" as const, glyph: "dataset" as const }));

        // Deduplicate against recents
        const seen = new Set(recentMatches.map(r => r.href));
        const apiResults = [...dashboards, ...charts, ...datasets].filter(r => !seen.has(r.href));

        applyResults([...recentMatches, ...apiResults, ...pageMatches]);
      } catch { /* ignore */ }
      setLoading(false);
    }, 200);

    // Show immediate local results
    applyResults([...recentMatches, ...pageMatches]);

    return () => clearTimeout(debounceRef.current);
  }, [query, recents, applyResults]);

  const tabs = useMemo(() => buildTabs(results), [results]);

  // The All tab keeps the headed groups the flat list had; a category tab is
  // one unheaded run, because the tab is already the heading.
  const groups = useMemo<{ kind: ResultKind | null; items: SearchResult[] }[]>(() => {
    if (tab !== "all") return [{ kind: null, items: results.filter((r) => r.type === tab) }];
    return KIND_ORDER
      .map((kind) => ({ kind: kind as ResultKind | null, items: results.filter((r) => r.type === kind) }))
      .filter((group) => group.items.length > 0);
  }, [results, tab]);

  const visible = useMemo(() => groups.flatMap((group) => group.items), [groups]);
  const groupOffsets = useMemo(() => {
    let running = 0;
    return groups.map((group) => {
      const start = running;
      running += group.items.length;
      return start;
    });
  }, [groups]);

  const activeIndex = visible.length === 0 ? -1 : Math.min(selected, visible.length - 1);
  const activeResult = activeIndex < 0 ? null : visible[activeIndex];
  const activeTabLabel = tabs.find((entry) => entry.id === tab)?.label ?? "Results";

  const selectTab = useCallback((id: TabId) => {
    pinnedTabRef.current = id;
    setTab(id);
    setSelected(0);
  }, []);

  const navigate = useCallback((href: string) => {
    restoreFocusRef.current = false;
    onNavigate(href);
  }, [onNavigate]);

  // Keep the chosen row and the chosen tab in sight when the keyboard, rather
  // than the pointer, is what moved them.
  useEffect(() => {
    panelRef.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, tab]);

  useEffect(() => {
    stripRef.current?.querySelector('[data-current="true"]')?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [tab]);

  // Left and Right belong to the query field while the caret still has text to
  // cross, so correcting a term mid-word keeps working. At either edge of the
  // text — and on an empty field — they step between categories instead.
  const caretCanMove = (step: -1 | 1) => {
    const input = inputRef.current;
    if (!input || document.activeElement !== input) return false;
    const { selectionStart, selectionEnd, value } = input;
    if (selectionStart === null || selectionEnd === null) return false;
    if (selectionStart !== selectionEnd) return true;
    return step < 0 ? selectionStart > 0 : selectionStart < value.length;
  };

  const stepTab = (step: -1 | 1) => {
    if (tabs.length < 2) return false;
    const at = tabs.findIndex((entry) => entry.id === tab);
    const next = tabs[((at < 0 ? 0 : at) + step + tabs.length) % tabs.length];
    selectTab(next.id);
    return true;
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        if (visible.length > 0) setSelected(Math.min(activeIndex + 1, visible.length - 1));
        break;
      case "ArrowUp":
        e.preventDefault();
        if (visible.length > 0) setSelected(Math.max(activeIndex - 1, 0));
        break;
      case "Home":
        if (visible.length > 0) { e.preventDefault(); setSelected(0); }
        break;
      case "End":
        if (visible.length > 0) { e.preventDefault(); setSelected(visible.length - 1); }
        break;
      case "ArrowLeft":
      case "ArrowRight": {
        const step: -1 | 1 = e.key === "ArrowLeft" ? -1 : 1;
        if (caretCanMove(step)) break;
        if (stepTab(step)) e.preventDefault();
        break;
      }
      case "Enter":
        if (activeResult) { e.preventDefault(); navigate(activeResult.href); }
        break;
      case "Escape":
        e.preventDefault();
        onClose();
        break;
      case "Tab":
        restoreFocusRef.current = false;
        onClose();
        break;
      default:
        break;
    }
  };

  const hint = (keys: string, label: string) => (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 5 }}>
      <kbd style={{ padding: "1px 4px", borderRadius: 3, border: "1px solid var(--border)", fontSize: 9, fontFamily: "inherit" }}>{keys}</kbd>
      {label}
    </span>
  );

  return (
    <>
      <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.5)", backdropFilter: "blur(4px)", zIndex: 10000 }} onClick={onClose} />
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Search Kaveon"
        onKeyDown={handleKeyDown}
        style={{
          position: "fixed", top: "18%", left: "50%", transform: "translateX(-50%)",
          width: "90%", maxWidth: 560,
          background: "var(--bg-surface)", border: "1px solid var(--border)",
          borderRadius: 16, boxShadow: "0 24px 80px rgba(0,0,0,0.4)",
          zIndex: 10001, overflow: "hidden",
        }}
      >
        {/* Search input */}
        <div style={{ display: "flex", alignItems: "center", gap: 12, padding: "14px 18px", borderBottom: "1px solid var(--border)" }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
          <input
            ref={inputRef}
            type="text"
            role="combobox"
            aria-expanded={visible.length > 0}
            aria-controls={`${domId}-listbox`}
            aria-activedescendant={activeResult ? `${domId}-option-${activeIndex}` : undefined}
            aria-autocomplete="list"
            placeholder="Search dashboards, charts, datasets..."
            value={query}
            onChange={e => setQuery(e.target.value)}
            style={{
              flex: 1, border: "none", outline: "none", background: "transparent",
              fontSize: 15, color: "var(--text-primary)", fontFamily: "inherit",
            }}
          />
          {loading && <SearchProgress />}
          <kbd style={{ fontSize: 10, padding: "2px 6px", borderRadius: 4, border: "1px solid var(--border)", color: "var(--text-muted)", background: "var(--bg-primary)", fontFamily: "inherit" }}>ESC</kbd>
        </div>

        {/* Categories */}
        {tabs.length > 0 && (
          <div
            ref={stripRef}
            role="tablist"
            aria-label="Result categories"
            aria-orientation="horizontal"
            style={{
              display: "flex", gap: 2, padding: "0 12px",
              borderBottom: "1px solid var(--border)",
              overflowX: "auto", scrollbarWidth: "none",
            }}
          >
            {tabs.map((entry) => {
              const current = entry.id === tab;
              return (
                <button
                  key={entry.id}
                  type="button"
                  role="tab"
                  id={`${domId}-tab-${entry.id}`}
                  data-current={current}
                  aria-selected={current}
                  aria-controls={`${domId}-panel`}
                  aria-label={`${entry.label}, ${entry.count} ${entry.count === 1 ? "result" : "results"}`}
                  tabIndex={-1}
                  onMouseDown={e => e.preventDefault()}
                  onClick={() => { selectTab(entry.id); inputRef.current?.focus(); }}
                  onMouseEnter={e => { if (!current) e.currentTarget.style.color = "var(--text-secondary)"; }}
                  onMouseLeave={e => { if (!current) e.currentTarget.style.color = "var(--text-muted)"; }}
                  style={{
                    display: "flex", alignItems: "center", gap: 6, flexShrink: 0,
                    height: 36, padding: "0 10px", marginBottom: -1,
                    background: "transparent", border: "none",
                    borderBottom: `2px solid ${current ? "var(--accent)" : "transparent"}`,
                    color: current ? "var(--text-primary)" : "var(--text-muted)",
                    fontFamily: "inherit", fontSize: 12.5, fontWeight: 500,
                    whiteSpace: "nowrap", cursor: "pointer",
                    transition: "color 0.15s, border-color 0.15s",
                  }}
                >
                  <span>{entry.label}</span>
                  <span
                    aria-hidden="true"
                    style={{
                      fontSize: 10.5, fontWeight: 600, fontVariantNumeric: "tabular-nums",
                      padding: "1px 5px", borderRadius: 999, minWidth: 16, textAlign: "center",
                      color: current ? "var(--accent)" : "var(--text-muted)",
                      background: current ? "rgba(var(--accent-rgb), 0.12)" : "var(--bg-hover)",
                    }}
                  >
                    {entry.count}
                  </span>
                </button>
              );
            })}
          </div>
        )}

        {/* Results */}
        <div
          ref={panelRef}
          id={`${domId}-panel`}
          role={tabs.length > 0 ? "tabpanel" : undefined}
          aria-labelledby={tabs.length > 0 ? `${domId}-tab-${tab}` : undefined}
          tabIndex={-1}
          style={{ maxHeight: 380, overflowY: "auto", padding: 6, outline: "none" }}
        >
          {visible.length === 0 && query && (
            <div style={{ padding: "24px 16px", textAlign: "center", color: "var(--text-muted)", fontSize: 13 }}>
              No results for &ldquo;{query}&rdquo;
            </div>
          )}
          <div id={`${domId}-listbox`} role="listbox" aria-label={activeTabLabel}>
            {groups.map((group, groupIndex) => {
              const rows = group.items.map((r, itemIndex) => {
                const index = groupOffsets[groupIndex] + itemIndex;
                const isActive = index === activeIndex;
                return (
                  <button
                    key={r.id}
                    type="button"
                    role="option"
                    id={`${domId}-option-${index}`}
                    data-active={isActive}
                    aria-selected={isActive}
                    onClick={() => navigate(r.href)}
                    onMouseEnter={() => setSelected(index)}
                    style={{
                      display: "flex", alignItems: "center", gap: 10,
                      width: "100%", padding: "9px 12px", border: "none", borderRadius: 8,
                      background: isActive ? "var(--bg-hover)" : "transparent",
                      color: isActive ? "var(--text-primary)" : "var(--text-secondary)",
                      fontSize: 13.5, cursor: "pointer", textAlign: "left",
                      fontFamily: "inherit", transition: "background 0.1s",
                    }}
                  >
                    <ResultGlyph glyph={r.glyph} />
                    <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{r.label}</span>
                    {isActive && (
                      <kbd style={{ padding: "1px 4px", borderRadius: 3, border: "1px solid var(--border)", fontSize: 9, color: "var(--text-muted)", fontFamily: "inherit" }}>&#x23CE;</kbd>
                    )}
                  </button>
                );
              });
              if (!group.kind) return <React.Fragment key="results">{rows}</React.Fragment>;
              return (
                <div key={group.kind} role="group" aria-label={KIND_LABELS[group.kind]}>
                  <div style={{ fontSize: 10, fontWeight: 600, color: "var(--text-muted)", padding: "8px 12px 4px", letterSpacing: "0.04em" }}>
                    {KIND_LABELS[group.kind]}
                  </div>
                  {rows}
                </div>
              );
            })}
          </div>
        </div>

        {/* Footer */}
        <div style={{ padding: "8px 16px", borderTop: "1px solid var(--border)", display: "flex", alignItems: "center", gap: 16, fontSize: 10, color: "var(--text-muted)" }}>
          {hint("↑↓", "results")}
          {tabs.length > 1 && hint("←→", "categories")}
          {hint("⏎", "open")}
          {hint("esc", "close")}
        </div>
      </div>
    </>
  );
}

export default Sidebar;
