"use client";

import React, { useState, useEffect, useId, useMemo, useRef, useCallback, ReactNode } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { KaveonMark, KaveonWordmark } from "./KaveonMark";
import { useAuth } from "../auth/useAuth";
import { useTheme } from "../contexts/ThemeContext";
import { useGuardedNavigate } from "../contexts/NavigationGuardContext";
import { useRole } from "../hooks/useRole";
import { ConfirmModal } from "./ConfirmModal";
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
  const { recents, addRecent, removeRecent, clearRecents, conversationsClearedBy } = useRecents();
  // Clearing is destructive now: a conversation is only listed here, so
  // removing it from the list removes it. The dialog has to say so before
  // anything happens.
  const [pendingClear, setPendingClear] = useState<{ type?: RecentItem["type"]; conversations: number } | null>(null);
  const [clearing, setClearing] = useState(false);
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
                            onClick={() => {
                              const type = recentFilter === "all" ? undefined : recentFilter;
                              setPendingClear({ type, conversations: conversationsClearedBy(type) });
                              setRecentMenuOpen(false);
                            }}
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

      <ConfirmModal
        isOpen={pendingClear !== null}
        danger
        busy={clearing}
        title={pendingClear?.conversations ? "Clear recents and delete conversations" : "Clear recents"}
        message={
          pendingClear?.conversations
            ? `This removes the list and permanently deletes ${pendingClear.conversations} `
              + `${pendingClear.conversations === 1 ? "conversation" : "conversations"}, with every question `
              + "and answer in them. Conversations are only listed here, so there is nowhere to recover them from. "
              + "Dashboards, charts and datasets are only removed from the list."
            : "This removes these items from the list. Nothing they point to is deleted."
        }
        confirmLabel={pendingClear?.conversations ? "Delete and clear" : "Clear"}
        onConfirm={async () => {
          if (!pendingClear) return;
          setClearing(true);
          try {
            await clearRecents(pendingClear.type);
          } finally {
            setClearing(false);
            setPendingClear(null);
          }
        }}
        onCancel={() => { if (!clearing) setPendingClear(null); }}
      />
    </div>
  );
}

/* ─── Spotlight Search (Cmd+K) ─────────────────────────────────────────────── */

/** The kinds of match the search produces. */
type ResultKind = "recent" | "dashboard" | "chart" | "dataset" | "page";

/** A category, or the ranked union of every category. */
type TabId = ResultKind | "all";

/** Result kinds, the two recent kinds with no category, and the page glyphs. */
type GlyphName =
  | ResultKind | "query" | "chat" | "star" | "source" | "engine" | "docs" | "settings";

/** A run of characters the typed term accounts for, marked in the row. */
interface MatchRun {
  start: number;
  end: number;
}

/**
 * Something the palette can open, before any term has been scored against it.
 *
 * `aliases` is text that makes an entry findable without being worth showing —
 * a dashboard's description, a dataset's table, the words a reader is likely to
 * reach for when they want a page. `boost` lifts what the reader has already
 * shown an interest in.
 */
interface Candidate {
  id: string;
  label: string;
  detail: string;
  href: string;
  type: ResultKind;
  glyph: GlyphName;
  aliases: string[];
  boost: number;
  /** Dataset columns, which stand in for the detail line when one is the match. */
  columns?: string[];
  /** Offered with nothing typed: the destinations worth naming unprompted. */
  primary?: boolean;
  adminOnly?: boolean;
}

/** A candidate that matched, with its score and the characters to mark. */
interface SearchResult {
  id: string;
  label: string;
  detail: string;
  href: string;
  type: ResultKind;
  glyph: GlyphName;
  score: number;
  labelRuns: MatchRun[];
  detailRuns: MatchRun[];
}

interface SearchTab {
  id: TabId;
  label: string;
  count: number;
}

/**
 * Kind order is the resting order of the tab strip: what the reader opened
 * last, then the content they own, then where they can go. The strip never
 * reorders under the arrow keys, so ←/→ always lands where the reader expects;
 * inside the All tab the groups follow the ranking instead.
 */
const KIND_ORDER: ResultKind[] = ["recent", "dashboard", "chart", "dataset", "page"];

const KIND_LABELS: Record<ResultKind, string> = {
  recent: "Recent",
  dashboard: "Dashboards",
  chart: "Charts",
  dataset: "Datasets",
  page: "Navigation",
};

/** What a recent is, for the line under its title. */
const RECENT_NOUNS: Record<RecentItem["type"], string> = {
  dashboard: "Dashboard",
  chart: "Chart",
  dataset: "Dataset",
  query: "Saved query",
  chat: "Conversation",
};

