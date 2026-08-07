"use client";

import React, { useState, useEffect, useRef, useCallback, ReactNode } from "react";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { KaveonMark } from "./KaveonMark";
import { useAuth } from "../auth/useAuth";
import { useTheme } from "../contexts/ThemeContext";
import { useRole } from "../hooks/useRole";
import { useRecents, RecentItem } from "../hooks/useRecents";

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
  router,
  isAdmin,
}: {
  account: { name?: string; email?: string } | null;
  collapsed: boolean;
  theme: string;
  toggleTheme: () => void;
  logout: () => Promise<void>;
  router: ReturnType<typeof useRouter>;
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
      {open && !collapsed && (
        <div
          style={{
            position: "absolute",
            bottom: "100%",
            left: 8,
            right: 8,
            marginBottom: 6,
            background: "var(--bg-surface)",
            border: "1px solid var(--border)",
            borderRadius: 10,
            padding: "6px",
            boxShadow: "var(--shadow-lg)",
            zIndex: 200,
          }}
        >
          {/* Appearance */}
          {menuItem(
            theme === "dark" ? "Light mode" : "Dark mode",
            toggleTheme,
            theme === "dark" ? <SunIcon /> : <MoonIcon />,
          )}

          <div style={{ height: 1, background: "var(--border)", margin: "4px 8px" }} />

          {/* Data Sources */}
          {menuItem("Data Sources", () => router.push("/data-sources"),
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>
          )}

          {/* Configurations (admin only) */}
          {isAdmin && menuItem("Configurations", () => router.push("/settings/system"),
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
          )}

          {/* Learn More */}
          {menuItem("Learn More", () => router.push("/docs"),
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>
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
            width: 26,
            height: 26,
            borderRadius: "50%",
            background: "linear-gradient(135deg, #6db3ed 0%, #4A9EE8 50%, #2d7dd2 100%)",
            color: "white",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: 10,
            fontWeight: 700,
            letterSpacing: "0.5px",
            flexShrink: 0,
            boxShadow: "0 2px 6px rgba(0,0,0,0.15)",
          }}
        >
          {getInitials(account?.name)}
        </div>
        {!collapsed && (
          <div style={{ overflow: "hidden", flex: 1, textAlign: "left" }}>
            <div style={{
              fontSize: 13,
              fontWeight: 600,
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

export function Sidebar({ children }: SidebarProps) {
  const { account, logout } = useAuth();
  const { theme, toggleTheme } = useTheme();
  const { recents, removeRecent } = useRecents();
  const router = useRouter();
  const { isAdmin } = useRole();
  const pathname = usePathname();

  const [collapsed, setCollapsed] = useState<boolean>(() => {
    if (typeof window !== "undefined") {
      return localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true";
    }
    return false;
  });

  // Persist collapse state
  useEffect(() => {
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(collapsed));
  }, [collapsed]);

  const width = collapsed ? COLLAPSED_WIDTH : EXPANDED_WIDTH;

  const navItems: NavItem[] = [
    {
      label: "New Chat",
      href: "/",
      icon: <span style={{ display: "flex", alignItems: "center" }}><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg></span>,
      exact: true,
    },
    {
      label: "Workspace",
      href: "/workspace",
      icon: <WorkspaceIcon />,
    },
    {
      label: "SQL Lab",
      href: "/lab",
      icon: <SqlLabIcon />,
    },
  ];

  function isActive(item: NavItem): boolean {
    if (item.exact) return pathname === item.href;
    return pathname?.startsWith(item.href) ?? false;
  }

  const sidebarStyle: React.CSSProperties = {
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
      {/* Sidebar */}
      <aside style={sidebarStyle}>

        {/* Collapse toggle */}
        <button
          type="button"
          onClick={() => setCollapsed(v => !v)}
          style={collapseButtonStyle}
          title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
        >
          {collapsed ? <PanelLeftOpenIcon /> : <PanelLeftCloseIcon />}
        </button>

        {/* Brand area */}
        <div style={{
          display: "flex",
          alignItems: "center",
          justifyContent: collapsed ? "center" : "flex-start",
          padding: collapsed ? "16px 0" : "16px 20px",
          borderBottom: "1px solid var(--border)",
          minHeight: 60,
          overflow: "hidden",
          flexShrink: 0,
          transition: `padding ${TRANSITION}`,
        }}>
          {collapsed ? (
            <div style={{
              transform: "scale(1.15)",
              animation: "kaveon-breathe 3s ease-in-out infinite",
            }}>
              <KaveonMark size={26} />
            </div>
          ) : (
            <svg width="160" height="28" viewBox="60 50 1180 200" fill="none" xmlns="http://www.w3.org/2000/svg" style={{ shapeRendering: "geometricPrecision" }}>
              <g fill="var(--text-primary)">
                <rect x="90" y="70" width="20" height="165" />
                <polygon points="108.73,161.20 215.73,86.39 204.27,70 97.27,144.80" />
                <polygon points="97.51,161.36 209.51,235 220.49,218.29 108.49,144.64" />
                <path d="M 260 235 L 330 70 L 350 70 L 420 235 L 397 235 L 340 104 L 283 235 Z" />
                <path d="M 465 70 L 488 70 L 545 201 L 602 70 L 625 70 L 555 235 L 535 235 Z" />
                <rect x="675" y="70" width="20" height="165" />
                <rect x="675" y="70" width="130" height="20" />
                <rect x="675" y="142.5" width="108" height="20" />
                <rect x="675" y="215" width="130" height="20" />
                <rect x="1060" y="70" width="20" height="165" />
                <rect x="1195" y="70" width="20" height="165" />
                <polygon points="1062.53,83.30 1197.53,235 1212.47,221.70 1077.47,70" />
              </g>
              <path d="M 966.25 215.29 A 72.5 72.5 0 1 0 893.75 215.29" fill="none" stroke="#4A9EE8" strokeWidth="20" strokeLinecap="butt" />
            </svg>
          )}
        </div>

        {/* Search bar */}
        <div style={{
          padding: collapsed ? "12px 0" : "12px 12px",
          flexShrink: 0,
          display: "flex",
          justifyContent: collapsed ? "center" : "stretch",
          transition: `padding ${TRANSITION}`,
        }}>
          <div
            role="button"
            tabIndex={0}
            title="Search"
            aria-label="Search"
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: collapsed ? "8px" : "8px 12px",
              borderRadius: 8,
              background: "var(--bg-hover)",
              border: "1px solid var(--border)",
              cursor: "pointer",
              color: "var(--text-muted)",
              fontSize: 13,
              width: collapsed ? 40 : "100%",
              justifyContent: collapsed ? "center" : "flex-start",
              transition: `width ${TRANSITION}, padding ${TRANSITION}`,
              flexShrink: 0,
              overflow: "hidden",
              whiteSpace: "nowrap",
            }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0 }}>
              <circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
            </svg>
            {!collapsed && (
              <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>Search</span>
            )}
          </div>
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
                  if (item.href === "/" && pathname === "/") {
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
                  fontSize: 13.5,
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
        <div style={{ height: 1, background: "var(--border)", margin: "8px 12px", flexShrink: 0 }} />

        {/* Conversations + Pinned */}
        <div style={{ flex: 1, overflow: "auto", padding: collapsed ? "0" : "0 8px" }}>
          {!collapsed && (
            <>
              {/* Recent items */}
              {recents.length > 0 && (
                <>
                  {recents.length >= 3 && (
                    <div style={{
                      fontSize: 10,
                      fontWeight: 600,
                      letterSpacing: "1px",
                      textTransform: "uppercase",
                      color: "var(--text-faint)",
                      padding: "8px 10px 4px",
                      userSelect: "none",
                    }}>
                      Recent
                    </div>
                  )}
                  {recents.map((item) => (
                    <button
                      key={item.id}
                      type="button"
                      onClick={() => router.push(item.href)}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 8,
                        width: "100%",
                        padding: "7px 10px",
                        border: "none",
                        background: "transparent",
                        color: "var(--text-secondary)",
                        fontSize: 12.5,
                        cursor: "pointer",
                        borderRadius: 6,
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
                      <span style={{ flexShrink: 0, display: "flex", alignItems: "center", color: "var(--text-muted)", opacity: 0.7 }}>
                        {item.type === "chat" ? (
                          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
                        ) : item.type === "dashboard" ? (
                          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>
                        ) : item.type === "chart" ? (
                          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/></svg>
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
                          removeRecent(item.id);
                          if (pathname === item.href) router.push("/");
                        }}
                        style={{
                          display: "none",
                          alignItems: "center",
                          justifyContent: "center",
                          width: 18,
                          height: 18,
                          borderRadius: 4,
                          fontSize: 14,
                          color: "var(--text-secondary)",
                          flexShrink: 0,
                          cursor: "pointer",
                        }}
                      >
                        ×
                      </span>
                    </button>
                  ))}
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
          router={router}
          isAdmin={isAdmin}
        />
      </aside>

      {/* Main content */}
      <div style={{
        flex: 1,
        marginLeft: width,
        transition: `margin-left ${TRANSITION}`,
        minWidth: 0,
        overflow: "auto",
      }}>
        {children}
      </div>
    </div>
  );
}

export default Sidebar;
