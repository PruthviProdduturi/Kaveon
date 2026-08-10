/**
 * Dashboard Builder Component
 *
 * Main builder interface for creating and editing dashboards.
 * Integrates with DashboardContext for state management.
 */

import React, { useCallback, useEffect, useState, useRef } from "react";
import { useRouter } from "next/navigation";
import { useDashboard } from "./DashboardContext";
import { BuilderContext } from "./BuilderContext";
import DashboardCanvas from "./DashboardCanvas";
import DashboardSidebar from "./DashboardSidebar";
import DashboardProperties from "./DashboardProperties";
import DashboardFilterBarEnhanced from "./DashboardFilterBarEnhanced";
import { msalFetch } from "../../utils/msalFetch";
import { API_BASE } from "../../config";
import { DASHBOARD_THEMES } from "../../types/dashboard";
import { useAuth } from "../../auth/useAuth";
import type { ComponentType } from "../../types/dashboard";

interface Chart {
  id: number;
  name: string;
  chart_type?: string;
  dataset_name?: string | null;
  updated_at?: string | null;
  created_by?: string | null;
  owner?: string | null;
}

interface DashboardBuilderProps {
  dashboardId?: string;
}

const DashboardBuilder: React.FC<DashboardBuilderProps> = ({ dashboardId }) => {
  const router = useRouter();
  const { account } = useAuth();
  const {
    name,
    description,
    setName,
    setDescription,
    setIsEditMode,
    addLayoutItem,
    addItemToContainer,
    hasUnsavedChanges,
    isSaving,
    saveError,
    saveDashboard,
    saveDashboardAs,
    theme,
    setTheme,
  } = useDashboard();

  const [charts, setCharts] = useState<Chart[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);

  // react-grid-layout's useContainerWidth only re-measures on *window* resize,
  // not when the canvas column grows because the customize panel collapsed. Nudge
  // it so the grid reflows to full width instead of staying at the old half width.
  useEffect(() => {
    const fire = () => window.dispatchEvent(new Event("resize"));
    const timers = [0, 120, 300].map((ms) => setTimeout(fire, ms));
    return () => timers.forEach(clearTimeout);
  }, [sidebarCollapsed]);

  const [showSaveModal, setShowSaveModal] = useState(false);
  const [showSaveAsModal, setShowSaveAsModal] = useState(false);
  const [saveAsName, setSaveAsName] = useState("");
  const [savingAs, setSavingAs] = useState(false);
  const [tempName, setTempName] = useState("");
  const [tempDescription, setTempDescription] = useState("");
  const [isEditingName, setIsEditingName] = useState(false);
  const nameInputRef = useRef<HTMLInputElement>(null);
  const [sidebarTab, setSidebarTab] = useState<"items" | "layout" | "filters">("items");
  const [isFavorite, setIsFavorite] = useState(false);
  const [isAnimating, setIsAnimating] = useState(false);
  const [chartScope, setChartScope] = useState<"mine" | "all">("all");
  const [isPublishing, setIsPublishing] = useState(false);
  const [isPublished, setIsPublished] = useState(false);
  const [showChartPicker, setShowChartPicker] = useState(false);
  const [chartSearch, setChartSearch] = useState("");
  // null = add to root canvas; string = add to this container id
  const [chartPickerTarget, setChartPickerTarget] = useState<string | null>(null);

  // Enable edit mode
  useEffect(() => {
    setIsEditMode(true);
  }, [setIsEditMode]);

  // Focus name input when editing starts
  useEffect(() => {
    if (isEditingName && nameInputRef.current) {
      nameInputRef.current.focus();
      nameInputRef.current.select();
    }
  }, [isEditingName]);

  // Load favorite and published status when editing existing dashboard
  useEffect(() => {
    if (!dashboardId) return;

    const loadDashboardStatus = async () => {
      try {
        const response = await msalFetch(`${API_BASE}/api/v1/dashboards/${dashboardId}`);
        if (response.ok) {
          const dashboard = await response.json();
          setIsFavorite(dashboard.is_favorite || false);
          setIsPublished(dashboard.is_published || false);
        }
      } catch (error) {
        console.error('Error loading dashboard status:', error);
      }
    };

    loadDashboardStatus();
  }, [dashboardId]);

  // Fetch available charts
  useEffect(() => {
    setLoading(true);
    msalFetch(`${API_BASE}/api/v1/charts/summary`)
      .then((res) => {
        if (!res.ok) throw new Error(`Failed to fetch charts: ${res.status}`);
        return res.json();
      })
      .then((data) => {
        setCharts(Array.isArray(data.recent) ? data.recent : []);
        setError(null);
      })
      .catch((err) => {
        setError((err as Error).message || "Failed to load charts");
        console.error("Error fetching charts:", err);
      })
      .finally(() => setLoading(false));
  }, []);

  // Filter charts based on scope
  const userEmail = account?.email || account?.username || null;
  const myCharts = userEmail
    ? charts.filter((c) => (c as any).created_by === userEmail || (c as any).owner === userEmail)
    : charts;
  const chartsForScope = chartScope === "mine" ? myCharts : charts;

  /**
   * Open chart picker targeting a specific container (or root canvas when null)
   */
  const openChartPickerForContainer = useCallback((containerId: string | null) => {
    setChartPickerTarget(containerId);
    setChartSearch("");
    setShowChartPicker(true);
  }, []);

  /**
   * Add a chart to the dashboard – either into a container or at root level
   */
  const handleAddChart = (chart: Chart) => {
    if (chartPickerTarget) {
      addItemToContainer(chartPickerTarget, "chart", { chartId: chart.id });
    } else {
      addLayoutItem("chart", { chartId: chart.id });
    }
    setChartPickerTarget(null);
    setShowChartPicker(false);
  };

  /**
   * Add other component types
   */
  const handleAddComponent = (type: ComponentType) => {
    switch (type) {
      case "row":
        addLayoutItem("row", {});
        break;
      case "column":
        addLayoutItem("column", {});
        break;
      case "text":
        addLayoutItem("text", {
          textConfig: {
            content: "Enter your text here...",
            alignment: "left",
          },
        });
        break;
      case "header":
        addLayoutItem("header", {
          headerConfig: {
            content: "Section Header",
            size: "large",
            alignment: "left",
          },
        });
        break;
      case "divider":
        addLayoutItem("divider", {
          dividerConfig: {
            orientation: "horizontal",
            thickness: 1,
            color: "#cbd5e1",
            style: "solid",
          },
        });
        break;
      case "tabs":
        addLayoutItem("tabs", {
          tabs: [
            { id: "tab-1", name: "Tab 1", layout: [] },
          ],
        });
        break;
    }
  };

  /**
   * Open save modal
   */
  const handleSave = () => {
    setTempName(name);
    setTempDescription(description);
    setValidationError(null);
    setShowSaveModal(true);
  };

  /**
   * Confirm and save dashboard
   */
  const handleConfirmSave = async () => {
    // Validate required fields
    if (!tempName.trim()) {
      setValidationError("Dashboard name is required");
      return;
    }

    // Update context for the UI…
    setName(tempName);
    setDescription(tempDescription);
    setValidationError(null);
    setShowSaveModal(false);

    // …but pass the just-typed values straight into the save so it never races
    // the setState above (that race silently dropped description edits).
    await saveDashboard({ name: tempName, description: tempDescription });
    if (!saveError) {
      if (dashboardId) {
        router.push(`/dashboards/${dashboardId}/view`);
      } else {
        router.push("/dashboards");
      }
    }
  };

  /**
   * Cancel save modal
   */
  const handleCancelSave = () => {
    setShowSaveModal(false);
    setValidationError(null);
  };

  /**
   * Open "Save As" (duplicate) modal, pre-filled with a "Copy" name.
   */
  const handleOpenSaveAs = () => {
    setSaveAsName(name ? `${name} (Copy)` : "Untitled dashboard");
    setValidationError(null);
    setShowSaveAsModal(true);
  };

  /**
   * Confirm "Save As" — creates a new dashboard and navigates to its editor.
   */
  const handleConfirmSaveAs = async () => {
    if (!saveAsName.trim()) {
      setValidationError("Name is required");
      return;
    }
    setSavingAs(true);
    try {
      const newId = await saveDashboardAs(saveAsName.trim());
      setShowSaveAsModal(false);
      if (newId) {
        router.push(`/dashboards/${newId}/edit`);
      }
    } finally {
      setSavingAs(false);
    }
  };

  /**
   * Handle favorite toggle
   */
  const handleFavoriteClick = async () => {
    if (!dashboardId) return;

    setIsAnimating(true);
    const newFavoriteStatus = !isFavorite;
    setIsFavorite(newFavoriteStatus);

    try {
      const response = await msalFetch(`${API_BASE}/api/v1/dashboards/${dashboardId}/favorite`, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ is_favorite: newFavoriteStatus }),
      });

      if (!response.ok) {
        // Revert on failure
        setIsFavorite(!newFavoriteStatus);
        console.error('Failed to update favorite status');
      }
    } catch (error) {
      // Revert on error
      setIsFavorite(!newFavoriteStatus);
      console.error('Error updating favorite status:', error);
    } finally {
      setTimeout(() => setIsAnimating(false), 300);
    }
  };

  /**
   * Handle publish dashboard
   */
  const handlePublish = async () => {
    if (!dashboardId) return;

    setIsPublishing(true);
    try {
      const response = await msalFetch(`${API_BASE}/api/v1/dashboards/${dashboardId}/publish`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ published: true }),
      });

      if (response.ok) {
        setIsPublished(true);
        // Navigate to view page after publishing
        router.push(`/dashboards/${dashboardId}/view`);
      } else {
        console.error('Failed to publish dashboard');
      }
    } catch (error) {
      console.error('Error publishing dashboard:', error);
    } finally {
      setIsPublishing(false);
    }
  };

  const chartTypeInfo = (type?: string): { icon: string; color: string; bg: string; label: string } => {
    if (!type) return { icon: "fas fa-chart-bar", color: "#6366f1", bg: "#eef2ff", label: "Chart" };
    const t = type.toLowerCase();
    const label = type.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
    if (t.includes("line"))    return { icon: "fas fa-chart-line",     color: "#2563eb", bg: "#eff6ff", label };
    if (t.includes("area"))    return { icon: "fas fa-chart-area",     color: "#0891b2", bg: "#ecfeff", label };
    if (t.includes("bar"))     return { icon: "fas fa-chart-bar",      color: "#16a34a", bg: "#f0fdf4", label };
    if (t.includes("pie") || t.includes("donut"))
                                return { icon: "fas fa-chart-pie",      color: "#ea580c", bg: "#fff7ed", label };
    if (t.includes("table") || t.includes("pivot"))
                                return { icon: "fas fa-table",          color: "#7c3aed", bg: "#f5f3ff", label };
    if (t.includes("scatter")) return { icon: "fas fa-braille",        color: "#db2777", bg: "#fdf2f8", label };
    if (t.includes("funnel"))  return { icon: "fas fa-filter",         color: "#d97706", bg: "#fffbeb", label };
    if (t.includes("metric") || t.includes("kpi") || t.includes("number") || t.includes("big"))
                                return { icon: "fas fa-tachometer-alt", color: "#0f766e", bg: "#f0fdfa", label };
    return { icon: "fas fa-chart-bar", color: "#6366f1", bg: "#eef2ff", label };
  };

  const formatRelativeDate = (val?: string | null) => {
    if (!val) return null;
    const d = new Date(val);
    if (isNaN(d.getTime())) return val;
    const diff = Date.now() - d.getTime();
    const days = Math.floor(diff / 86400000);
    if (days === 0) return "Today";
    if (days === 1) return "Yesterday";
    if (days < 7) return `${days}d ago`;
    if (days < 30) return `${Math.floor(days / 7)}w ago`;
    if (days < 365) return `${Math.floor(days / 30)}mo ago`;
    return `${Math.floor(days / 365)}y ago`;
  };

  const chartQuery = chartSearch.toLowerCase().trim();
  const filteredCharts = chartsForScope.filter((c) =>
    !chartQuery ||
    c.name.toLowerCase().includes(chartQuery) ||
    (c.chart_type || "").toLowerCase().includes(chartQuery) ||
    (c.dataset_name || "").toLowerCase().includes(chartQuery)
  );

  const builderContextValue = React.useMemo(
    () => ({ openChartPickerForContainer }),
    [openChartPickerForContainer]
  );

  return (
    <BuilderContext.Provider value={builderContextValue}>
    <div className="page-shell page-shell-wide">
      {/* Top Header Bar */}
      <header className="page-header page-header-with-actions">
        <div className="page-header-main" style={{ display: "flex", flexDirection: 'row', alignItems: "center", gap: 8, flexWrap: 'nowrap' }}>
          {!isEditingName ? (
            <h1
              onClick={() => setIsEditingName(true)}
              style={{
                fontSize: 18,
                fontWeight: 600,
                color: name ? "var(--text-primary)" : "var(--text-muted)",
                cursor: "pointer",
                padding: "4px 8px",
                margin: 0,
                borderRadius: 6,
                border: "2px solid transparent",
                transition: "background 0.15s ease, color 0.15s ease",
                lineHeight: 1.4,
                whiteSpace: 'nowrap',
              }}
              onMouseOver={(e) => {
                e.currentTarget.style.background = "var(--bg-hover)";
              }}
              onMouseOut={(e) => {
                e.currentTarget.style.background = "transparent";
              }}
              title="Click to edit"
            >
              {name || "[New Dashboard]"}
            </h1>
          ) : (
            <input
              ref={nameInputRef}
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onBlur={() => setIsEditingName(false)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  setIsEditingName(false);
                }
              }}
              placeholder="[New Dashboard]"
              style={{
                fontSize: 18,
                fontWeight: 600,
                color: "var(--text-primary)",
                padding: "4px 8px",
                margin: 0,
                border: "2px solid #2563eb",
                borderRadius: 6,
                outline: "none",
                background: "var(--bg-surface)",
                minWidth: 300,
                lineHeight: 1.4,
                boxSizing: "border-box",
              }}
            />
          )}
          {hasUnsavedChanges && (
            <span
              style={{
                fontSize: 11,
                fontWeight: 600,
                color: "#f59e0b",
                background: "#fef3c7",
                padding: "4px 10px",
                borderRadius: 4,
                textTransform: "uppercase",
                letterSpacing: "0.5px",
                whiteSpace: 'nowrap',
                flexShrink: 0,
              }}
            >
              Unsaved
            </span>
          )}
        </div>

        {/* Right: Actions */}
        <div className="page-header-actions">
          {dashboardId && (
            <button
              type="button"
              className="chart-builder-fav-btn chart-builder-fav-btn-inline"
              aria-label={isFavorite ? "Unfavorite dashboard" : "Favorite dashboard"}
              onClick={handleFavoriteClick}
              style={{
                background: 'transparent',
                border: 'none',
                padding: '8px 12px',
                cursor: 'pointer',
                transition: 'all 0.2s ease',
                borderRadius: '6px',
                transform: isAnimating ? 'scale(0.9)' : 'scale(1)',
              }}
              onMouseOver={(e) => {
                if (!isAnimating) {
                  e.currentTarget.style.background = isFavorite ? '#FEF3C7' : '#F3F4F6';
                  e.currentTarget.style.transform = 'scale(1.05)';
                }
              }}
              onMouseOut={(e) => {
                if (!isAnimating) {
                  e.currentTarget.style.background = 'transparent';
                  e.currentTarget.style.transform = 'scale(1)';
                }
              }}
            >
              <i
                className={isFavorite ? "fas fa-star" : "far fa-star"}
                aria-hidden="true"
                style={{
                  fontSize: '20px',
                  color: isFavorite ? '#F59E0B' : '#9CA3AF',
                  transition: 'all 0.2s ease',
                  filter: isFavorite ? 'drop-shadow(0 2px 4px rgba(245, 158, 11, 0.3))' : 'none',
                  transform: isAnimating && isFavorite ? 'scale(1.3)' : 'scale(1)',
                }}
              />
            </button>
          )}
          {/* Publish Button - Only show for existing dashboards */}
          {dashboardId && !isPublished && (
            <button
              onClick={handlePublish}
              disabled={isPublishing}
              style={{
                padding: "8px 16px",
                fontWeight: 600,
                fontSize: 13,
                background: "#10b981",
                color: "#ffffff",
                border: "none",
                borderRadius: 6,
                cursor: isPublishing ? "not-allowed" : "pointer",
                transition: "all 0.2s ease",
                display: "flex",
                alignItems: "center",
                gap: 6,
              }}
              onMouseOver={(e) => {
                if (!isPublishing) {
                  e.currentTarget.style.background = "#059669";
                }
              }}
              onMouseOut={(e) => {
                if (!isPublishing) {
                  e.currentTarget.style.background = "#10b981";
                }
              }}
            >
              {isPublishing ? (
                <>
                  <i className="fas fa-spinner fa-spin" />
                  Publishing...
                </>
              ) : (
                <>
                  <i className="fas fa-globe" />
                  Publish
                </>
              )}
            </button>
          )}
          {/* Colour theme picker — recolours every chart on the board */}
          <div style={{ display: "flex", alignItems: "center", gap: 6, marginRight: 4 }} title="Colour theme — recolours all charts">
            <i className="fas fa-palette" style={{ color: "var(--text-muted)", fontSize: 13 }} />
            <select
              value={theme || "default"}
              onChange={(e) => setTheme(e.target.value)}
              style={{ padding: "7px 8px", fontSize: 13, borderRadius: 6, border: "1px solid var(--border)", background: "var(--bg-surface)", color: "var(--text-secondary)", cursor: "pointer" }}
            >
              {Object.entries(DASHBOARD_THEMES).map(([key, t]) => (
                <option key={key} value={key}>{t.label}</option>
              ))}
            </select>
          </div>
          {dashboardId && (
            <button
              onClick={handleOpenSaveAs}
              disabled={isSaving}
              title="Save as a new dashboard (duplicate)"
              style={{
                padding: "8px 16px",
                fontWeight: 600,
                fontSize: 13,
                background: "transparent",
                color: "var(--text-secondary)",
                border: "1px solid var(--border)",
                borderRadius: 6,
                cursor: "pointer",
                display: "flex",
                alignItems: "center",
                gap: 6,
              }}
            >
              <i className="fas fa-copy" />
              Save As
            </button>
          )}
          <button
            onClick={handleSave}
            disabled={!hasUnsavedChanges || isSaving}
            style={{
              padding: "8px 20px",
              fontWeight: 600,
              fontSize: 13,
              background: hasUnsavedChanges ? "#2563eb" : "var(--border)",
              color: hasUnsavedChanges ? "#ffffff" : "var(--text-muted)",
              border: "none",
              borderRadius: 6,
              cursor: hasUnsavedChanges ? "pointer" : "not-allowed",
              transition: "all 0.2s ease",
              display: "flex",
              alignItems: "center",
              gap: 6,
            }}
            onMouseOver={(e) => {
              if (hasUnsavedChanges) {
                e.currentTarget.style.background = "#1d4ed8";
              }
            }}
            onMouseOut={(e) => {
              if (hasUnsavedChanges) {
                e.currentTarget.style.background = "#2563eb";
              }
            }}
          >
            {isSaving ? (
              <>
                <i className="fas fa-spinner fa-spin" />
                Saving...
              </>
            ) : (
              <>
                <i className="fas fa-save" />
                Save
              </>
            )}
          </button>
          {dashboardId && (
            <button
              onClick={() => router.push(`/dashboards/${dashboardId}/view`)}
              style={{
                padding: "8px 16px",
                fontWeight: 600,
                fontSize: 13,
                background: "transparent",
                color: "var(--text-secondary)",
                border: "1px solid var(--border)",
                borderRadius: 6,
                cursor: "pointer",
                display: "flex",
                alignItems: "center",
                gap: 6,
              }}
              onMouseOver={(e) => { e.currentTarget.style.background = "var(--bg-hover)"; }}
              onMouseOut={(e) => { e.currentTarget.style.background = "transparent"; }}
            >
              <i className="fas fa-eye" />
              View
            </button>
          )}
          {saveError && (
            <span style={{ color: "#ef4444", fontSize: 12 }}>
              <i className="fas fa-exclamation-circle" style={{ marginRight: 4 }} />
              {saveError}
            </span>
          )}
        </div>
      </header>

      {/* Builder Interface */}
      <div
        style={{
          flex: 1,
          display: 'grid',
          gridTemplateColumns: sidebarCollapsed
            ? "auto minmax(0, 1fr)"
            : "minmax(0, 360px) minmax(0, 1fr)",
          overflow: 'hidden',
        }}
      >
        {/* Left Sidebar - Dashboard Settings */}
        <div
          style={{
            background: sidebarCollapsed ? 'transparent' : 'var(--bg-surface)',
            borderRight: sidebarCollapsed ? 'none' : '1px solid var(--border)',
            display: 'flex',
            flexDirection: 'column',
            overflow: 'hidden',
            width: sidebarCollapsed ? 'auto' : undefined,
          }}
        >
          {sidebarCollapsed ? (
            <div style={{
              background: 'var(--bg-elevated)',
              borderRight: '1px solid var(--border)',
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              height: '100%',
            }}>
              <button
                onClick={() => setSidebarCollapsed(false)}
                style={{
                  background: 'transparent',
                  border: 'none',
                  padding: '12px 8px',
                  cursor: 'pointer',
                  color: 'var(--text-secondary)',
                  fontSize: 18,
                  fontWeight: 600,
                }}
                aria-label="Expand dashboard settings"
              >
                ❯
              </button>
              <div style={{
                writingMode: 'vertical-rl',
                transform: 'rotate(180deg)',
                fontSize: 12,
                fontWeight: 600,
                color: 'var(--text-muted)',
                letterSpacing: '0.5px',
                padding: '16px 0',
                textTransform: 'uppercase',
              }}>
                Dashboard Settings
              </div>
            </div>
          ) : (
            <>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '12px 16px', borderBottom: '1px solid var(--border)' }}>
                <span style={{ fontSize: 14, fontWeight: 600, color: 'var(--text-primary)' }}>Dashboard Settings</span>
                <button
                  onClick={() => setSidebarCollapsed(true)}
                  style={{
                    background: 'transparent',
                    border: 'none',
                    padding: '4px 8px',
                    cursor: 'pointer',
                    color: 'var(--text-muted)',
                    fontSize: 14,
                  }}
                  aria-label="Collapse dashboard settings"
                >
                  ❮
                </button>
              </div>
              <div style={{ flex: 1, overflow: 'auto', padding: '16px' }}>
              {/* Sidebar Tabs */}
              <div style={{ borderBottom: "1px solid var(--border)", marginBottom: 20 }}>
                <div style={{ display: "flex", gap: 0 }}>
                  <button
                    onClick={() => setSidebarTab("items")}
                    style={{
                      flex: 1,
                      padding: "10px 16px",
                      fontSize: 13,
                      fontWeight: 600,
                      background: "transparent",
                      color: sidebarTab === "items" ? "#2563eb" : "var(--text-muted)",
                      border: "none",
                      borderBottom: sidebarTab === "items" ? "2px solid #2563eb" : "2px solid transparent",
                      cursor: "pointer",
                      transition: "all 0.15s ease",
                    }}
                  >
                    Items
                  </button>
                  <button
                    onClick={() => setSidebarTab("layout")}
                    style={{
                      flex: 1,
                      padding: "10px 16px",
                      fontSize: 13,
                      fontWeight: 600,
                      background: "transparent",
                      color: sidebarTab === "layout" ? "#2563eb" : "var(--text-muted)",
                      border: "none",
                      borderBottom: sidebarTab === "layout" ? "2px solid #2563eb" : "2px solid transparent",
                      cursor: "pointer",
                      transition: "all 0.15s ease",
                    }}
                  >
                    Layout
                  </button>
                  <button
                    onClick={() => setSidebarTab("filters")}
                    style={{
                      flex: 1,
                      padding: "10px 16px",
                      fontSize: 13,
                      fontWeight: 600,
                      background: "transparent",
                      color: sidebarTab === "filters" ? "#2563eb" : "var(--text-muted)",
                      border: "none",
                      borderBottom: sidebarTab === "filters" ? "2px solid #2563eb" : "2px solid transparent",
                      cursor: "pointer",
                      transition: "all 0.15s ease",
                    }}
                  >
                    Filters
                  </button>
                </div>
              </div>

              {/* Items Tab Content */}
              {sidebarTab === "items" && (
                <div>
                  {/* Search */}
                  <div style={{ position: "relative", marginBottom: 8 }}>
                    <i className="fas fa-search" style={{
                      position: "absolute", left: 10, top: "50%", transform: "translateY(-50%)",
                      color: "var(--text-muted)", fontSize: 12, pointerEvents: "none",
                    }} />
                    <input
                      type="text"
                      placeholder="Search charts…"
                      value={chartSearch}
                      onChange={(e) => setChartSearch(e.target.value)}
                      style={{
                        width: "100%", padding: "7px 10px 7px 30px",
                        border: "1px solid var(--border)", borderRadius: 6,
                        fontSize: 12, outline: "none", boxSizing: "border-box",
                        background: "var(--bg-surface)", color: "var(--text-primary)",
                      }}
                      onFocus={(e) => { e.target.style.borderColor = "#2563eb"; }}
                      onBlur={(e) => { e.target.style.borderColor = "var(--border)"; }}
                    />
                  </div>
                  {/* Toolbar: count + mine toggle */}
                  <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 8 }}>
                    <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
                      {filteredCharts.length} chart{filteredCharts.length !== 1 ? "s" : ""}
                    </span>
                    <label style={{ display: "flex", alignItems: "center", gap: 6, cursor: "pointer", userSelect: "none" }}>
                      <span
                        onClick={() => setChartScope(chartScope === "mine" ? "all" : "mine")}
                        style={{
                          display: "inline-block", width: 30, height: 16,
                          background: chartScope === "mine" ? "#2563eb" : "var(--border)",
                          borderRadius: 8, position: "relative",
                          transition: "background 0.2s ease", cursor: "pointer", flexShrink: 0,
                        }}
                      >
                        <span style={{
                          position: "absolute", top: 2,
                          left: chartScope === "mine" ? 14 : 2,
                          width: 12, height: 12, background: "var(--bg-surface)",
                          borderRadius: "50%", transition: "left 0.2s ease",
                          boxShadow: "0 1px 2px rgba(0,0,0,0.2)",
                        }} />
                      </span>
                      <span
                        onClick={() => setChartScope(chartScope === "mine" ? "all" : "mine")}
                        style={{ fontSize: 11, color: "var(--text-secondary)", fontWeight: 500 }}
                      >
                        Mine only
                      </span>
                    </label>
                  </div>
                  {/* Inline chart list */}
                  <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                    {loading ? (
                      <div style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: "24px 0", color: "var(--text-muted)", gap: 8 }}>
                        <i className="fas fa-spinner fa-spin" style={{ fontSize: 14 }} />
                        <span style={{ fontSize: 12 }}>Loading…</span>
                      </div>
                    ) : error ? (
                      <div style={{ padding: "8px 10px", background: "#fef2f2", border: "1px solid #fecaca", borderRadius: 6, color: "#dc2626", fontSize: 11 }}>
                        {error}
                      </div>
                    ) : filteredCharts.length === 0 ? (
                      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", padding: "24px 0", color: "var(--text-muted)", gap: 6 }}>
                        <i className="fas fa-chart-bar" style={{ fontSize: 24, opacity: 0.4 }} />
                        <span style={{ fontSize: 12 }}>No charts found</span>
                      </div>
                    ) : (
                      filteredCharts.map((chart) => {
                        const info = chartTypeInfo(chart.chart_type);
                        const modified = formatRelativeDate(chart.updated_at);
                        return (
                          <button
                            key={chart.id}
                            onClick={() => handleAddChart(chart)}
                            title={`Add "${chart.name}" to dashboard`}
                            style={{
                              width: "100%", padding: "8px 10px",
                              background: "var(--bg-surface)", border: "1px solid var(--border)",
                              borderRadius: 6, textAlign: "left", cursor: "pointer",
                              display: "flex", alignItems: "center", gap: 8,
                              transition: "border-color 0.12s, background 0.12s",
                              flexShrink: 0,
                            }}
                            onMouseOver={(e) => {
                              e.currentTarget.style.borderColor = info.color;
                              e.currentTarget.style.background = info.bg;
                            }}
                            onMouseOut={(e) => {
                              e.currentTarget.style.borderColor = "var(--border)";
                              e.currentTarget.style.background = "var(--bg-surface)";
                            }}
                          >
                            <div style={{
                              width: 28, height: 28, borderRadius: 6, flexShrink: 0,
                              background: info.bg, display: "flex", alignItems: "center", justifyContent: "center",
                              border: `1px solid ${info.color}33`,
                            }}>
                              <i className={info.icon} style={{ fontSize: 13, color: info.color }} />
                            </div>
                            <div style={{ minWidth: 0, flex: 1 }}>
                              <div style={{
                                fontSize: 12, fontWeight: 600, color: "var(--text-primary)",
                                overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
                              }}>
                                {chart.name}
                              </div>
                              <div style={{ display: "flex", alignItems: "center", gap: 4, fontSize: 10, color: "var(--text-muted)", marginTop: 2 }}>
                                <span style={{
                                  background: info.bg, color: info.color,
                                  borderRadius: 4, padding: "0px 4px", fontWeight: 500,
                                  border: `1px solid ${info.color}33`,
                                }}>
                                  {info.label}
                                </span>
                                {chart.dataset_name && (
                                  <>
                                    <span style={{ color: "var(--border)" }}>·</span>
                                    <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                                      {chart.dataset_name}
                                    </span>
                                  </>
                                )}
                              </div>
                            </div>
                            {modified && (
                              <div style={{ fontSize: 10, color: "var(--text-muted)", flexShrink: 0, whiteSpace: "nowrap" }}>
                                {modified}
                              </div>
                            )}
                          </button>
                        );
                      })
                    )}
                  </div>
                </div>
              )}

              {/* Layout Tab Content */}
              {sidebarTab === "layout" && (
                <div>
                  <h3 style={{ fontSize: 12, fontWeight: 600, color: "var(--text-muted)", marginBottom: 12, textTransform: "uppercase", letterSpacing: "0.5px" }}>
                    Layout Components
                  </h3>
                  <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                    <button
                      onClick={() => handleAddComponent("row")}
                      style={{
                        padding: "10px 12px",
                        background: "var(--bg-elevated)",
                        border: "1px solid var(--border)",
                        borderRadius: 6,
                        textAlign: "left",
                        cursor: "pointer",
                        fontSize: 13,
                        fontWeight: 500,
                        color: "var(--text-secondary)",
                        transition: "all 0.15s ease",
                      }}
                      onMouseOver={(e) => {
                        e.currentTarget.style.background = "var(--bg-hover)";
                        e.currentTarget.style.transform = "translateX(2px)";
                      }}
                      onMouseOut={(e) => {
                        e.currentTarget.style.background = "var(--bg-elevated)";
                        e.currentTarget.style.transform = "translateX(0)";
                      }}
                    >
                      <i className="fas fa-grip-lines" style={{ marginRight: 8, width: 16, textAlign: "center" }} />
                      Row
                    </button>
                    <button
                      onClick={() => handleAddComponent("column")}
                      style={{
                        padding: "10px 12px",
                        background: "var(--bg-elevated)",
                        border: "1px solid var(--border)",
                        borderRadius: 6,
                        textAlign: "left",
                        cursor: "pointer",
                        fontSize: 13,
                        fontWeight: 500,
                        color: "var(--text-secondary)",
                        transition: "all 0.15s ease",
                      }}
                      onMouseOver={(e) => {
                        e.currentTarget.style.background = "var(--bg-hover)";
                        e.currentTarget.style.transform = "translateX(2px)";
                      }}
                      onMouseOut={(e) => {
                        e.currentTarget.style.background = "var(--bg-elevated)";
                        e.currentTarget.style.transform = "translateX(0)";
                      }}
                    >
                      <i className="fas fa-grip-lines-vertical" style={{ marginRight: 8, width: 16, textAlign: "center" }} />
                      Column
                    </button>
                    <button
                      onClick={() => handleAddComponent("header")}
                      style={{
                        padding: "10px 12px",
                        background: "var(--bg-elevated)",
                        border: "1px solid var(--border)",
                        borderRadius: 6,
                        textAlign: "left",
                        cursor: "pointer",
                        fontSize: 13,
                        fontWeight: 500,
                        color: "var(--text-secondary)",
                        transition: "all 0.15s ease",
                      }}
                      onMouseOver={(e) => {
                        e.currentTarget.style.background = "var(--bg-hover)";
                        e.currentTarget.style.transform = "translateX(2px)";
                      }}
                      onMouseOut={(e) => {
                        e.currentTarget.style.background = "var(--bg-elevated)";
                        e.currentTarget.style.transform = "translateX(0)";
                      }}
                    >
                      <i className="fas fa-heading" style={{ marginRight: 8, width: 16, textAlign: "center" }} />
                      Header
                    </button>
                    <button
                      onClick={() => handleAddComponent("text")}
                      style={{
                        padding: "10px 12px",
                        background: "var(--bg-elevated)",
                        border: "1px solid var(--border)",
                        borderRadius: 6,
                        textAlign: "left",
                        cursor: "pointer",
                        fontSize: 13,
                        fontWeight: 500,
                        color: "var(--text-secondary)",
                        transition: "all 0.15s ease",
                      }}
                      onMouseOver={(e) => {
                        e.currentTarget.style.background = "var(--bg-hover)";
                        e.currentTarget.style.transform = "translateX(2px)";
                      }}
                      onMouseOut={(e) => {
                        e.currentTarget.style.background = "var(--bg-elevated)";
                        e.currentTarget.style.transform = "translateX(0)";
                      }}
                    >
                      <i className="fas fa-align-left" style={{ marginRight: 8, width: 16, textAlign: "center" }} />
                      Text Block
                    </button>
                    <button
                      onClick={() => handleAddComponent("divider")}
                      style={{
                        padding: "10px 12px",
                        background: "var(--bg-elevated)",
                        border: "1px solid var(--border)",
                        borderRadius: 6,
                        textAlign: "left",
                        cursor: "pointer",
                        fontSize: 13,
                        fontWeight: 500,
                        color: "var(--text-secondary)",
                        transition: "all 0.15s ease",
                      }}
                      onMouseOver={(e) => {
                        e.currentTarget.style.background = "var(--bg-hover)";
                        e.currentTarget.style.transform = "translateX(2px)";
                      }}
                      onMouseOut={(e) => {
                        e.currentTarget.style.background = "var(--bg-elevated)";
                        e.currentTarget.style.transform = "translateX(0)";
                      }}
                    >
                      <i className="fas fa-minus" style={{ marginRight: 8, width: 16, textAlign: "center" }} />
                      Divider
                    </button>
                  </div>
                </div>
              )}

              {/* Filters Tab Content */}
              {sidebarTab === "filters" && (
                <div>
                  <h3 style={{ fontSize: 12, fontWeight: 600, color: "var(--text-muted)", marginBottom: 12, textTransform: "uppercase", letterSpacing: "0.5px" }}>
                    Dashboard Filters
                  </h3>
                  <DashboardFilterBarEnhanced />
                </div>
              )}
            </div>
              </>
            )}
        </div>

        {/* Main Center Area - Canvas */}
        <div style={{ background: 'var(--bg-elevated)', padding: '12px', overflow: 'auto' }}>
          <DashboardCanvas />
        </div>
      </div>

      {/* Chart Picker Modal — tile grid (container-targeted) */}
      {showChartPicker && (
          <div
            style={{
              position: "fixed", inset: 0, background: "rgba(15,23,42,0.5)",
              display: "flex", alignItems: "center", justifyContent: "center", zIndex: 1100,
            }}
            onClick={() => { setShowChartPicker(false); setChartPickerTarget(null); }}
          >
            <div
              style={{
                background: "#fff", borderRadius: 12,
                width: "min(1100px, 95vw)", height: "min(720px, 90vh)",
                display: "flex", flexDirection: "column",
                boxShadow: "0 32px 80px rgba(0,0,0,0.3)",
                overflow: "hidden",
              }}
              onClick={(e) => e.stopPropagation()}
            >
              {/* ── Header ──────────────────────────────────────────────── */}
              <div style={{
                padding: "18px 24px", borderBottom: "1px solid #e2e8f0",
                display: "flex", alignItems: "center", gap: 16, flexShrink: 0,
              }}>
                <div style={{ flex: 1 }}>
                  <h2 style={{ margin: 0, fontSize: 16, fontWeight: 700, color: "#0f172a" }}>Add charts</h2>
                </div>
                <button
                  onClick={() => { setShowChartPicker(false); setChartPickerTarget(null); }}
                  style={{ background: "none", border: "none", cursor: "pointer", color: "#64748b", fontSize: 18, padding: "4px 8px", borderRadius: 6, lineHeight: 1 }}
                  onMouseOver={(e) => { e.currentTarget.style.background = "#f1f5f9"; e.currentTarget.style.color = "#1e293b"; }}
                  onMouseOut={(e) => { e.currentTarget.style.background = "none"; e.currentTarget.style.color = "#64748b"; }}
                >
                  <i className="fas fa-times" />
                </button>
              </div>

              {/* ── Toolbar: search + mine toggle ───────────────────────── */}
              <div style={{
                padding: "12px 24px", borderBottom: "1px solid #e2e8f0",
                display: "flex", alignItems: "center", gap: 16, flexShrink: 0,
                background: "#f8fafc",
              }}>
                {/* Search */}
                <div style={{ flex: 1, position: "relative" }}>
                  <i className="fas fa-search" style={{
                    position: "absolute", left: 12, top: "50%", transform: "translateY(-50%)",
                    color: "#94a3b8", fontSize: 13, pointerEvents: "none",
                  }} />
                  <input
                    type="text"
                    placeholder="Search by name, chart type or dataset…"
                    value={chartSearch}
                    onChange={(e) => setChartSearch(e.target.value)}
                    autoFocus
                    style={{
                      width: "100%", padding: "8px 12px 8px 36px",
                      border: "1px solid #e2e8f0", borderRadius: 8,
                      fontSize: 13, outline: "none", boxSizing: "border-box",
                      background: "#fff",
                    }}
                    onFocus={(e) => { e.target.style.borderColor = "#2563eb"; e.target.style.boxShadow = "0 0 0 3px rgba(37,99,235,0.1)"; }}
                    onBlur={(e) => { e.target.style.borderColor = "#e2e8f0"; e.target.style.boxShadow = "none"; }}
                  />
                </div>

                {/* Count */}
                <span style={{ fontSize: 13, color: "#64748b", whiteSpace: "nowrap", flexShrink: 0 }}>
                  {filteredCharts.length} chart{filteredCharts.length !== 1 ? "s" : ""}
                </span>

                {/* "Show only mine" toggle */}
                <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer", flexShrink: 0, userSelect: "none" }}>
                  {/* Toggle switch */}
                  <span
                    onClick={() => setChartScope(chartScope === "mine" ? "all" : "mine")}
                    style={{
                      display: "inline-block",
                      width: 36, height: 20,
                      background: chartScope === "mine" ? "#2563eb" : "#cbd5e1",
                      borderRadius: 10,
                      position: "relative",
                      transition: "background 0.2s ease",
                      cursor: "pointer",
                      flexShrink: 0,
                    }}
                  >
                    <span style={{
                      position: "absolute",
                      top: 2, left: chartScope === "mine" ? 18 : 2,
                      width: 16, height: 16,
                      background: "#fff",
                      borderRadius: "50%",
                      transition: "left 0.2s ease",
                      boxShadow: "0 1px 3px rgba(0,0,0,0.2)",
                    }} />
                  </span>
                  <span
                    onClick={() => setChartScope(chartScope === "mine" ? "all" : "mine")}
                    style={{ fontSize: 13, color: "#475569", fontWeight: 500 }}
                  >
                    Show only mine
                  </span>
                </label>
              </div>

              {/* ── Chart grid ──────────────────────────────────────────── */}
              <div style={{ flex: 1, overflowY: "auto", padding: "20px 24px 24px" }}>
                {loading ? (
                  <div style={{ display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", height: "100%", color: "#94a3b8", gap: 12 }}>
                    <i className="fas fa-spinner fa-spin" style={{ fontSize: 28 }} />
                    <span style={{ fontSize: 14 }}>Loading charts…</span>
                  </div>
                ) : error ? (
                  <div style={{ padding: "12px 16px", background: "#fef2f2", border: "1px solid #fecaca", borderRadius: 8, color: "#dc2626", fontSize: 13 }}>
                    {error}
                  </div>
                ) : filteredCharts.length === 0 ? (
                  <div style={{ display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", height: "100%", color: "#94a3b8", gap: 10 }}>
                    <i className="fas fa-chart-bar" style={{ fontSize: 36, opacity: 0.4 }} />
                    <span style={{ fontSize: 14 }}>No charts found{chartQuery ? ` matching "${chartSearch}"` : ""}</span>
                  </div>
                ) : (
                  <div style={{
                    display: "grid",
                    gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
                    gap: 12,
                  }}>
                    {filteredCharts.map((chart) => {
                      const info = chartTypeInfo(chart.chart_type);
                      const owner = chart.owner || chart.created_by || null;
                      const modified = formatRelativeDate(chart.updated_at);
                      return (
                        <button
                          key={chart.id}
                          onClick={() => handleAddChart(chart)}
                          style={{
                            padding: 0,
                            background: "#fff",
                            border: "1px solid #e2e8f0",
                            borderRadius: 10,
                            textAlign: "left",
                            cursor: "pointer",
                            transition: "box-shadow 0.15s ease, border-color 0.15s ease",
                            display: "flex",
                            flexDirection: "column",
                            overflow: "hidden",
                          }}
                          onMouseOver={(e) => {
                            e.currentTarget.style.borderColor = "#2563eb";
                            e.currentTarget.style.boxShadow = "0 0 0 3px rgba(37,99,235,0.12)";
                          }}
                          onMouseOut={(e) => {
                            e.currentTarget.style.borderColor = "#e2e8f0";
                            e.currentTarget.style.boxShadow = "none";
                          }}
                        >
                          {/* Colored banner with chart type icon */}
                          <div style={{
                            background: info.bg,
                            padding: "16px 16px 12px",
                            display: "flex",
                            alignItems: "center",
                            justifyContent: "center",
                            height: 64,
                            borderBottom: `2px solid ${info.color}22`,
                          }}>
                            <i className={info.icon} style={{ fontSize: 28, color: info.color, opacity: 0.9 }} />
                          </div>

                          {/* Card body */}
                          <div style={{ padding: "10px 12px 12px", display: "flex", flexDirection: "column", gap: 6, flex: 1 }}>
                            {/* Chart name */}
                            <div style={{
                              fontSize: 13, fontWeight: 600, color: "#0f172a",
                              lineHeight: 1.35, wordBreak: "break-word",
                            }}>
                              {chart.name}
                            </div>

                            {/* Chart type badge */}
                            <div style={{
                              display: "inline-flex", alignItems: "center", gap: 4,
                              fontSize: 11, fontWeight: 500, color: info.color,
                              background: info.bg, padding: "2px 7px",
                              borderRadius: 6, width: "fit-content",
                            }}>
                              {info.label}
                            </div>

                            {/* Meta: dataset + date */}
                            <div style={{ marginTop: "auto", display: "flex", flexDirection: "column", gap: 3, paddingTop: 6 }}>
                              {chart.dataset_name && (
                                <div style={{ display: "flex", alignItems: "center", gap: 5, fontSize: 11, color: "#64748b" }}>
                                  <i className="fas fa-database" style={{ fontSize: 10, color: "#94a3b8", flexShrink: 0 }} />
                                  <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                                    {chart.dataset_name}
                                  </span>
                                </div>
                              )}
                              {modified && (
                                <div style={{ display: "flex", alignItems: "center", gap: 5, fontSize: 11, color: "#94a3b8" }}>
                                  <i className="fas fa-clock" style={{ fontSize: 10, flexShrink: 0 }} />
                                  {modified}
                                  {owner && <span style={{ marginLeft: 2 }}>· {owner.split("@")[0]}</span>}
                                </div>
                              )}
                            </div>
                          </div>
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>
          </div>
      )}

      {/* Save Confirmation Modal */}
      {showSaveAsModal && (
        <div
          style={{
            position: "fixed", inset: 0, background: "rgba(15,23,42,0.5)",
            display: "flex", alignItems: "center", justifyContent: "center", zIndex: 1001, padding: 20,
          }}
          onClick={() => { if (!savingAs) setShowSaveAsModal(false); }}
        >
          <div
            onClick={(e) => e.stopPropagation()}
            style={{
              width: "100%", maxWidth: 420, background: "#ffffff", borderRadius: 14,
              boxShadow: "0 24px 48px rgba(0,0,0,0.25)", padding: 24,
            }}
          >
            <h3 style={{ margin: "0 0 4px", fontSize: 17, fontWeight: 700, color: "#0f172a" }}>Save as new dashboard</h3>
            <p style={{ margin: "0 0 16px", fontSize: 13, color: "#64748b" }}>
              Creates an independent copy. The original stays unchanged.
            </p>
            <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "#475569", marginBottom: 6 }}>Name</label>
            <input
              autoFocus
              value={saveAsName}
              onChange={(e) => setSaveAsName(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") handleConfirmSaveAs(); }}
              style={{
                width: "100%", padding: "10px 12px", fontSize: 14, boxSizing: "border-box",
                border: "1px solid #e2e8f0", borderRadius: 8, outline: "none", color: "#0f172a",
              }}
            />
            {validationError && <div style={{ marginTop: 10, fontSize: 13, color: "#dc2626" }}>{validationError}</div>}
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10, marginTop: 20 }}>
              <button
                type="button"
                onClick={() => setShowSaveAsModal(false)}
                disabled={savingAs}
                style={{ padding: "9px 18px", fontSize: 13, fontWeight: 600, cursor: "pointer", border: "1px solid #e2e8f0", borderRadius: 8, background: "#fff", color: "#475569" }}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={handleConfirmSaveAs}
                disabled={savingAs}
                style={{ padding: "9px 18px", fontSize: 13, fontWeight: 700, cursor: "pointer", border: "none", borderRadius: 8, background: "#2563eb", color: "#fff", display: "flex", alignItems: "center", gap: 8, minWidth: 96, justifyContent: "center" }}
              >
                {savingAs ? <i className="fas fa-spinner fa-spin" /> : "Create copy"}
              </button>
            </div>
          </div>
        </div>
      )}

      {showSaveModal && (
        <div
          style={{
            position: "fixed",
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
            background: "rgba(0, 0, 0, 0.5)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            zIndex: 1000,
          }}
          onClick={handleCancelSave}
        >
          <div
            style={{
              background: "#ffffff",
              borderRadius: 12,
              padding: "32px",
              minWidth: 480,
              maxWidth: 600,
              boxShadow: "0 20px 50px rgba(0, 0, 0, 0.3)",
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <h2 style={{ fontSize: 20, fontWeight: 600, color: "#1e293b", marginBottom: 8 }}>
              Save Dashboard
            </h2>
            <p style={{ fontSize: 14, color: "#64748b", marginBottom: 24 }}>
              Confirm dashboard details before saving.
            </p>

            <div style={{ marginBottom: 20 }}>
              <label style={{ display: "block", fontSize: 13, fontWeight: 600, color: "#475569", marginBottom: 8 }}>
                Dashboard Name <span style={{ color: "#ef4444" }}>*</span>
              </label>
              <input
                type="text"
                value={tempName}
                onChange={(e) => {
                  setTempName(e.target.value);
                  if (validationError && e.target.value.trim()) {
                    setValidationError(null);
                  }
                }}
                placeholder="Enter dashboard name"
                style={{
                  width: "100%",
                  padding: "10px 14px",
                  border: validationError && !tempName.trim() ? "1px solid #ef4444" : "1px solid #cbd5e1",
                  borderRadius: 8,
                  fontSize: 14,
                  outline: "none",
                  transition: "border-color 0.15s ease",
                }}
                onFocus={(e) => {
                  e.target.style.borderColor = "#2563eb";
                }}
                onBlur={(e) => {
                  if (!validationError || tempName.trim()) {
                    e.target.style.borderColor = "#cbd5e1";
                  }
                }}
              />
              {validationError && !tempName.trim() && (
                <span style={{ fontSize: 12, color: "#ef4444", marginTop: 6, display: "block" }}>
                  <i className="fas fa-exclamation-circle" style={{ marginRight: 4 }} />
                  {validationError}
                </span>
              )}
            </div>

            <div style={{ marginBottom: 28 }}>
              <label style={{ display: "block", fontSize: 13, fontWeight: 600, color: "#475569", marginBottom: 8 }}>
                Description (Optional)
              </label>
              <textarea
                value={tempDescription}
                onChange={(e) => setTempDescription(e.target.value)}
                placeholder="Add a description for this dashboard"
                rows={4}
                style={{
                  width: "100%",
                  padding: "10px 14px",
                  border: "1px solid #cbd5e1",
                  borderRadius: 8,
                  fontSize: 14,
                  resize: "vertical",
                  outline: "none",
                  fontFamily: "inherit",
                  transition: "border-color 0.15s ease",
                }}
                onFocus={(e) => {
                  e.target.style.borderColor = "#2563eb";
                }}
                onBlur={(e) => {
                  e.target.style.borderColor = "#cbd5e1";
                }}
              />
            </div>

            <div style={{ display: "flex", justifyContent: "flex-end", gap: 12 }}>
              <button
                onClick={handleCancelSave}
                style={{
                  padding: "10px 20px",
                  fontSize: 14,
                  fontWeight: 500,
                  background: "#ffffff",
                  color: "#64748b",
                  border: "1px solid #cbd5e1",
                  borderRadius: 8,
                  cursor: "pointer",
                  transition: "all 0.15s ease",
                }}
                onMouseOver={(e) => {
                  e.currentTarget.style.background = "#f8fafc";
                  e.currentTarget.style.borderColor = "#94a3b8";
                }}
                onMouseOut={(e) => {
                  e.currentTarget.style.background = "#ffffff";
                  e.currentTarget.style.borderColor = "#cbd5e1";
                }}
              >
                Cancel
              </button>
              <button
                onClick={handleConfirmSave}
                style={{
                  padding: "10px 24px",
                  fontSize: 14,
                  fontWeight: 600,
                  background: "#2563eb",
                  color: "#ffffff",
                  border: "none",
                  borderRadius: 8,
                  cursor: "pointer",
                  transition: "all 0.15s ease",
                }}
                onMouseOver={(e) => {
                  e.currentTarget.style.background = "#1d4ed8";
                }}
                onMouseOut={(e) => {
                  e.currentTarget.style.background = "#2563eb";
                }}
              >
                <i className="fas fa-check" style={{ marginRight: 8 }} />
                Confirm & Save
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
    </BuilderContext.Provider>
  );
};

export default DashboardBuilder;