/** A recent keeps its own glyph, so the Recent tab still shows what each row is. */
const RECENT_GLYPHS: Record<RecentItem["type"], GlyphName> = {
  dashboard: "dashboard",
  chart: "chart",
  dataset: "dataset",
  query: "query",
  chat: "chat",
};

/**
 * Every destination in Studio, each with the one line that says what it is for
 * and the words a reader might type instead of its name. The aliases are why
 * "connection" reaches Data sources and "dark mode" reaches Preferences.
 */
const PAGES: Candidate[] = [
  {
    id: "p-chat", label: "New Chat", href: "/home", type: "page", glyph: "chat", boost: 0, primary: true,
    detail: "Ask a question of your data in plain language",
    aliases: ["ask question prompt natural language conversation dlm nl to sql"],
  },
  {
    id: "p-library", label: "Library", href: "/workspace", type: "page", glyph: "dashboard", boost: 0, primary: true,
    detail: "Everything saved here — dashboards, charts and datasets",
    aliases: ["workspace saved browse mine all content"],
  },
  {
    id: "p-favorites", label: "Favorites", href: "/favorites", type: "page", glyph: "star", boost: 0,
    detail: "The dashboards and charts you starred",
    aliases: ["starred pinned bookmarks favourites"],
  },
  {
    id: "p-catalog", label: "Catalog", href: "/catalog", type: "page", glyph: "dataset", boost: 0, primary: true,
    detail: "Browse catalogs, schemas and tables in the warehouse",
    aliases: ["tables schemas metadata warehouse browse columns"],
  },
  {
    id: "p-lab", label: "SQL Lab", href: "/lab", type: "page", glyph: "query", boost: 0, primary: true,
    detail: "Write and run SQL against any connected source",
    aliases: ["query editor statement run sql console"],
  },
  {
    id: "p-queries", label: "Query Activity", href: "/lab/queries", type: "page", glyph: "query", boost: 0,
    detail: "Statements run in the Lab, and the ones you saved",
    aliases: ["saved queries history statements sql log"],
  },
  {
    id: "p-engine", label: "KaveonDB", href: "/engine", type: "page", glyph: "engine", boost: 0,
    detail: "Engine activity, query history and execution detail",
    aliases: ["engine performance plans latency history execution"],
  },
  {
    id: "p-activity", label: "Workspace Activity", href: "/workspace-activity", type: "page", glyph: "recent", boost: 0,
    detail: "Who changed what, across every dashboard and chart",
    aliases: ["audit history changes log timeline edits"],
  },
  {
    id: "p-lineage", label: "Lineage", href: "/workspace?tab=lineage", type: "page", glyph: "chart", boost: 0,
    detail: "How datasets, charts and dashboards depend on each other",
    aliases: ["dependencies graph upstream downstream impact"],
  },
  {
    id: "p-docs", label: "Documentation", href: "/docs", type: "page", glyph: "docs", boost: 0,
    detail: "Guides, SQL reference and architecture notes",
    aliases: ["help guide reference manual quickstart api"],
  },
  {
    id: "p-connections", label: "Connections", href: "/settings/connections", type: "page", glyph: "settings",
    boost: 0, adminOnly: true,
    detail: "KaveonDB's own connection, and where it keeps the control plane",
    aliases: ["kaveondb engine metadata control plane store credentials settings"],
  },
  {
    id: "p-storage", label: "Storage", href: "/settings/storage", type: "page", glyph: "settings",
    boost: 0, adminOnly: true,
    detail: "Where the warehouse keeps data, and for how long",
    aliases: ["retention lake parquet disk settings"],
  },
  {
    id: "p-sources", label: "Data sources", href: "/settings/data-sources", type: "page", glyph: "settings",
    boost: 0, adminOnly: true,
    detail: "External databases Kaveon queries over the wire",
    aliases: ["connection connect database postgres mysql fabric starrocks register driver external"],
  },
  {
    id: "p-maintenance", label: "Maintenance", href: "/settings/maintenance", type: "page", glyph: "settings",
    boost: 0, adminOnly: true,
    detail: "Housekeeping jobs, rebuilds and context regeneration",
    aliases: ["jobs rebuild cleanup vacuum regenerate settings"],
  },
  {
    id: "p-governance", label: "Governance", href: "/settings/governance", type: "page", glyph: "settings",
    boost: 0, adminOnly: true,
    detail: "Visibility rules and who may publish",
    aliases: ["policy roles permissions visibility published internal settings"],
  },
  {
    id: "p-catalog-access", label: "Catalog access", href: "/settings/catalog-access", type: "page", glyph: "settings",
    boost: 0, adminOnly: true,
    detail: "Which catalogs and schemas each role can reach",
    aliases: ["permissions grants access roles schemas settings"],
  },
  {
    id: "p-preferences", label: "Preferences", href: "/settings/preferences", type: "page", glyph: "settings", boost: 0,
    detail: "Your own theme and query defaults",
    aliases: ["theme dark mode light appearance profile defaults settings options"],
  },
  {
    id: "p-about", label: "About Kaveon", href: "/", type: "page", glyph: "page", boost: 0,
    detail: "What Kaveon is, and how the engine, the DLM and Studio fit together",
    aliases: ["home landing overview product pillars"],
  },
];

