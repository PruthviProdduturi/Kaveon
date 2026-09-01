"use client";

import { CHART_CATEGORIES, TEMPLATES } from "../../../components/charts/ChartBuilderContext";
import type { ChartCategory } from "../../../components/charts/ChartBuilderContext";
import { ChartIcon, CATEGORY_COLOR } from "../../../components/charts/ChartTypePicker";
import React, { useEffect, useMemo, useState } from 'react';
import { API_BASE } from '../../../config';
import { msalFetch } from '../../../utils/msalFetch';
import { useAuth } from '../../../auth/useAuth';
import { useRouter, useSearchParams } from 'next/navigation';
import { Button } from '../../../components/Button';

interface DatasetSummary {
  id: number;
  name: string;
  description?: string | null;
}

const DEFAULT_COLOR = '#0078d4';

// Height of content panels — fills remaining viewport after header + page title + action bar
const PANEL_H = 'calc(100vh - 282px)';
const PANEL_MIN_H = 520;

const ChartNewPage: React.FC = () => {
  const { isAuthenticated } = useAuth();
  const router = useRouter();
  const searchParams = useSearchParams();

  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [datasetsError, setDatasetsError] = useState<string | null>(null);
  const [selectedDatasetId, setSelectedDatasetId] = useState<number | null>(null);
  const [selectedTemplateId, setSelectedTemplateId] = useState<string | null>(null);
  const [labSql, setLabSql] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const [activeCategory, setActiveCategory] = useState<string | null>(null);
  const [chartSearch, setChartSearch] = useState('');
  const [datasetSearch, setDatasetSearch] = useState('');

  useEffect(() => {
    if (!isAuthenticated) return;
    const load = async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/datasets/summary`);
        if (!res.ok) throw new Error(`Failed to load datasets: ${res.status}`);
        const data = await res.json();
        setDatasets((data.recent || []).map((d: any) => ({
          id: d.id,
          name: d.dataset_name,
          description: d.description || null,
        })));
      } catch (e: unknown) {
        setDatasetsError(e instanceof Error ? e.message : 'Unknown error');
      }
    };
    void load();
  }, [isAuthenticated]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    if (!searchParams.get('lab')) return;
    try {
      const raw = sessionStorage.getItem('lab-visualize');
      if (raw) {
        const { sql } = JSON.parse(raw) as { sql?: string };
        setLabSql(sql || null);
        sessionStorage.removeItem('lab-visualize');
      }
    } catch { /* ignore */ }
  }, [searchParams]);

  const categories: ChartCategory[] = useMemo(
    () => CHART_CATEGORIES.filter(cat => TEMPLATES.some(t => t.category === cat.id)),
    [],
  );

  const visibleTemplates = useMemo(() => {
    let list = [...TEMPLATES];
    if (activeCategory) list = list.filter(t => t.category === activeCategory);
    if (chartSearch.trim()) {
      const q = chartSearch.toLowerCase();
      list = list.filter(t => t.name.toLowerCase().includes(q) || t.description.toLowerCase().includes(q));
    }
    return list;
  }, [activeCategory, chartSearch]);

  const filteredDatasets = useMemo(() => {
    if (!datasetSearch.trim()) return datasets;
    const q = datasetSearch.toLowerCase();
    return datasets.filter(d => d.name.toLowerCase().includes(q));
  }, [datasets, datasetSearch]);

  const selectedDataset = datasets.find(d => d.id === selectedDatasetId);
  const selectedTemplate = TEMPLATES.find(t => t.id === selectedTemplateId);
  const selectedColor = selectedTemplate ? (CATEGORY_COLOR[selectedTemplate.category] ?? DEFAULT_COLOR) : DEFAULT_COLOR;
  const canContinue = Boolean(selectedDatasetId && selectedTemplateId);

  const handleNext = async () => {
    if (!canContinue || !selectedDatasetId || !selectedTemplateId) return;
    setIsCreating(true);
    setCreateError(null);
    try {
      const name = `New Chart – ${new Date().toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })}`;
      const res = await msalFetch(`${API_BASE}/api/v1/charts`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name, dataset_id: selectedDatasetId, chart_type: selectedTemplateId }),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        throw new Error((err as any).detail || `Failed to create chart (${res.status})`);
      }
      const chart = await res.json();
      router.push(`/charts/${chart.id}`);
    } catch (e) {
      setCreateError(e instanceof Error ? e.message : 'Failed to create chart');
      setIsCreating(false);
    }
  };

  return (
    <div className="page-shell page-shell-wide" style={{ display: 'flex', flexDirection: 'column', gap: 0 }}>
      {/* ── Page header ──────────────────────────────────────────────────── */}
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '12px 0 14px', borderBottom: '1px solid var(--border)', marginBottom: 14 }}>
        <div>
          <h1 style={{ margin: 0, fontSize: 18, fontWeight: 700, color: 'var(--text-primary)', fontFamily: 'Inter, -apple-system, sans-serif', letterSpacing: '-0.02em', lineHeight: 1.2 }}>
            New chart
          </h1>
          <p style={{ margin: 0, color: 'var(--text-muted)', fontSize: 12, fontFamily: 'Inter, -apple-system, sans-serif' }}>
            {labSql ? 'Choose a dataset matching your Lab query, then pick a chart type.' : 'Select a dataset and chart type to get started.'}
          </p>
        </div>

        {/* Action area in header */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          {createError && (
            <span style={{ color: 'var(--error)', fontSize: 12, fontFamily: 'Inter, -apple-system, sans-serif' }}>{createError}</span>
          )}
          {/* Selection pills */}
          {(selectedDataset || selectedTemplate) && (
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '4px 10px', background: 'var(--bg-primary)', borderRadius: 8, border: '1px solid var(--border)' }}>
              {selectedDataset && (
                <span style={{ fontSize: 12, color: 'var(--text-primary)', fontFamily: 'Inter, -apple-system, sans-serif', fontWeight: 500 }}>
                  <i className="fas fa-table" style={{ color: 'var(--accent)', marginRight: 5, fontSize: 10 }} />
                  {selectedDataset.name.length > 22 ? selectedDataset.name.slice(0, 22) + '…' : selectedDataset.name}
                </span>
              )}
              {selectedDataset && selectedTemplate && (
                <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="var(--border)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <polyline points="5,3 11,8 5,13" />
                </svg>
              )}
              {selectedTemplate && (
                <span style={{ display: 'flex', alignItems: 'center', gap: 4, fontSize: 12, color: selectedColor, fontFamily: 'Inter, -apple-system, sans-serif', fontWeight: 500 }}>
                  <ChartIcon id={selectedTemplate.id} color={selectedColor} size={14} />
                  {selectedTemplate.name}
                </span>
              )}
            </div>
          )}
          <Button
            disabled={!canContinue || isCreating}
            onClick={() => void handleNext()}
          >
            {isCreating ? (
              <><i className="fas fa-spinner fa-spin" style={{ fontSize: 11, marginRight: 6 }} />Creating…</>
            ) : (
              <><i className="fas fa-plus" style={{ fontSize: 11, marginRight: 6 }} />Create chart</>
            )}
          </Button>
        </div>
      </div>

      {/* Lab SQL banner */}
      {labSql && (
        <div style={{ marginBottom: 12, padding: '10px 14px', background: 'color-mix(in srgb, var(--accent) 8%, var(--bg-surface))', border: '1px solid color-mix(in srgb, var(--accent) 30%, var(--border))', borderRadius: 8, fontSize: 12, fontFamily: 'Inter, -apple-system, sans-serif' }}>
          <span style={{ fontWeight: 600, color: 'var(--accent)' }}>
            <i className="fas fa-flask" style={{ marginRight: 6 }} />From SQL Lab —&nbsp;
          </span>
          <code style={{ color: 'var(--text-secondary)', wordBreak: 'break-all' }}>
            {labSql.length > 200 ? `${labSql.slice(0, 200)}…` : labSql}
          </code>
        </div>
      )}

      {!isAuthenticated && (
        <p style={{ color: 'var(--text-muted)', fontFamily: 'Inter, -apple-system, sans-serif' }}>Sign in to create charts.</p>
      )}

      {isAuthenticated && datasetsError && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading datasets</p>
          <p className="page-empty-body">{datasetsError}</p>
        </div>
      )}

      {isAuthenticated && !datasetsError && (
        <div style={{ display: 'flex', gap: 14, alignItems: 'stretch', height: PANEL_H, minHeight: PANEL_MIN_H }}>

          {/* ── Left: Dataset panel ──────────────────────────────────────── */}
          <div style={{ width: 272, flexShrink: 0, display: 'flex', flexDirection: 'column' }}>
            <div className="card" style={{ padding: 0, overflow: 'hidden', flex: 1, display: 'flex', flexDirection: 'column' }}>
              {/* Header */}
              <div style={{ padding: '12px 14px 10px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', gap: 8, flexShrink: 0 }}>
                <span style={{
                  width: 18, height: 18, borderRadius: '50%',
                  background: 'var(--text-primary)', color: 'var(--bg-surface)',
                  display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                  fontSize: 10, fontWeight: 700, letterSpacing: 0, flexShrink: 0,
                  fontFamily: 'Inter, -apple-system, sans-serif',
                }}>1</span>
                <span style={{ fontWeight: 600, fontSize: 12.5, color: 'var(--text-primary)', fontFamily: 'Inter, -apple-system, sans-serif', flex: 1 }}>
                  Dataset
                </span>
                <button
                  type="button"
                  onClick={() => router.push('/datasets/new')}
                  style={{
                    background: 'none', border: 'none', cursor: 'pointer',
                    color: 'var(--accent)', fontSize: 11, fontWeight: 500,
                    fontFamily: 'Inter, -apple-system, sans-serif',
                    display: 'flex', alignItems: 'center', gap: 3, padding: 0,
                  }}
                >
                  <i className="fas fa-plus" style={{ fontSize: 9 }} />New
                </button>
              </div>

              {/* Search */}
              <div style={{ padding: '8px 10px', borderBottom: '1px solid var(--border)', flexShrink: 0 }}>
                <div style={{ position: 'relative' }}>
                  <i className="fas fa-search" style={{ position: 'absolute', left: 8, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)', fontSize: 10, pointerEvents: 'none' }} />
                  <input
                    type="text"
                    placeholder="Search datasets..."
                    value={datasetSearch}
                    onChange={e => setDatasetSearch(e.target.value)}
                    style={{
                      width: '100%', boxSizing: 'border-box',
                      paddingLeft: 26, paddingRight: 8, paddingTop: 5, paddingBottom: 5,
                      border: '1.5px solid var(--border)', borderRadius: 6,
                      fontSize: 12, color: 'var(--text-primary)', background: 'var(--bg-primary)',
                      outline: 'none', fontFamily: 'Inter, -apple-system, sans-serif',
                    }}
                  />
                </div>
              </div>

              {/* Scrollable dataset list */}
              <div style={{ flex: 1, overflowY: 'auto', padding: '5px 7px' }}>
                {datasets.length === 0 && (
                  <div style={{ padding: '24px 8px', textAlign: 'center', color: 'var(--text-muted)', fontSize: 12, fontFamily: 'Inter, -apple-system, sans-serif' }}>
                    No datasets available yet.
                  </div>
                )}
                {filteredDatasets.length === 0 && datasets.length > 0 && (
                  <div style={{ padding: '16px 8px', textAlign: 'center', color: 'var(--text-muted)', fontSize: 12, fontFamily: 'Inter, -apple-system, sans-serif' }}>
                    No datasets match.
                  </div>
                )}
                {filteredDatasets.map(ds => {
                  const isSelected = ds.id === selectedDatasetId;
                  return (
                    <button
                      key={ds.id}
                      type="button"
                      onClick={() => setSelectedDatasetId(ds.id)}
                      style={{
                        width: '100%', textAlign: 'left',
                        padding: '8px 9px', borderRadius: 7, marginBottom: 2,
                        border: isSelected ? '1.5px solid var(--accent)' : '1.5px solid transparent',
                        background: isSelected ? 'color-mix(in srgb, var(--accent) 8%, transparent)' : 'transparent',
                        cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 8,
                        transition: 'background 0.1s',
                        fontFamily: 'Inter, -apple-system, sans-serif',
                      }}
                      onMouseEnter={e => { if (!isSelected) (e.currentTarget as HTMLButtonElement).style.background = 'var(--bg-hover)'; }}
                      onMouseLeave={e => { if (!isSelected) (e.currentTarget as HTMLButtonElement).style.background = 'transparent'; }}
                    >
                      <div style={{
                        width: 28, height: 28, borderRadius: 6, flexShrink: 0,
                        background: isSelected ? 'color-mix(in srgb, var(--accent) 15%, transparent)' : 'var(--bg-hover)',
                        display: 'flex', alignItems: 'center', justifyContent: 'center',
                      }}>
                        <i className="fas fa-table" style={{ fontSize: 11, color: isSelected ? 'var(--accent)' : 'var(--text-muted)' }} />
                      </div>
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ fontSize: 12, fontWeight: isSelected ? 600 : 500, color: isSelected ? 'var(--accent)' : 'var(--text-primary)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                          {ds.name}
                        </div>
                        {ds.description && (
                          <div style={{ fontSize: 11, color: 'var(--text-muted)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', marginTop: 1 }}>
                            {ds.description}
                          </div>
                        )}
                      </div>
                      {isSelected && (
                        <div style={{ width: 15, height: 15, borderRadius: '50%', background: 'var(--accent)', flexShrink: 0, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                          <svg viewBox="0 0 10 10" width="8" height="8"><polyline points="1.5,5 4,7.5 8.5,2.5" fill="none" stroke="var(--bg-surface)" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" /></svg>
                        </div>
                      )}
                    </button>
                  );
                })}
              </div>
            </div>
          </div>

          {/* ── Right: Chart type panel ──────────────────────────────────── */}
          <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
            <div className="card" style={{ padding: 0, overflow: 'hidden', flex: 1, display: 'flex', flexDirection: 'column' }}>
              {/* Header row: label + search */}
              <div style={{ padding: '12px 14px 10px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', gap: 10, flexShrink: 0 }}>
                <span style={{
                  width: 18, height: 18, borderRadius: '50%',
                  background: 'var(--text-primary)', color: 'var(--bg-surface)',
                  display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                  fontSize: 10, fontWeight: 700, letterSpacing: 0, flexShrink: 0,
                  fontFamily: 'Inter, -apple-system, sans-serif',
                }}>2</span>
                <span style={{ fontWeight: 600, fontSize: 12.5, color: 'var(--text-primary)', fontFamily: 'Inter, -apple-system, sans-serif' }}>
                  Chart type
                </span>
                <div style={{ marginLeft: 'auto', position: 'relative' }}>
                  <i className="fas fa-search" style={{ position: 'absolute', left: 8, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-muted)', fontSize: 10, pointerEvents: 'none' }} />
                  <input
                    type="text"
                    placeholder="Search chart types..."
                    value={chartSearch}
                    onChange={e => setChartSearch(e.target.value)}
                    style={{
                      paddingLeft: 26, paddingRight: 8, paddingTop: 5, paddingBottom: 5,
                      border: '1.5px solid var(--border)', borderRadius: 6, width: 170,
                      fontSize: 12, color: 'var(--text-primary)', background: 'var(--bg-primary)',
                      outline: 'none', fontFamily: 'Inter, -apple-system, sans-serif',
                    }}
                  />
                </div>
              </div>

              {/* Body: vertical category sidebar + chart grid */}
              <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>

                {/* ── Category sidebar ──────────────────────────────────── */}
                <div style={{ width: 118, flexShrink: 0, borderRight: '1px solid var(--border)', overflowY: 'auto', padding: '6px 0' }}>
                  {[{ id: null as string | null, label: 'All', iconClass: 'fas fa-th-large' }, ...categories].map(cat => {
                    const active = activeCategory === (cat.id ?? null);
                    const color = cat.id ? (CATEGORY_COLOR[cat.id] ?? DEFAULT_COLOR) : 'var(--text-secondary)';
                    return (
                      <button
                        key={cat.id ?? 'all'}
                        type="button"
                        onClick={() => setActiveCategory(cat.id ?? null)}
                        style={{
                          width: '100%', display: 'flex', alignItems: 'center', gap: 8,
                          padding: '7px 10px 7px 12px',
                          background: active ? `${color}12` : 'transparent',
                          border: 'none',
                          borderLeft: active ? `3px solid ${color}` : '3px solid transparent',
                          cursor: 'pointer',
                          fontFamily: 'Inter, -apple-system, sans-serif',
                          transition: 'background 0.1s',
                          textAlign: 'left',
                        }}
                      >
                        <span style={{
                          width: 22, height: 22, borderRadius: 6, flexShrink: 0,
                          background: active ? `${color}20` : 'var(--bg-hover)',
                          display: 'flex', alignItems: 'center', justifyContent: 'center',
                        }}>
                          <i className={(cat as any).iconClass || 'fas fa-shapes'} style={{ fontSize: 9.5, color: active ? color : 'var(--text-muted)' }} />
                        </span>
                        <span style={{
                          fontSize: 11.5, fontWeight: active ? 600 : 400,
                          color: active ? color : 'var(--text-secondary)',
                          whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis',
                        }}>
                          {cat.label}
                        </span>
                      </button>
                    );
                  })}
                </div>

                {/* ── Scrollable chart grid ─────────────────────────────── */}
                <div style={{ flex: 1, overflowY: 'auto', padding: '12px' }}>
                  {visibleTemplates.length === 0 ? (
                    <div style={{ padding: '30px 0', textAlign: 'center', color: 'var(--text-muted)', fontSize: 12, fontFamily: 'Inter, -apple-system, sans-serif' }}>
                      No chart types match this search.
                    </div>
                  ) : (
                    <div style={{
                      display: 'grid',
                      gridTemplateColumns: 'repeat(auto-fill, minmax(130px, 1fr))',
                      gap: 10,
                    }}>
                      {visibleTemplates.map(tpl => {
                        const isSelected = selectedTemplateId === tpl.id;
                        const color = CATEGORY_COLOR[tpl.category] ?? DEFAULT_COLOR;
                        return (
                          <button
                            key={tpl.id}
                            type="button"
                            title={tpl.description}
                            onClick={() => setSelectedTemplateId(tpl.id)}
                            style={{
                              display: 'flex', flexDirection: 'column', alignItems: 'center',
                              justifyContent: 'center', gap: 10,
                              padding: '18px 8px 14px',
                              borderRadius: 12, cursor: 'pointer',
                              border: isSelected ? `2px solid ${color}` : '1.5px solid var(--border)',
                              background: isSelected ? `${color}0e` : 'var(--bg-elevated)',
                              boxShadow: isSelected ? `0 0 0 3px ${color}20` : 'none',
                              transition: 'all 0.12s ease',
                              position: 'relative',
                              fontFamily: 'Inter, -apple-system, sans-serif',
                            }}
                            onMouseEnter={e => {
                              if (!isSelected) {
                                const el = e.currentTarget as HTMLButtonElement;
                                el.style.borderColor = color;
                                el.style.background = `${color}08`;
                                el.style.boxShadow = 'var(--shadow-sm)';
                              }
                            }}
                            onMouseLeave={e => {
                              if (!isSelected) {
                                const el = e.currentTarget as HTMLButtonElement;
                                el.style.borderColor = 'var(--border)';
                                el.style.background = 'var(--bg-elevated)';
                                el.style.boxShadow = 'none';
                              }
                            }}
                          >
                            {/* Large icon */}
                            <div style={{
                              width: 64, height: 64, borderRadius: 14, flexShrink: 0,
                              background: isSelected ? `${color}22` : `${color}12`,
                              display: 'flex', alignItems: 'center', justifyContent: 'center',
                              transition: 'background 0.12s ease',
                            }}>
                              <ChartIcon id={tpl.id} color={color} size={40} />
                            </div>

                            {/* Name */}
                            <span style={{
                              fontSize: 11.5, fontWeight: isSelected ? 600 : 500,
                              color: isSelected ? color : 'var(--text-secondary)',
                              textAlign: 'center', lineHeight: 1.35,
                              letterSpacing: '-0.01em',
                            }}>
                              {tpl.name}
                            </span>

                            {/* Selected checkmark */}
                            {isSelected && (
                              <div style={{
                                position: 'absolute', top: 6, right: 7,
                                width: 16, height: 16, borderRadius: '50%', background: color,
                                display: 'flex', alignItems: 'center', justifyContent: 'center',
                              }}>
                                <svg viewBox="0 0 10 10" width="9" height="9">
                                  <polyline points="1.5,5 4,7.5 8.5,2.5" fill="none" stroke="white" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
                                </svg>
                              </div>
                            )}
                          </button>
                        );
                      })}
                    </div>
                  )}
                </div>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

export default ChartNewPage;
