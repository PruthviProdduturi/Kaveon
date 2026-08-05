"use client";

import { APP_DISPLAY_NAME, APP_LOGO_URL } from "../constants/branding";
import { LensLogo } from "./LensLogo";
import { SettingsModal } from "./SettingsModal";

import Link from "next/link";
import { ReactNode, useEffect, useRef, useState } from "react";
import { useAuth } from "../auth/useAuth";
import { usePathname } from "next/navigation";
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
  const [isRevolving, setIsRevolving] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [userMenuOpen, setUserMenuOpen] = useState(false);
  const userMenuRef = useRef<HTMLDivElement>(null);

  const getInitials = (name: string | undefined) => {
    if (!name) return "U";
    const parts = name.trim().split(" ");
    if (parts.length === 1) return parts[0].charAt(0).toUpperCase();
    return (parts[0].charAt(0) + parts[parts.length - 1].charAt(0)).toUpperCase();
  };

  // Close user menu on outside click
  useEffect(() => {
    const handleClick = (e: MouseEvent) => {
      if (userMenuRef.current && !userMenuRef.current.contains(e.target as Node)) {
        setUserMenuOpen(false);
      }
    };
    if (userMenuOpen) document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [userMenuOpen]);

  const isLabPage = pathname === "/lab" || pathname?.startsWith("/ai") || false;
  const isSqlActivityPage = pathname === "/lab/queries";
  const isChartBuildPage = pathname === "/charts/new/build";
  const isChartsPage = pathname?.startsWith("/charts") || false;
  const isDashboardsPage = pathname?.startsWith("/dashboards") || false;
  const isDatasetsPage = pathname?.startsWith("/datasets") || false;
  const isAboutPage = pathname === "/about";
  const isWidePage = isSqlActivityPage || isChartBuildPage || isChartsPage || isDashboardsPage || isDatasetsPage;
  const isHomePage = pathname === "/";

  const handleLogoClick = () => {
    setIsRevolving(true);
    setTimeout(() => {
      setIsRevolving(false);
      if (pathname === "/") window.location.reload();
      else window.location.href = "/";
    }, 400);
  };

  const handleRefresh = () => {
    if (typeof window === "undefined") return;
    if (isLabPage) window.dispatchEvent(new CustomEvent("labRefresh"));
    else window.location.reload();
  };

  return (
    <>
      <header className="app-header">
        {/* ── Left: logo + nav ── */}
        <div className="header-left">
          {/* Logo */}
          <div className="header-logo-area" onClick={handleLogoClick}>
            <LensLogo size={36} animate={isRevolving ? "revolve" : "none"} />
          </div>

          {/* Separator */}
          <div className="header-divider" />

          {/* Primary nav */}
          <nav className="top-nav">
            <Link className={`header-btn${pathname?.startsWith("/dashboards") ? " header-btn-active" : ""}`} href="/dashboards">
              <i className="fas fa-tachometer-alt" /> Dashboards
            </Link>
            <Link className={`header-btn${pathname?.startsWith("/charts") ? " header-btn-active" : ""}`} href="/charts">
              <i className="fas fa-chart-bar" /> Charts
            </Link>
            <Link className={`header-btn${pathname?.startsWith("/datasets") ? " header-btn-active" : ""}`} href="/datasets">
              <i className="fas fa-database" /> Datasets
            </Link>

            {/* SQL Lab dropdown */}
            <div className="header-dropdown">
              <Link className={`header-btn${pathname?.startsWith("/lab") ? " header-btn-active" : ""}`} href="/lab">
                <i className="fas fa-flask" />
                SQL Lab
                <i className="fas fa-caret-down lab-dropdown-caret" aria-hidden="true" />
              </Link>
              <div className="header-dropdown-menu">
                <Link className="header-dropdown-item" href="/lab">
                  <i className="fas fa-flask" style={{ width: 14 }} /> SQL Lab
                </Link>
                <Link className="header-dropdown-item" href="/lab/queries">
                  <i className="fas fa-bookmark" style={{ width: 14 }} /> Saved queries
                </Link>
                <Link className="header-dropdown-item" href="/lab/queries?view=history">
                  <i className="fas fa-history" style={{ width: 14 }} /> Query history
                </Link>
              </div>
            </div>

            {/* AI */}
            <Link className={`header-btn${pathname?.startsWith("/ai") ? " header-btn-active" : ""}`} href="/ai">
              <i className="fas fa-magic" />
              AI
              <span style={{
                fontSize: 9, fontWeight: 700, padding: "1px 5px", borderRadius: 8,
                background: "linear-gradient(135deg, #7c3aed, #a855f7)",
                color: "white", letterSpacing: "0.3px", lineHeight: 1.4,
              }}>NEW</span>
            </Link>
          </nav>
        </div>

        {/* ── Right: actions + user ── */}
        <div className="header-right">
          {/* About / Help */}
          <Link href="/about" className="header-icon-btn" title="About Lens — features, API reference, docs">
            <i className="fas fa-circle-question" />
          </Link>

          {/* Refresh */}
          <button type="button" className="header-icon-btn" title="Refresh" onClick={handleRefresh}>
            <i className="fas fa-sync-alt" />
          </button>

          {/* Settings dropdown */}
          <div className="header-dropdown header-dropdown-right">
            <button type="button" className="header-icon-btn" title="Settings">
              <i className="fas fa-cog" />
            </button>
            <div className="header-dropdown-menu">
              <button className="header-dropdown-item" onClick={() => setSettingsOpen(true)}>
                <i className="fas fa-palette" style={{ width: 14, color: "#8b5cf6" }} /> Themes
              </button>
              {isAdmin && (
                <Link href="/settings/system" className="header-dropdown-item">
                  <i className="fas fa-sliders" style={{ width: 14, color: "#dc2626" }} /> System
                </Link>
              )}
              <div className="header-dropdown-divider" />
              <Link href="/data-sources" className="header-dropdown-item">
                <i className="fas fa-server" style={{ width: 14, color: "#0ea5e9" }} /> Data Sources
              </Link>
              <Link href="/settings/ai" className="header-dropdown-item">
                <i className="fas fa-magic" style={{ width: 14, color: "#7c3aed" }} /> AI Providers
              </Link>
            </div>
          </div>

          {/* User avatar + dropdown */}
          <div style={{ position: "relative" }} ref={userMenuRef}>
            <button
              type="button"
              onClick={() => setUserMenuOpen(v => !v)}
              title={account?.name ?? "User menu"}
              style={{ background: "none", border: "none", cursor: "pointer", padding: "0 0.25rem", display: "flex", alignItems: "center" }}
            >
              <div style={{
                width: 34, height: 34, borderRadius: "50%",
                background: `linear-gradient(135deg, ${gradientColors.light} 0%, ${gradientColors.base} 50%, ${gradientColors.dark} 100%)`,
                color: "white", display: "flex", alignItems: "center", justifyContent: "center",
                fontSize: 13, fontWeight: 700, letterSpacing: "0.5px",
                boxShadow: "0 2px 6px rgba(0,0,0,0.2)",
              }}>
                {getInitials(account?.name)}
              </div>
            </button>

            {userMenuOpen && (
              <div className="user-dropdown-menu" style={{ display: "block" }}>
                <div className="user-dropdown-header">
                  <div style={{ fontWeight: 700, fontSize: 14, color: "#0f172a", marginBottom: 1 }}>
                    {account?.name || "User"}
                  </div>
                  <div style={{ fontSize: 12, color: "#64748b", marginBottom: 2 }}>
                    {account?.email || account?.username || ""}
                  </div>
                  {isAuthenticated && (
                    <span style={{
                      fontSize: 11, fontWeight: 600, padding: "1px 7px", borderRadius: 8,
                      background: `${primaryColor}15`, color: primaryColor,
                      border: `1px solid ${primaryColor}30`,
                    }}>
                      {role ?? "Viewer"}
                    </span>
                  )}
                </div>
                <button
                  className="header-dropdown-item"
                  style={{ color: "#dc2626" }}
                  onClick={() => { setUserMenuOpen(false); logout(); }}
                >
                  <i className="fas fa-sign-out-alt" style={{ width: 14 }} /> Sign Out
                </button>
              </div>
            )}
          </div>
        </div>
      </header>

      <main
        className={
          isAboutPage ? "main-about"
          : isLabPage ? "main-lab"
          : isWidePage ? "main-wide"
          : isHomePage ? "main-home"
          : undefined
        }
      >
        {children}
      </main>

      <SettingsModal isOpen={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </>
  );
}