const GLYPH_PATHS: Record<GlyphName, ReactNode> = {
  recent: <><circle cx="12" cy="12" r="9" /><polyline points="12 7 12 12 16 14" /></>,
  dashboard: <><rect x="3" y="3" width="7" height="7" /><rect x="14" y="3" width="7" height="7" /><rect x="14" y="14" width="7" height="7" /><rect x="3" y="14" width="7" height="7" /></>,
  chart: <><line x1="5" y1="20" x2="19" y2="20" /><line x1="8" y1="20" x2="8" y2="12" /><line x1="12" y1="20" x2="12" y2="5" /><line x1="16" y1="20" x2="16" y2="15" /></>,
  dataset: <><rect x="3" y="4" width="18" height="16" rx="2" /><line x1="3" y1="10" x2="21" y2="10" /><line x1="9" y1="10" x2="9" y2="20" /></>,
  query: <><polyline points="5 8 9 12 5 16" /><line x1="12" y1="16" x2="19" y2="16" /></>,
  chat: <path d="M21 14a2 2 0 0 1-2 2H8l-4 4V5a2 2 0 0 1 2-2h13a2 2 0 0 1 2 2z" />,
  star: <polygon points="12 3.5 14.7 9.2 21 10 16.5 14.4 17.6 20.6 12 17.6 6.4 20.6 7.5 14.4 3 10 9.3 9.2" />,
  source: <><ellipse cx="12" cy="6" rx="8" ry="3" /><path d="M4 6v6c0 1.66 3.58 3 8 3s8-1.34 8-3V6" /><path d="M4 12v6c0 1.66 3.58 3 8 3s8-1.34 8-3v-6" /></>,
  engine: <><rect x="6" y="6" width="12" height="12" rx="2" /><path d="M10 3v3M14 3v3M10 18v3M14 18v3M3 10h3M3 14h3M18 10h3M18 14h3" /></>,
  docs: <><path d="M4 19V5a2 2 0 0 1 2-2h13v18H6a2 2 0 0 1-2-2z" /><line x1="8" y1="7.5" x2="15" y2="7.5" /><line x1="8" y1="11.5" x2="15" y2="11.5" /></>,
  settings: <><line x1="3.5" y1="8" x2="20.5" y2="8" /><circle cx="9" cy="8" r="2.4" /><line x1="3.5" y1="16" x2="20.5" y2="16" /><circle cx="15" cy="16" r="2.4" /></>,
  page: <><line x1="4" y1="12" x2="19" y2="12" /><polyline points="13 6 19 12 13 18" /></>,
};

function ResultGlyph({ glyph }: { glyph: GlyphName }) {
  return (
    <svg
      width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"
      style={{ flexShrink: 0, opacity: 0.65 }}
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
      style={{ animation: "spin 0.8s linear infinite" }}
    >
      <path d="M21 12a9 9 0 1 1-6.22-8.56" />
    </svg>
  );
}

/* ── Matching ─────────────────────────────────────────────────────────────── */

const WORD_BREAK = /[\s_\-./:,()[\]&%]/;

/** Whether `at` begins a word, counting punctuation breaks and camel humps. */
function startsWord(text: string, at: number): boolean {
  if (at <= 0) return true;
  const before = text[at - 1];
  if (WORD_BREAK.test(before)) return true;
  return before !== before.toUpperCase()
    && before === before.toLowerCase()
    && text[at] !== text[at].toLowerCase();
}

interface FieldMatch {
  score: number;
  runs: MatchRun[];
}

/** Collapse matched character positions into the fewest contiguous runs. */
function toRuns(indices: number[]): MatchRun[] {
  const runs: MatchRun[] = [];
  for (const at of indices) {
    const last = runs[runs.length - 1];
    if (last && last.end === at) last.end = at + 1;
    else runs.push({ start: at, end: at + 1 });
  }
  return runs;
}

/**
 * The term read as initials: every character has to land on the start of a
 * word, in order. This is how a reader types a title they already know — "ubr"
 * for Users by Region — so it scores just under a literal prefix.
 */
