"use client";

import { CHART_CATEGORIES, TEMPLATES } from "../../../components/charts/ChartBuilderContext";
import type { ChartCategory, ChartTemplate } from "../../../components/charts/ChartBuilderContext";
import React, { useEffect, useMemo, useRef, useState } from "react";

import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useAuth } from "../../../auth/useAuth";
import { useRouter } from "next/navigation";

// using same-origin relative API calls

interface DatasetSummary {
  id: number;
  name: string;
  description?: string | null;
}

// Categories we do NOT want to show in the left sidebar for the
// create-chart page (based on the screenshot you shared).
const HIDDEN_CREATE_PAGE_CATEGORY_IDS: string[] = [
  "Scatter",
  "Candlestick",
  "Radar",
  "Boxplot",
  "Gauge",
  "PictorialBar",
  "ThemeRiver",
];

const ChartNewPage: React.FC = () => {
  const { isAuthenticated } = useAuth();
  const router = useRouter();

  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [datasetsError, setDatasetsError] = useState<string | null>(null);
  const [selectedDatasetId, setSelectedDatasetId] = useState<number | null>(null);
  const [selectedTemplateId, setSelectedTemplateId] = useState<string | null>(null);
  const [activeCategory, setActiveCategory] = useState<string | null>(null);
  const [search, setSearch] = useState<string>("");
  const [isDatasetOpen, setIsDatasetOpen] = useState(false);
  const [datasetSearch, setDatasetSearch] = useState("");
  const datasetDropdownRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!isAuthenticated) return;

    const load = async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/datasets/summary`);
        if (!res.ok) {
          throw new Error(`Failed to load datasets: ${res.status}`);
        }
        const data = await res.json();
        // The API returns { count, recent: [...] }
        setDatasets((data.recent || []).map((d: any) => ({
          id: d.id,
          name: d.dataset_name,
          description: d.description || null
        })));
      } catch (e: unknown) {
        const message = e instanceof Error ? e.message : "Unknown error";
        setDatasetsError(message);
      }
    };

    void load();
  }, [isAuthenticated]);

  const categories: ChartCategory[] = useMemo(
    () =>
      CHART_CATEGORIES.filter(
        (cat) =>
          !HIDDEN_CREATE_PAGE_CATEGORY_IDS.includes(cat.id) &&
          TEMPLATES.some((t) => t.category === cat.id),
      ),
    [],
  );

  const visibleTemplates = useMemo(() => {
    let list = activeCategory ? TEMPLATES.filter((t) => t.category === activeCategory) : TEMPLATES;
    if (search.trim()) {
      const q = search.toLowerCase();
      list = list.filter(
        (t) => t.name.toLowerCase().includes(q) || t.description.toLowerCase().includes(q),
      );
    }
    return list;
  }, [activeCategory, search]);

  const filteredDatasets = useMemo(() => {
    if (!datasetSearch.trim()) return datasets;
    const q = datasetSearch.toLowerCase();
    return datasets.filter((d) => d.name.toLowerCase().includes(q));
  }, [datasets, datasetSearch]);

  // Close dataset dropdown when clicking outside
  useEffect(() => {
    if (!isDatasetOpen) return;

    const handleClickOutside = (event: MouseEvent) => {
      if (!datasetDropdownRef.current) return;
      if (!(event.target instanceof Node)) return;
      if (!datasetDropdownRef.current.contains(event.target)) {
        setIsDatasetOpen(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, [isDatasetOpen]);

  const canContinue = Boolean(selectedDatasetId && selectedTemplateId);

  const handleNext = () => {
    if (!canContinue || !selectedDatasetId || !selectedTemplateId) return;
    const params = new URLSearchParams({
      datasetId: selectedDatasetId.toString(),
      template: selectedTemplateId,
    });
    void router.push(`/charts/new/build?${params.toString()}`);
  };

  return (
    <div className="page-shell page-shell-wide">
      <header className="page-header">
        <h1 className="page-header-title">Create a new chart</h1>
        <p className="page-header-subtitle">
          First, choose a dataset and chart type. You&apos;ll configure metrics and filters on the next step.
        </p>
      </header>

      {!isAuthenticated && <p className="muted">Sign in to create charts.</p>}

      {isAuthenticated && datasetsError && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading datasets</p>
          <p className="page-empty-body">{datasetsError}</p>
        </div>
      )}

      {isAuthenticated && !datasetsError && (
        <div className="chart-step-stack">
          <section className="card chart-step-card">
            <div className="chart-step-header-row">
              <h2 className="section-title">1. Choose a dataset</h2>
            </div>
            {datasets.length === 0 && <p className="muted">No datasets available yet.</p>}
            {datasets.length > 0 && (
              <div className="chart-dataset-row">
                <div className="chart-dataset-main">
                    <label className="chart-builder-label">Dataset</label>
                    <div
                      ref={datasetDropdownRef}
                      className={`dropdown-container chart-dataset-dropdown ${isDatasetOpen ? "open" : ""}`}
                    >
                      <div
                        className={
                          isDatasetOpen
                            ? "chart-dataset-combobox chart-dataset-combobox-open"
                            : "chart-dataset-combobox"
                        }
                      >
                        <input
                          type="text"
                          className="chart-dataset-input"
                          placeholder={
                            datasets.length === 0
                              ? "No datasets available"
                              : "Search datasets..."
                          }
                          value={datasetSearch}
                          onFocus={() => {
                            if (datasets.length > 0) setIsDatasetOpen(true);
                          }}
                          onChange={(e) => {
                            setDatasetSearch(e.target.value);
                            if (!isDatasetOpen && datasets.length > 0) {
                              setIsDatasetOpen(true);
                            }
                          }}
                          disabled={datasets.length === 0}
                        />
                        <span
                          className="chart-dataset-input-icon"
                          onMouseDown={(e) => {
                            e.preventDefault();
                            if (datasets.length === 0) return;
                            setIsDatasetOpen((open) => !open);
                          }}
                        >
                          <i className={isDatasetOpen ? "fas fa-search" : "fas fa-chevron-down"} />
                        </span>
                      </div>
                    {isDatasetOpen && (
                      <div className="dropdown-menu chart-dataset-menu show">
                        {filteredDatasets.map((ds) => (
                          <button
                            key={ds.id}
                            type="button"
                            className={
                              "dropdown-item chart-dataset-item" +
                              (ds.id === selectedDatasetId ? " active" : "")
                            }
                            onClick={() => {
                              setSelectedDatasetId(ds.id);
                              setDatasetSearch(ds.name);
                              setIsDatasetOpen(false);
                            }}
                          >
                            <span>{ds.name}</span>
                            {ds.description && <small>{ds.description}</small>}
                          </button>
                        ))}
                        {filteredDatasets.length === 0 && (
                          <div className="chart-dataset-empty">No datasets match this search.</div>
                        )}
                      </div>
                    )}
                  </div>
                </div>
                <button
                  type="button"
                  className="overview-section-link chart-dataset-create-btn"
                  onClick={() => router.push("/datasets/new")}
                >
                  <i className="fas fa-plus" />
                  New dataset
                </button>
              </div>
            )}
            {selectedDatasetId && (
              <p className="muted" style={{ marginTop: 8, fontSize: 12 }}>
                {datasets.find((d) => d.id === selectedDatasetId)?.description}
              </p>
            )}
          </section>

          <section className="card chart-step-card chart-step-card-charts">
            <h2 className="section-title">2. Choose chart type</h2>
            <div className="chart-chooser-layout">
              <aside className="chart-chooser-sidebar">
                <div className="chart-chooser-sidebar-group">
                  <button
                    type="button"
                    className={
                      !activeCategory
                        ? "chart-sidebar-item chart-sidebar-item-active"
                        : "chart-sidebar-item"
                    }
                    onClick={() => setActiveCategory(null)}
                  >
                    <span className="chart-sidebar-icon-wrap">
                      <i className="fas fa-layer-group" />
                    </span>
                    <span>All charts</span>
                  </button>
                </div>
                <div className="chart-chooser-sidebar-group">
                  <div className="chart-chooser-sidebar-label">Category</div>
                  {categories.map((cat) => (
                    <button
                      key={cat.id}
                      type="button"
                      className={
                        activeCategory === cat.id
                          ? "chart-sidebar-item chart-sidebar-item-active"
                          : "chart-sidebar-item"
                      }
                      onClick={() => setActiveCategory(cat.id)}
                    >
                      <span className="chart-sidebar-icon-wrap">
                        <i className={cat.iconClass} />
                      </span>
                      <span>{cat.label}</span>
                    </button>
                  ))}
                </div>
              </aside>

              <div className="chart-chooser-main">
                <div className="chart-chooser-main-header">
                  <input
                    type="text"
                    className="chart-search-input"
                    placeholder="Search all chart types"
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                  />
                </div>
                <div className="chart-picker-row chart-picker-row-scroll">
                  {visibleTemplates.map((tpl) => (
                    <button
                      key={tpl.id}
                      type="button"
                      className="chart-picker-tile"
                      onClick={() => setSelectedTemplateId(tpl.id)}
                      style={
                        selectedTemplateId === tpl.id
                          ? {
                              boxShadow:
                                "0 0 0 1px #2563eb, 0 10px 22px rgba(37, 99, 235, 0.18)",
                            }
                          : undefined
                      }
                    >
                      <div
                        className={
                          tpl.previewKind
                            ? `chart-picker-preview chart-preview-${tpl.previewKind}`
                            : "chart-picker-preview"
                        }
                      >
                        {tpl.thumbnail && (
                          <img
                            src={tpl.thumbnail}
                            alt={tpl.name}
                            className="chart-picker-thumb"
                          />
                        )}
                      </div>
                      <div className="chart-picker-meta">
                        <div className="chart-picker-name">{tpl.name}</div>
                        <div className="chart-picker-description">{tpl.description}</div>
                      </div>
                    </button>
                  ))}
                  {visibleTemplates.length === 0 && (
                    <div className="muted" style={{ fontSize: 12 }}>
                      No chart types match this search.
                    </div>
                  )}
                </div>
              </div>
            </div>

            <div style={{ marginTop: 18, textAlign: "right" }}>
              <button
                type="button"
                className="overview-primary-btn"
                disabled={!canContinue}
                style={{ opacity: canContinue ? 1 : 0.6 }}
                onClick={handleNext}
              >
                Next: Configure chart
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
};

export default ChartNewPage;
