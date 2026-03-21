"use client";

import { APP_DISPLAY_NAME, APP_LOGO_URL } from "../constants/branding";
import { LoomXLogo } from "./LoomXLogo";
import { SettingsModal } from "./SettingsModal";

import Link from "next/link";
import { ReactNode, useState } from "react";
import { useAuth } from "../auth/useAuth";
import { usePathname, useRouter } from "next/navigation";
import { useTheme } from "../contexts/ThemeContext";
import { useRole } from "../hooks/useRole";

interface LayoutProps {
  children: ReactNode;
}

export function Layout({ children }: LayoutProps) {
  const { account, isAuthenticated, login, logout, role } = useAuth();
  const { primaryColor, gradientColors } = useTheme();
  const { isAdmin } = useRole();
  const pathname = usePathname();
  const router = useRouter();
  const [isRevolving, setIsRevolving] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);

  // Get user initials from name
  const getInitials = (name: string | undefined) => {
    if (!name) return "U";
    const parts = name.trim().split(" ");
    if (parts.length === 1) return parts[0].charAt(0).toUpperCase();
    return (parts[0].charAt(0) + parts[parts.length - 1].charAt(0)).toUpperCase();
  };

  const isLabPage = pathname === "/lab";
  const isSqlActivityPage = pathname === "/lab/queries";
  const isChartBuildPage = pathname === "/charts/new/build";
  const isChartsPage = pathname?.startsWith("/charts") || false;
  const isDashboardsPage = pathname?.startsWith("/dashboards") || false;
  const isDatasetsPage = pathname?.startsWith("/datasets") || false;
  const isWidePage =
    isSqlActivityPage ||
    isChartBuildPage ||
    isChartsPage ||
    isDashboardsPage ||
    isDatasetsPage;
  const isHomePage = pathname === "/";

  const handleLogoClick = () => {
    setIsRevolving(true);
    setTimeout(() => {
      setIsRevolving(false);
      if (pathname === '/') {
        // Already on home — hard reload to refresh all data
        window.location.reload();
      } else {
        window.location.href = '/';
      }
    }, 400);
  };

  return (
    <>
      <header className="app-header" style={{ paddingLeft: "0.5rem" }}>
        <div className="header-left" style={{ gap: "1rem" }}>
          <div
            style={{
              cursor: "pointer",
              display: "flex",
              alignItems: "center",
              height: "56px",
              color: "transparent",
              marginLeft: "-8px"
            }}
            onClick={handleLogoClick}
          >
            {/* LoomX Logo with revolve animation */}
            <LoomXLogo
              size={64}
              animate={isRevolving ? 'revolve' : 'none'}
            />
          </div>
          <nav className="top-nav" style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <Link
              className={`header-btn ${pathname?.startsWith('/dashboards') ? 'header-btn-active' : ''}`}
              href="/dashboards"
            >
              <i className="fas fa-tachometer-alt" /> Dashboards
            </Link>
            <Link
              className={`header-btn ${pathname?.startsWith('/charts') ? 'header-btn-active' : ''}`}
              href="/charts"
            >
              <i className="fas fa-chart-bar" /> Charts
            </Link>
            <Link
              className={`header-btn ${pathname?.startsWith('/datasets') ? 'header-btn-active' : ''}`}
              href="/datasets"
            >
              <i className="fas fa-database" /> Datasets
            </Link>
            <div className="header-dropdown">
              <Link
                className={`header-btn header-dropdown-toggle ${pathname?.startsWith('/lab') ? 'header-btn-active' : ''}`}
                href="/lab"
              >
                <i className="fas fa-flask" />
                SQL Lab
                <i className="fas fa-caret-down lab-dropdown-caret" aria-hidden="true" />
                <span className="sr-only">Open Lab menu</span>
              </Link>
              <div className="header-dropdown-menu" aria-label="Lab navigation submenu">
                <Link className="header-dropdown-item" href="/lab">
                  <i className="fas fa-flask" style={{ marginRight: 6, width: 14 }} />
                  <span>SQL Lab</span>
                </Link>
                <Link className="header-dropdown-item" href="/lab/queries">
                  <i className="fas fa-bookmark" style={{ marginRight: 6, width: 14 }} />
                  <span>Saved queries</span>
                </Link>
                <Link className="header-dropdown-item" href="/lab/queries?view=history">
                  <i className="fas fa-history" style={{ marginRight: 6, width: 14 }} />
                  <span>Query history</span>
                </Link>
              </div>
            </div>
          </nav>
        </div>
        <div className="header-right">
          <button
            type="button"
            className="header-btn"
            title="Refresh"
            onClick={() => {
              if (typeof window === "undefined") return;
              if (isLabPage) {
                window.dispatchEvent(new CustomEvent("labRefresh"));
              } else {
                window.location.reload();
              }
            }}
          >
            <i className="fas fa-sync-alt" />
          </button>
          <div className="header-dropdown header-dropdown-right">
            <button
              type="button"
              className="header-btn"
              title="Settings"
            >
              <i className="fas fa-cog" />
              <span className="sr-only">Open Settings menu</span>
            </button>
            <div className="header-dropdown-menu" aria-label="Settings submenu">
              <button
                className="header-dropdown-item"
                onClick={() => setSettingsOpen(true)}
                style={{ width: '100%', textAlign: 'left' }}
              >
                <i className="fas fa-palette" style={{ marginRight: 6, width: 14 }} />
                <span>Themes</span>
              </button>
              <Link href="/data-sources" className="header-dropdown-item">
                <i className="fas fa-server" style={{ marginRight: 6, width: 14 }} />
                <span>Data Sources</span>
              </Link>
              {isAdmin && (
                <Link href="/settings/users" className="header-dropdown-item">
                  <i className="fas fa-users-cog" style={{ marginRight: 6, width: 14 }} />
                  <span>User Management</span>
                </Link>
              )}
            </div>
          </div>
          <div style={{ position: "relative", display: "inline-block" }}>
            <button
              type="button"
              className="header-btn"
              title="User Menu"
              style={{ padding: 0, border: "none", background: "none" }}
              onClick={() => {
                const dropdown = document.getElementById("user-menu-dropdown");
                if (dropdown) dropdown.style.display = dropdown.style.display === "block" ? "none" : "block";
              }}
            >
              <div style={{
                width: 36,
                height: 36,
                borderRadius: "50%",
                background: `linear-gradient(135deg, ${gradientColors.light} 0%, ${gradientColors.base} 50%, ${gradientColors.dark} 100%)`,
                color: "white",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                fontSize: 14,
                fontWeight: 600,
                letterSpacing: "0.5px",
                boxShadow: "0 2px 6px rgba(0, 0, 0, 0.15)"
              }}>
                {getInitials(account?.name)}
              </div>
            </button>
            <div
              id="user-menu-dropdown"
              style={{
                display: "none",
                position: "absolute",
                right: 0,
                top: 40,
                minWidth: 180,
                background: "#222",
                color: "#fff",
                borderRadius: 8,
                boxShadow: "0 2px 8px rgba(0,0,0,0.15)",
                zIndex: 1000,
                padding: "8px 0"
              }}
              onMouseLeave={e => { (e.currentTarget as HTMLDivElement).style.display = "none"; }}
            >
              <div style={{ padding: "8px 16px", borderBottom: "1px solid #444" }}>
                <div style={{ fontWeight: "bold" }}>{account?.name || "User"}</div>
                <div style={{ fontSize: 13, color: "#bbb" }}>{account?.email || account?.username || ""}</div>
                {isAuthenticated && <div style={{ fontSize: 11, color: "#888", marginTop: 2 }}>{role ?? "Viewer"}</div>}
              </div>
              <button
                style={{ width: "100%", background: "none", border: "none", color: "#fff", textAlign: "left", padding: "8px 16px", cursor: "pointer" }}
                onClick={logout}
              >
                <i className="fas fa-sign-out-alt" style={{ marginRight: 8 }} /> Sign Out
              </button>
            </div>
          </div>
        </div>
      </header>
      <main
        className={
          isLabPage
            ? "main-lab"
            : isWidePage
            ? "main-wide"
            : isHomePage
            ? "main-home"
            : undefined
        }
      >
        {children}
      </main>
      <SettingsModal isOpen={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </>
  );
}