function matchInitials(text: string, hay: string, term: string): FieldMatch | null {
  if (term.length < 2) return null;
  const indices: number[] = [];
  let at = 0;
  for (const want of term) {
    while (at < hay.length && !(hay[at] === want && startsWord(text, at))) at += 1;
    if (at >= hay.length) return null;
    indices.push(at);
    at += 1;
  }
  return { score: 840, runs: toRuns(indices) };
}

/**
 * The term read loosely: its characters in order, gaps allowed. Scored well
 * below any literal match, lifted by how much of it landed on the start of a
 * word and cut by how far it had to stretch to find the rest.
 */
function matchLoose(text: string, hay: string, term: string): FieldMatch | null {
  if (term.length < 3) return null;
  const indices: number[] = [];
  let at = 0;
  for (const want of term) {
    const found = hay.indexOf(want, at);
    if (found < 0) return null;
    indices.push(found);
    at = found + 1;
  }
  const span = indices[indices.length - 1] - indices[0] + 1;
  const anchored = indices.filter((index) => startsWord(text, index)).length;
  const score = 300 + anchored * 14 - Math.min(90, (span - term.length) * 3);
  return { score: Math.max(150, score), runs: toRuns(indices) };
}

/**
 * How well `term` matches `text`, and where. The bands are far enough apart
 * that a weaker kind of match can never outrank a stronger one: the whole
 * string, then a prefix, then initials, then the start of a word inside it,
 * then any substring, and last the characters in order with gaps.
 */
function matchField(text: string, term: string): FieldMatch | null {
  if (!text) return null;
  const hay = text.toLowerCase();
  const at = hay.indexOf(term);
  if (at === 0) {
    return {
      score: hay.length === term.length ? 1000 : 900,
      runs: [{ start: 0, end: term.length }],
    };
  }
  if (at > 0) {
    return {
      score: startsWord(text, at) ? 700 : 520,
      runs: [{ start: at, end: at + term.length }],
    };
  }
  return matchInitials(text, hay, term) ?? matchLoose(text, hay, term);
}

/** The title carries full weight; what a row merely mentions carries less. */
const DETAIL_WEIGHT = 0.62;
const ALIAS_WEIGHT = 0.42;
const COLUMN_PREFIX = "Column ";

/**
 * Score one candidate against the term and resolve what its row should mark.
 *
 * The strongest field wins outright, so a title match always beats the same
 * term found in a description. A short title then scores a little above a long
 * one for the same kind of match, which is what settles "conn" on Connections
 * rather than on Catalog access.
 */
function rank(candidate: Candidate, term: string): SearchResult | null {
  const label = matchField(candidate.label, term);
  let detailText = candidate.detail;
  let detail = matchField(detailText, term);

  // A dataset is reached by its columns as often as by its name. When a column
  // is the better match, the row names that column instead of repeating a table
  // the reader never typed.
  if (candidate.columns) {
    let best: FieldMatch | null = null;
    let bestName = "";
    for (const name of candidate.columns) {
      const match = matchField(name, term);
      if (match && (!best || match.score > best.score)) { best = match; bestName = name; }
    }
    if (best && best.score > (label?.score ?? 0) && best.score > (detail?.score ?? 0)) {
      detailText = `${COLUMN_PREFIX}${bestName}`;
      detail = {
        score: best.score,
        runs: best.runs.map((run) => ({
          start: run.start + COLUMN_PREFIX.length,
          end: run.end + COLUMN_PREFIX.length,
        })),
      };
    }
  }

  let aliasScore = 0;
  for (const alias of candidate.aliases) {
    const match = matchField(alias, term);
    if (match && match.score > aliasScore) aliasScore = match.score;
  }

  const best = Math.max(
    label?.score ?? 0,
    (detail?.score ?? 0) * DETAIL_WEIGHT,
    aliasScore * ALIAS_WEIGHT,
  );
  if (best <= 0) return null;

  const brevity = 36 * (1 - Math.min(candidate.label.length, 48) / 48);
  return {
    id: candidate.id,
    label: candidate.label,
    detail: detailText,
    href: candidate.href,
    type: candidate.type,
    glyph: candidate.glyph,
    score: best + brevity + candidate.boost,
    labelRuns: label?.runs ?? [],
    detailRuns: detail?.runs ?? [],
  };
}

