"use client";

import React, { useState, useEffect, ReactNode } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { KaveonMark, KaveonWordmark } from "./KaveonMark";
import { useAuth } from "../auth/useAuth";
import { useTheme } from "../contexts/ThemeContext";
import { useRole } from "../hooks/useRole";

const SIDEBAR_COLLAPSED_KEY = "kaveon-sidebar-collapsed";
const EXPANDED_WIDTH = 260;
const COLLAPSED_WIDTH = 64;
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

function ChevronLeftIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="15 18 9 12 15 6" />
    </svg>
  );
}

function ChevronRightIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="9 18 15 12 9 6" />
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

export function Sidebar({ children }: SidebarProps) {
  const { account } = useAuth();
  const { theme, toggleTheme } = useTheme();
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
      label: "Chat",
      href: "/",
      icon: <ChatIcon />,
      exact: true,
      badge: <NewBadge />,
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
    {
      label: "Data Sources",
      href: "/data-sources",
      icon: <DataSourcesIcon />,
    },
    {
      label: "Settings",
      href: "/settings/system",
      icon: <SettingsIcon />,
      adminOnly: true,
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
    background: "var(--bg-surface)",
    borderRight: "1px solid var(--border)",
    display: "flex",
    flexDirection: "column",
    transition: `width ${TRANSITION}, min-width ${TRANSITION}, max-width ${TRANSITION}`,
    overflow: "hidden",
    zIndex: 100,
  };

  const collapseButtonStyle: React.CSSProperties = {
    position: "absolute",
    top: 20,
    right: -12,
    width: 24,
    height: 24,
    borderRadius: "50%",
    background: "var(--bg-surface)",
    border: "1px solid var(--border)",
    cursor: "pointer",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    color: "var(--text-secondary)",
    zIndex: 10,
    boxShadow: "0 1px 4px rgba(0,0,0,0.08)",
    flexShrink: 0,
    transition: `color ${TRANSITION}`,
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
          {collapsed ? <ChevronRightIcon /> : <ChevronLeftIcon />}
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
              <KaveonMark size={28} />
            </div>
          ) : (
            <div style={{ display: "flex", alignItems: "center", gap: 8, overflow: "hidden" }}>
              <KaveonWordmark height={18} />
            </div>
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
            title="Search everything"
            aria-label="Search everything"
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
              <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>Search everything...</span>
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

        {/* Pinned + Recent (placeholder) */}
        <div style={{ flex: 1 }} />

        {/* Footer */}
        <div style={{
          borderTop: "1px solid var(--border)",
          padding: collapsed ? "12px 0" : "12px",
          display: "flex",
          flexDirection: "column",
          gap: 8,
          flexShrink: 0,
        }}>
          {/* Theme toggle */}
          <button
            type="button"
            onClick={toggleTheme}
            title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
            aria-label={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: collapsed ? "8px 0" : "8px 10px",
              justifyContent: collapsed ? "center" : "flex-start",
              borderRadius: 7,
              border: "none",
              background: "transparent",
              cursor: "pointer",
              color: "var(--text-secondary)",
              fontSize: 13.5,
              width: "100%",
              transition: `background ${TRANSITION}, color ${TRANSITION}`,
            }}
          >
            <span style={{ flexShrink: 0, display: "flex", alignItems: "center" }}>
              {theme === "dark" ? <SunIcon /> : <MoonIcon />}
            </span>
            {!collapsed && (
              <span style={{ whiteSpace: "nowrap" }}>
                {theme === "dark" ? "Light mode" : "Dark mode"}
              </span>
            )}
          </button>

          {/* User avatar */}
          <div style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: collapsed ? "6px 0" : "6px 10px",
            justifyContent: collapsed ? "center" : "flex-start",
            overflow: "hidden",
          }}>
            <div
              title={collapsed ? (account?.name ?? "User") : undefined}
              style={{
                width: 32,
                height: 32,
                borderRadius: "50%",
                background: "linear-gradient(135deg, #6db3ed 0%, #4A9EE8 50%, #2d7dd2 100%)",
                color: "white",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                fontSize: 12,
                fontWeight: 700,
                letterSpacing: "0.5px",
                flexShrink: 0,
                boxShadow: "0 2px 6px rgba(0,0,0,0.15)",
              }}
            >
              {getInitials(account?.name)}
            </div>
            {!collapsed && (
              <div style={{ overflow: "hidden", flex: 1 }}>
                <div style={{
                  fontSize: 13,
                  fontWeight: 600,
                  color: "var(--text-primary)",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}>
                  {account?.name ?? "User"}
                </div>
                <div style={{
                  fontSize: 11,
                  color: "var(--text-muted)",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}>
                  {account?.email ?? ""}
                </div>
              </div>
            )}
          </div>
        </div>
      </aside>

      {/* Main content */}
      <div style={{
        flex: 1,
        marginLeft: width,
        transition: `margin-left ${TRANSITION}`,
        minWidth: 0,
      }}>
        {children}
      </div>
    </div>
  );
}

export default Sidebar;
