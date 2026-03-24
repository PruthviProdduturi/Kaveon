"use client";

import { ReactNode } from "react";
import { useTheme } from "../contexts/ThemeContext";
import { LoomXLoading } from "./LoomXLoading";

export interface ListPill {
  label: string;
  icon?: string;
  color?: string;
  bg?: string;
  border?: string;
}

interface ListPageShellProps {
  /** FontAwesome icon class e.g. "fa-tachometer-alt" */
  icon: string;
  title: string;
  subtitle: string;
  /** Count/status pills shown inline in the hero header */
  pills?: ListPill[];
  /** Primary action button (New …) */
  action?: ReactNode;
  /** Loading state — shows LoomXLoading spinner */
  loading?: boolean;
  loadingMessage?: string;
  /** Error message — shows error card */
  error?: string | null;
  /** Empty state — shows empty card */
  empty?: boolean;
  emptyIcon?: string;
  emptyTitle?: string;
  emptyBody?: string;
  emptyAction?: ReactNode;
  /** Search bar */
  search?: string;
  onSearch?: (q: string) => void;
  searchPlaceholder?: string;
  /** Used to show result count while searching */
  resultCount?: number;
  /** Page content — shown only when not loading/error/empty */
  children?: ReactNode;
}

export function ListPageShell({
  icon, title, subtitle, pills, action,
  loading, loadingMessage,
  error,
  empty, emptyIcon, emptyTitle, emptyBody, emptyAction,
  search, onSearch, searchPlaceholder, resultCount,
  children,
}: ListPageShellProps) {
  const { primaryColor, gradientColors } = useTheme();

  return (
    <div className="page-shell animate-fade-in">

      {/* ── Hero header card ── */}
      <div style={{
        display: "flex", alignItems: "center", justifyContent: "space-between",
        flexWrap: "wrap", gap: "1rem",
        marginBottom: "1.25rem",
        padding: "1.25rem 1.5rem",
        background: "white",
        borderRadius: 12,
        border: "1px solid #e5e7eb",
        boxShadow: "0 1px 4px rgba(15,23,42,0.06)",
      }}>
        {/* Left: icon + title + pills */}
        <div style={{ display: "flex", alignItems: "center", gap: "1.25rem", flexWrap: "wrap", flex: 1, minWidth: 0 }}>
          <div style={{
            width: 48, height: 48, borderRadius: 12, flexShrink: 0,
            background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
            display: "flex", alignItems: "center", justifyContent: "center",
            boxShadow: `0 4px 14px ${primaryColor}35`,
          }}>
            <i className={`fas ${icon}`} style={{ color: "white", fontSize: 20 }} />
          </div>
          <div style={{ minWidth: 0 }}>
            <h1 style={{ margin: 0, fontSize: "1.15rem", fontWeight: 700, color: "#0f172a" }}>{title}</h1>
            <p style={{ margin: "3px 0 0", fontSize: "0.85rem", color: "#64748b", lineHeight: 1.4 }}>{subtitle}</p>
          </div>
          {pills && pills.length > 0 && (
            <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
              {pills.map((pill, i) => (
                <span key={i} style={{
                  display: "inline-flex", alignItems: "center", gap: "0.35rem",
                  padding: "0.3rem 0.75rem", borderRadius: 20,
                  background: pill.bg ?? `${primaryColor}12`,
                  border: `1px solid ${pill.border ?? `${primaryColor}30`}`,
                  fontSize: 12, fontWeight: 600,
                  color: pill.color ?? primaryColor,
                  whiteSpace: "nowrap",
                }}>
                  {pill.icon && <i className={`fas ${pill.icon}`} style={{ fontSize: 10 }} />}
                  {pill.label}
                </span>
              ))}
            </div>
          )}
        </div>
        {/* Right: action */}
        {action && <div style={{ flexShrink: 0 }}>{action}</div>}
      </div>

      {/* ── Search bar ── */}
      {onSearch && (
        <div style={{ position: "relative", marginBottom: "1rem" }}>
          <i className="fas fa-search" style={{
            position: "absolute", left: "0.85rem", top: "50%", transform: "translateY(-50%)",
            color: "#94a3b8", fontSize: 13, pointerEvents: "none",
          }} />
          <input
            type="text"
            value={search ?? ""}
            onChange={e => onSearch(e.target.value)}
            placeholder={searchPlaceholder ?? `Search ${title.toLowerCase()}…`}
            style={{
              width: "100%", boxSizing: "border-box",
              padding: "0.625rem 2.5rem 0.625rem 2.25rem",
              borderRadius: 8, border: "1px solid #e2e8f0",
              fontSize: "0.875rem", background: "white",
              color: "#0f172a", outline: "none",
              boxShadow: "0 1px 3px rgba(15,23,42,0.05)",
              transition: "border-color 0.15s, box-shadow 0.15s",
            }}
            onFocus={e => {
              e.currentTarget.style.borderColor = primaryColor;
              e.currentTarget.style.boxShadow = `0 0 0 3px ${primaryColor}18`;
            }}
            onBlur={e => {
              e.currentTarget.style.borderColor = "#e2e8f0";
              e.currentTarget.style.boxShadow = "0 1px 3px rgba(15,23,42,0.05)";
            }}
          />
          {search && (
            <>
              {resultCount !== undefined && (
                <span style={{
                  position: "absolute", right: "2.25rem", top: "50%", transform: "translateY(-50%)",
                  fontSize: 11, color: "#94a3b8", pointerEvents: "none",
                }}>
                  {resultCount} result{resultCount !== 1 ? "s" : ""}
                </span>
              )}
              <button
                type="button"
                onClick={() => onSearch("")}
                style={{
                  position: "absolute", right: "0.65rem", top: "50%", transform: "translateY(-50%)",
                  background: "none", border: "none", cursor: "pointer",
                  color: "#94a3b8", padding: "4px 6px", borderRadius: 4,
                  fontSize: 12, lineHeight: 1,
                }}
                aria-label="Clear search"
              >
                <i className="fas fa-times" />
              </button>
            </>
          )}
        </div>
      )}

      {/* ── Loading ── */}
      {loading && <LoomXLoading message={loadingMessage} />}

      {/* ── Error ── */}
      {!loading && error && (() => {
        const metadataReady = typeof sessionStorage !== "undefined"
          ? sessionStorage.getItem("loomx_setup_ok") === "1"
          : true;
        if (!metadataReady) {
          return (
            <div className="card page-empty-card">
              <div style={{
                width: 48, height: 48, borderRadius: 12, margin: "0 auto 12px",
                background: "#fef3c7", display: "flex", alignItems: "center", justifyContent: "center",
              }}>
                <i className="fas fa-database" style={{ color: "#d97706", fontSize: 20 }} />
              </div>
              <p className="page-empty-title">Metadata database not configured</p>
              <p className="page-empty-body">
                LoomX needs a metadata database to store dashboards, charts, and datasets.{" "}
                <a href="/settings/system" style={{ color: "#2563eb", textDecoration: "underline" }}>
                  Go to System Settings
                </a>{" "}
                to configure it.
              </p>
            </div>
          );
        }
        return (
          <div className="card page-empty-card">
            <div style={{
              width: 48, height: 48, borderRadius: 12, margin: "0 auto 12px",
              background: "#fef2f2", display: "flex", alignItems: "center", justifyContent: "center",
            }}>
              <i className="fas fa-exclamation-triangle" style={{ color: "#dc2626", fontSize: 20 }} />
            </div>
            <p className="page-empty-title">Something went wrong</p>
            <p className="page-empty-body">{error}</p>
          </div>
        );
      })()}

      {/* ── Empty state ── */}
      {!loading && !error && empty && (
        <div className="card page-empty-card">
          <div style={{
            width: 56, height: 56, borderRadius: 14, margin: "0 auto 14px",
            background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
            display: "flex", alignItems: "center", justifyContent: "center",
          }}>
            <i className={`fas ${emptyIcon ?? icon}`} style={{ color: "white", fontSize: 22 }} />
          </div>
          <p className="page-empty-title">{emptyTitle ?? `No ${title.toLowerCase()} yet`}</p>
          {emptyBody && <p className="page-empty-body">{emptyBody}</p>}
          {emptyAction && <div style={{ marginTop: 14 }}>{emptyAction}</div>}
        </div>
      )}

      {/* ── Content ── */}
      {!loading && !error && !empty && children}
    </div>
  );
}