/** Mark the characters the term accounts for, so a row says why it is here. */
function Marked({ text, runs, strong }: { text: string; runs: MatchRun[]; strong: string }) {
  if (runs.length === 0) return <>{text}</>;
  const parts: ReactNode[] = [];
  let at = 0;
  runs.forEach((run, index) => {
    if (run.start > at) parts.push(text.slice(at, run.start));
    parts.push(
      <mark
        key={index}
        style={{ background: "transparent", color: strong, fontWeight: 600 }}
      >
        {text.slice(run.start, run.end)}
      </mark>,
    );
    at = run.end;
  });
  if (at < text.length) parts.push(text.slice(at));
  return <>{parts}</>;
}

/* ── The searchable corpus ────────────────────────────────────────────────── */

/** The API answers some lists bare and others inside an envelope. */
function listOf(payload: unknown): Record<string, unknown>[] {
  if (Array.isArray(payload)) return payload as Record<string, unknown>[];
  if (payload && typeof payload === "object") {
    const record = payload as Record<string, unknown>;
    for (const key of ["recent", "result", "items", "datasets", "dashboards", "charts"]) {
      if (Array.isArray(record[key])) return record[key] as Record<string, unknown>[];
    }
  }
  return [];
}

function str(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

const CHART_TYPE_LABELS: Record<string, string> = {
  bar: "Bar chart",
  stacked_bar: "Stacked bar chart",
  line: "Line chart",
  area: "Area chart",
  pie: "Pie chart",
  scatter: "Scatter chart",
  table: "Table",
  world_map: "Map",
  heatmap: "Heatmap",
  funnel: "Funnel chart",
  big_number: "Metric",
  big_number_trend: "Metric with trend",
};

/** A description reads as a single line in a result row, cut at a word. */
function oneLine(value: string, limit = 92): string {
  const flat = value.replace(/\s+/g, " ").trim();
  if (flat.length <= limit) return flat;
  const cut = flat.slice(0, limit);
  const at = cut.lastIndexOf(" ");
  const kept = at > limit * 0.6 ? cut.slice(0, at) : cut;
  return `${kept.replace(/[,;:.—-]+$/, "")}…`;
}

function dashboardCandidates(payload: unknown): Candidate[] {
  const out: Candidate[] = [];
  for (const row of listOf(payload)) {
    const id = str(row.id);
    const label = str(row.name);
    if (!id || !label) continue;
    const description = str(row.description);
    let charts = 0;
    if (Array.isArray(row.charts)) charts = row.charts.length;
    else if (typeof row.charts === "string") {
      try {
        const parsed: unknown = JSON.parse(row.charts);
        if (Array.isArray(parsed)) charts = parsed.length;
      } catch { charts = 0; }
    }
    const count = `${charts} ${charts === 1 ? "chart" : "charts"}`;
    out.push({
      id: `d-${id}`,
      label,
      detail: description ? oneLine(description) : count,
      href: `/dashboards/${id}/view`,
      type: "dashboard",
      glyph: "dashboard",
      aliases: [description, count, "dashboard"],
      boost: row.favorite === true ? 50 : 0,
    });
  }
  return out;
}

function chartCandidates(payload: unknown): Candidate[] {
  const out: Candidate[] = [];
  for (const row of listOf(payload)) {
    const id = str(row.id);
    const label = str(row.name);
    if (!id || !label) continue;
    const typeLabel = CHART_TYPE_LABELS[str(row.chart_type)] ?? "Chart";
    const dataset = str(row.dataset_name);
    out.push({
      id: `c-${id}`,
      label,
      detail: dataset ? `${typeLabel} in ${dataset}` : typeLabel,
      href: `/charts/${id}`,
      type: "chart",
      glyph: "chart",
      aliases: [str(row.description), typeLabel, dataset, "chart"],
      boost: row.favorite === true ? 50 : 0,
    });
  }
  return out;
}

function datasetCandidates(payload: unknown): Candidate[] {
  const out: Candidate[] = [];
  for (const row of listOf(payload)) {
    const id = str(row.id);
    const label = str(row.name);
    if (!id || !label) continue;
    const table = str(row.table_name);
    const schema = str(row.schema_name);
    const description = str(row.description);
    const columns = Array.isArray(row.columns)
      ? row.columns
        .map((column) => str((column as Record<string, unknown>)?.column_name)
          || str((column as Record<string, unknown>)?.name))
        .filter(Boolean)
      : [];
    let detail = description ? oneLine(description) : "Dataset";
    if (table) detail = schema ? `${table} in ${schema}` : table;
    out.push({
      id: `ds-${id}`,
      label,
      detail,
      href: `/datasets/${id}`,
      type: "dataset",
      glyph: "dataset",
      aliases: [description, str(row.database_name), schema, table, "dataset table"],
      columns,
      boost: row.favorite === true ? 50 : 0,
    });
  }
  return out;
}

/**
 * The corpus is fetched once per palette session and matched in the browser,
 * so every keystroke after the first is answered without a request. It is held
 * past close for a couple of minutes, which is what makes reopening instant;
 * a stale copy is still shown at once and replaced when the refresh lands.
 */
const CORPUS_TTL_MS = 120_000;
/** How long a fetch may run before the reader is told one is running. */
const PROGRESS_AFTER_MS = 400;

let corpusItems: Candidate[] | null = null;
let corpusFetchedAt = 0;
let corpusInFlight: Promise<Candidate[]> | null = null;

function fetchCorpus(): Promise<Candidate[]> {
  if (corpusInFlight) return corpusInFlight;
  const json = (path: string) => msalFetch(path)
    .then((response) => (response.ok ? response.json() : null))
    .catch(() => null);
  corpusInFlight = Promise.all([
    json("/api/v1/dashboards"),
    json("/api/v1/charts"),
    json("/api/v1/datasets/summary"),
  ]).then(([dashboards, charts, datasets]) => {
    const items = [
      ...dashboardCandidates(dashboards),
      ...chartCandidates(charts),
      ...datasetCandidates(datasets),
    ];
    corpusItems = items;
    corpusFetchedAt = Date.now();
    return items;
  }).finally(() => { corpusInFlight = null; });
  return corpusInFlight;
}

/** The time since a recent was opened, as the row's one disambiguating fact. */
const RELATIVE_STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["second", 60], ["minute", 60], ["hour", 24], ["day", 7], ["week", 4.345], ["month", 12],
];

