"use client";

import React, { ReactNode } from "react";
import { useTheme } from "../contexts/ThemeContext";
import { LensLoading } from "./LensLoading";

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
  /** Loading state — shows LensLoading spinner */
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
  /** Optional secondary link/action shown beneath the primary empty CTA */
  emptySecondaryAction?: ReactNode;
  /** Optional guided steps rendered in the empty state for a systematic onboarding */
  emptySteps?: { icon: string; title: string; desc: string }[];
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
  empty, emptyIcon, emptyTitle, emptyBody, emptyAction, emptySecondaryAction, emptySteps,
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
        {/* Right: action — hidden in empty/error states so those cards own the CTA */}
        {action && !empty && !error && <div style={{ flexShrink: 0 }}>{action}</div>}
      </div>

      {/* ── Search bar (hidden when there is nothing to search) ── */}
      {onSearch && !empty && !error && (
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
      {loading && <LensLoading message={loadingMessage} />}

      {/* ── Error ── */}
      {!loading && error && (() => {
        const metadataReady = typeof sessionStorage !== "undefined"
          ? sessionStorage.getItem("lens_setup_ok") === "1"
          : true;
        const panel: React.CSSProperties = {
          background: "white", borderRadius: 16, border: "1px solid #e5e7eb",
          boxShadow: "0 1px 4px rgba(15,23,42,0.06)",
          padding: "3.5rem 2rem", textAlign: "center",
          display: "flex", flexDirection: "column", alignItems: "center",
        };
        const iconWrap = (bg: string): React.CSSProperties => ({
          width: 68, height: 68, borderRadius: 18, marginBottom: 18,
          background: bg, display: "flex", alignItems: "center", justifyContent: "center",
        });
        const titleStyle: React.CSSProperties = { margin: 0, fontSize: "1.35rem", fontWeight: 800, color: "#0f172a", letterSpacing: "-0.4px" };
        const bodyStyle: React.CSSProperties = { margin: "8px 0 0", fontSize: "0.95rem", color: "#64748b", maxWidth: 460, lineHeight: 1.55 };
        if (!metadataReady) {
          return (
            <div style={panel}>
              <div style={iconWrap("#fef3c7")}>
                <i className="fas fa-database" style={{ color: "#d97706", fontSize: 28 }} />
              </div>
              <h2 style={titleStyle}>Connect a metadata database</h2>
              <p style={bodyStyle}>
                Lens stores your dashboards, charts and datasets in a metadata database.
                Configure one to get started — it takes about a minute.
              </p>
              <a
                href="/settings/system"
                style={{
                  marginTop: 24, display: "inline-flex", alignItems: "center", gap: 8,
                  padding: "0.6rem 1.1rem", borderRadius: 8, textDecoration: "none",
                  background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
                  color: "white", fontWeight: 600, fontSize: "0.9rem",
                  boxShadow: `0 6px 18px ${primaryColor}35`,
                }}
              >
                <i className="fas fa-sliders-h" /> Open System Settings
              </a>
            </div>
          );
        }
        return (
          <div style={panel}>
            <div style={iconWrap("#fef2f2")}>
              <i className="fas fa-exclamation-triangle" style={{ color: "#dc2626", fontSize: 28 }} />
            </div>
            <h2 style={titleStyle}>Something went wrong</h2>
            <p style={bodyStyle}>{error}</p>
          </div>
        );
      })()}

      {/* ── Empty state — centered, systematic onboarding ── */}
      {!loading && !error && empty && (
        <div style={{
          background: "white", borderRadius: 16, border: "1px solid #e5e7eb",
          boxShadow: "0 1px 4px rgba(15,23,42,0.06)",
          padding: "3.5rem 2rem", textAlign: "center",
          display: "flex", flexDirection: "column", alignItems: "center",
        }}>
          <div style={{
            width: 68, height: 68, borderRadius: 18, marginBottom: 18,
            background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
            display: "flex", alignItems: "center", justifyContent: "center",
            boxShadow: `0 10px 28px ${primaryColor}30`,
          }}>
            <i className={`fas ${emptyIcon ?? icon}`} style={{ color: "white", fontSize: 28 }} />
          </div>
          <h2 style={{ margin: 0, fontSize: "1.35rem", fontWeight: 800, color: "#0f172a", letterSpacing: "-0.4px" }}>
            {emptyTitle ?? `No ${title.toLowerCase()} yet`}
          </h2>
          {emptyBody && (
            <p style={{ margin: "8px 0 0", fontSize: "0.95rem", color: "#64748b", maxWidth: 460, lineHeight: 1.55 }}>
              {emptyBody}
            </p>
          )}

          {/* Guided steps */}
          {emptySteps && emptySteps.length > 0 && (
            <div style={{
              display: "flex", alignItems: "stretch", justifyContent: "center",
              gap: 0, flexWrap: "wrap", margin: "28px 0 4px", maxWidth: 760, width: "100%",
            }}>
              {emptySteps.map((step, i) => (
                <React.Fragment key={i}>
                  <div style={{
                    flex: "1 1 200px", maxWidth: 230, minWidth: 180,
                    background: "#f8fafc", border: "1px solid #eef2f7",
                    borderRadius: 12, padding: "18px 16px", textAlign: "left",
                    display: "flex", flexDirection: "column", gap: 8, position: "relative",
                  }}>
                    <div style={{
                      display: "flex", alignItems: "center", gap: 10,
                    }}>
                      <span style={{
                        width: 30, height: 30, borderRadius: 8, flexShrink: 0,
                        background: `${primaryColor}12`, color: primaryColor,
                        display: "flex", alignItems: "center", justifyContent: "center",
                        fontSize: 13,
                      }}>
                        <i className={`fas ${step.icon}`} />
                      </span>
                      <span style={{
                        fontSize: 11, fontWeight: 700, color: "#94a3b8",
                        textTransform: "uppercase", letterSpacing: "0.6px",
                      }}>Step {i + 1}</span>
                    </div>
                    <div style={{ fontSize: 14, fontWeight: 700, color: "#0f172a" }}>{step.title}</div>
                    <div style={{ fontSize: 12.5, color: "#64748b", lineHeight: 1.5 }}>{step.desc}</div>
                  </div>
                  {i < emptySteps.length - 1 && (
                    <div style={{
                      display: "flex", alignItems: "center", justifyContent: "center",
                      padding: "0 6px", color: "#cbd5e1", fontSize: 14,
                    }}>
                      <i className="fas fa-chevron-right" />
                    </div>
                  )}
                </React.Fragment>
              ))}
            </div>
          )}

          {emptyAction && <div style={{ marginTop: 28 }}>{emptyAction}</div>}
          {emptySecondaryAction && <div style={{ marginTop: 12 }}>{emptySecondaryAction}</div>}
        </div>
      )}

      {/* ── Content ── */}
      {!loading && !error && !empty && children}
    </div>
  );
}