const RELATIVE_FORMAT = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

function openedAgo(timestamp: number): string {
  let amount = (timestamp - Date.now()) / 1000;
  for (const [unit, span] of RELATIVE_STEPS) {
    if (Math.abs(amount) < span) return RELATIVE_FORMAT.format(Math.round(amount), unit);
    amount /= span;
  }
  return RELATIVE_FORMAT.format(Math.round(amount), "year");
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

/** How many of a category the All tab previews before its own tab is needed. */
const ALL_PREVIEW = 5;

/**
 * How far below the best match a result may score and still be worth listing.
 *
 * Reading the characters in order with gaps finds a great many weak matches —
 * "conn" is in "Avg Carbon Intensity" if you let it stretch far enough. Those
 * are real matches and worth having when nothing better exists, but alongside
 * a title that starts with the term they are only noise, and they would inflate
 * every count in the tab strip. So the bar rises with the quality of the best
 * answer rather than sitting at a fixed score.
 */
const RELEVANCE_FLOOR = 0.45;

function SpotlightSearch({ recents, onClose, onNavigate }: {
  recents: RecentItem[];
  onClose: () => void;
  onNavigate: (href: string) => void;
}) {
  const { isAdmin } = useRole();
  const [query, setQuery] = useState("");
  const [corpus, setCorpus] = useState<Candidate[]>(() => corpusItems ?? []);
  const [loading, setLoading] = useState(false);
  const [pinnedTab, setPinnedTab] = useState<TabId | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const stripRef = useRef<HTMLDivElement>(null);
  const restoreFocusRef = useRef(true);
  const domId = useId();

  // The field takes focus on open, and whatever opened the overlay takes it
  // back when the overlay closes without going anywhere.
  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    inputRef.current?.focus();
    return () => { if (restoreFocusRef.current) opener?.focus?.(); };
  }, []);

  // A fresh corpus is already in hand, so the first keystroke is answered
  // without a request. A stale one is shown immediately and refreshed behind
  // the reader; only an empty one is ever worth reporting as progress.
  useEffect(() => {
    if (corpusItems && Date.now() - corpusFetchedAt < CORPUS_TTL_MS) return;
    let live = true;
    const reveal: ReturnType<typeof setTimeout> | undefined = corpusItems
      ? undefined
      : setTimeout(() => { if (live) setLoading(true); }, PROGRESS_AFTER_MS);
    fetchCorpus().then((items) => { if (live) setCorpus(items); })
      .finally(() => { if (live) { clearTimeout(reveal); setLoading(false); } });
    return () => { live = false; clearTimeout(reveal); };
  }, []);

  const pages = useMemo(
    () => (isAdmin ? PAGES : PAGES.filter((page) => !page.adminOnly)),
    [isAdmin],
  );

  /**
   * Recents are their own category rather than a boost alone, because where the
   * reader just was is the single most likely place they want to go back to.
   * The same item still appears under its own category; the copy there carries
   * the recency lift so it ranks above everything the reader has not touched.
   */
  const recentHrefs = useMemo(() => new Set(recents.map((item) => item.href)), [recents]);

  const results = useMemo<SearchResult[]>(() => {
    const term = query.trim().toLowerCase();
    const recentCandidates: Candidate[] = recents.map((item) => ({
      id: `r-${item.type}-${item.id}`,
      label: item.label,
      detail: `${RECENT_NOUNS[item.type] ?? "Item"} opened ${openedAgo(item.timestamp)}`,
      href: item.href,
      type: "recent",
      glyph: RECENT_GLYPHS[item.type] ?? "recent",
      aliases: [RECENT_NOUNS[item.type] ?? ""],
      boost: 0,
    }));

    if (!term) {
      // Nothing typed: where the reader was, then where they can go. Both are
      // offered unranked, in the order they are useful in.
      const resting = (items: Candidate[]) => items.map((candidate) => ({
        ...candidate,
        score: 0,
        labelRuns: [] as MatchRun[],
        detailRuns: [] as MatchRun[],
      }));
      return [
        ...resting(recentCandidates.slice(0, 7)),
        ...resting(pages.filter((page) => page.primary)),
      ];
    }

    const scored: SearchResult[] = [];
    const push = (candidates: Candidate[]) => {
      for (const candidate of candidates) {
        const result = rank(candidate, term);
        if (result) scored.push(result);
      }
    };
    push(recentCandidates);
    push(corpus.map((candidate) => (recentHrefs.has(candidate.href)
      ? { ...candidate, boost: candidate.boost + 110 }
      : candidate)));
    push(pages);
    scored.sort((a, b) => b.score - a.score
      || KIND_ORDER.indexOf(a.type) - KIND_ORDER.indexOf(b.type)
      || a.label.localeCompare(b.label));
    const floor = (scored[0]?.score ?? 0) * RELEVANCE_FLOOR;
    return scored.filter((result) => result.score >= floor);
  }, [query, recents, corpus, pages, recentHrefs]);

  const tabs = useMemo(() => buildTabs(results), [results]);

  const tab = useMemo<TabId>(() => {
    if (pinnedTab !== null && tabs.some((entry) => entry.id === pinnedTab)) return pinnedTab;
    return defaultTab(results);
  }, [pinnedTab, tabs, results]);

  /**
   * The All tab keeps the headed groups the flat list had, ordered by how well
   * each category matched rather than by a fixed hierarchy, and shows the top
   * few of each. A category tab is one unheaded run of every match it holds,
   * because the tab is already the heading.
   */
  const groups = useMemo<{ kind: ResultKind | null; items: SearchResult[]; total: number }[]>(() => {
    if (tab !== "all") {
      const items = results.filter((result) => result.type === tab);
      return [{ kind: null, items, total: items.length }];
    }
    const byKind = KIND_ORDER
      .map((kind) => ({ kind: kind as ResultKind | null, items: results.filter((result) => result.type === kind) }))
      .filter((group) => group.items.length > 0);
    const best = (items: SearchResult[]) => items.reduce((top, item) => Math.max(top, item.score), 0);
    byKind.sort((a, b) => {
      if (a.kind === "recent") return -1;
      if (b.kind === "recent") return 1;
      return best(b.items) - best(a.items)
        || KIND_ORDER.indexOf(a.kind as ResultKind) - KIND_ORDER.indexOf(b.kind as ResultKind);
    });
    return byKind.map((group) => ({
      kind: group.kind,
      items: group.items.slice(0, ALL_PREVIEW),
      total: group.items.length,
    }));
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

  // The cursor is held by identity, not by position, so a corpus landing
  // mid-type never slides a different row under the reader's finger. A new
  // term has no previous row to keep, and falls back to the top of the list.
  const activeIndex = useMemo(() => {
    if (visible.length === 0) return -1;
    const at = selectedId === null ? -1 : visible.findIndex((result) => result.id === selectedId);
    return at < 0 ? 0 : at;
  }, [visible, selectedId]);
  const activeResult = activeIndex < 0 ? null : visible[activeIndex];
  const activeTabLabel = tabs.find((entry) => entry.id === tab)?.label ?? "Results";

  const selectTab = useCallback((id: TabId) => {
    setPinnedTab(id);
    setSelectedId(null);
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

  const stepRow = (to: number) => {
    if (visible.length === 0) return;
    setSelectedId(visible[Math.max(0, Math.min(to, visible.length - 1))].id);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        stepRow(activeIndex + 1);
        break;
      case "ArrowUp":
        e.preventDefault();
        stepRow(activeIndex - 1);
        break;
      case "Home":
        if (visible.length > 0) { e.preventDefault(); stepRow(0); }
        break;
      case "End":
        if (visible.length > 0) { e.preventDefault(); stepRow(visible.length - 1); }
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
          position: "fixed", top: "max(44px, 12vh)", left: "50%", transform: "translateX(-50%)",
          width: "90%", maxWidth: 580,
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
            placeholder="Search dashboards, charts, datasets and pages"
            value={query}
            onChange={(e) => { setQuery(e.target.value); setPinnedTab(null); setSelectedId(null); }}
            style={{
              flex: 1, minWidth: 0, border: "none", outline: "none", background: "transparent",
              fontSize: 15, color: "var(--text-primary)", fontFamily: "inherit",
            }}
          />
          {/* The slot is held open whether or not a fetch is running, so nothing
              in the field moves when one starts or finishes. */}
          <span style={{ width: 14, height: 14, flexShrink: 0, display: "flex" }}>
            {loading && <SearchProgress />}
          </span>
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
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => { selectTab(entry.id); inputRef.current?.focus(); }}
                  onMouseEnter={(e) => { if (!current) e.currentTarget.style.color = "var(--text-secondary)"; }}
                  onMouseLeave={(e) => { if (!current) e.currentTarget.style.color = "var(--text-muted)"; }}
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

        {/* Categories change under the arrow keys while focus stays in the
            field, so the move is announced and not only drawn. */}
        <span
          role="status"
          aria-live="polite"
          style={{
            position: "absolute", width: 1, height: 1, margin: -1, padding: 0,
            overflow: "hidden", clipPath: "inset(50%)", whiteSpace: "nowrap", border: 0,
          }}
        >
          {tabs.length > 0 ? `${activeTabLabel}, ${visible.length} ${visible.length === 1 ? "result" : "results"}` : ""}
        </span>

        {/* Results */}
        <div
          ref={panelRef}
          id={`${domId}-panel`}
          role={tabs.length > 0 ? "tabpanel" : undefined}
          aria-labelledby={tabs.length > 0 ? `${domId}-tab-${tab}` : undefined}
          tabIndex={-1}
          style={{ maxHeight: "min(424px, 52vh)", overflowY: "auto", padding: 6, outline: "none" }}
        >
          {visible.length === 0 && query.trim() && (
            <div style={{ padding: "14px 12px", color: "var(--text-muted)", fontSize: 12.5 }}>
              Nothing here matches &ldquo;{query.trim()}&rdquo;.
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
                    aria-label={`${r.label}. ${r.detail}`}
                    onClick={() => navigate(r.href)}
                    onMouseEnter={() => setSelectedId(r.id)}
                    style={{
                      display: "flex", alignItems: "center", gap: 11,
                      width: "100%", minHeight: 48, padding: "7px 12px",
                      border: "none", borderRadius: 8,
                      background: isActive ? "color-mix(in srgb, var(--accent) 9%, transparent)" : "transparent",
                      color: "var(--text-secondary)",
                      cursor: "pointer", textAlign: "left", fontFamily: "inherit",
                      transition: "background 0.1s",
                    }}
                  >
                    <ResultGlyph glyph={r.glyph} />
                    <span style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", gap: 2 }}>
                      <span style={{ fontSize: 13.5, lineHeight: "18px", color: "var(--text-primary)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                        <Marked text={r.label} runs={r.labelRuns} strong="var(--text-primary)" />
                      </span>
                      <span style={{ fontSize: 11.5, lineHeight: "15px", color: "var(--text-muted)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                        <Marked text={r.detail} runs={r.detailRuns} strong="var(--text-secondary)" />
                      </span>
                    </span>
                    {isActive && (
                      <kbd style={{ padding: "1px 4px", borderRadius: 3, border: "1px solid var(--border)", fontSize: 9, color: "var(--text-muted)", fontFamily: "inherit" }}>&#x23CE;</kbd>
                    )}
                  </button>
                );
              });
              if (!group.kind) return <React.Fragment key="results">{rows}</React.Fragment>;
              return (
                <div key={group.kind} role="group" aria-label={KIND_LABELS[group.kind]}>
                  <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: 8, padding: "8px 12px 4px" }}>
                    <span style={{ fontSize: 10, fontWeight: 600, color: "var(--text-muted)", letterSpacing: "0.04em" }}>
                      {KIND_LABELS[group.kind]}
                    </span>
                    {/* Said only when the preview is hiding something, so the
                        reader knows the category's own tab holds more. */}
                    {group.total > group.items.length && (
                      <span style={{ fontSize: 10, color: "var(--text-muted)", fontVariantNumeric: "tabular-nums" }}>
                        {group.total} matches
                      </span>
                    )}
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
