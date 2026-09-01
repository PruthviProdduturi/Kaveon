"use client";

import React, { useEffect, useMemo, useState } from "react";
import { useChartBuilder } from "./ChartBuilderContext";

// ── Inline SVG mini-chart icons ──────────────────────────────────────────────
export const ChartIcon: React.FC<{ id: string; color: string; size?: number }> = ({ id, color, size = 28 }) => {
  const s = { width: size, height: size, display: "block" };
  switch (id) {
    case "time_series_line":
    case "line_multi_series":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <polyline points="2,22 8,14 14,17 20,8 26,4" fill="none" stroke={color} strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      );
    case "time_series_line_share":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <polyline points="2,22 8,16 14,18 20,10 26,6" fill="none" stroke={color} strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" strokeDasharray="3 1.5" />
          <polyline points="2,24 8,20 14,22 20,16 26,12" fill="none" stroke={color} strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" opacity="0.5" />
        </svg>
      );
    case "time_series_area":
    case "area_stack":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <path d="M2,22 L8,14 L14,17 L20,8 L26,4 L26,24 L2,24 Z" fill={color} fillOpacity="0.25" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      );
    case "time_series_area_share":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <path d="M2,24 L26,24 L26,6 L20,10 L14,15 L8,18 L2,20 Z" fill={color} fillOpacity="0.35" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
          <path d="M2,20 L8,18 L14,15 L20,10 L26,6 L26,14 L20,16 L14,20 L8,22 L2,24 Z" fill={color} fillOpacity="0.15" stroke="none" />
        </svg>
      );
    case "bar_vertical":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="3" y="12" width="5" height="12" rx="1.5" fill={color} />
          <rect x="11" y="7" width="5" height="17" rx="1.5" fill={color} fillOpacity="0.75" />
          <rect x="19" y="15" width="5" height="9" rx="1.5" fill={color} fillOpacity="0.5" />
        </svg>
      );
    case "grouped_bar":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2"  y="14" width="4" height="10" rx="1.2" fill={color} />
          <rect x="7"  y="10" width="4" height="14" rx="1.2" fill={color} fillOpacity="0.5" />
          <rect x="13" y="9"  width="4" height="15" rx="1.2" fill={color} />
          <rect x="18" y="14" width="4" height="10" rx="1.2" fill={color} fillOpacity="0.5" />
          <rect x="23" y="17" width="3" height="7"  rx="1.2" fill={color} />
        </svg>
      );
    case "stacked_bar_vertical":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="3"  y="17" width="5" height="7"  rx="1.5" fill={color} />
          <rect x="3"  y="12" width="5" height="5"  rx="0"   fill={color} fillOpacity="0.55" />
          <rect x="3"  y="9"  width="5" height="3"  rx="1.5 1.5 0 0" fill={color} fillOpacity="0.3" />
          <rect x="11" y="13" width="5" height="11" rx="1.5" fill={color} />
          <rect x="11" y="7"  width="5" height="6"  rx="0"   fill={color} fillOpacity="0.55" />
          <rect x="11" y="4"  width="5" height="3"  rx="1.5 1.5 0 0" fill={color} fillOpacity="0.3" />
          <rect x="19" y="18" width="5" height="6"  rx="1.5" fill={color} />
          <rect x="19" y="14" width="5" height="4"  rx="0"   fill={color} fillOpacity="0.55" />
          <rect x="19" y="11" width="5" height="3"  rx="1.5 1.5 0 0" fill={color} fillOpacity="0.3" />
        </svg>
      );
    case "bar_horizontal":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2" y="4" width="16" height="5" rx="1.5" fill={color} />
          <rect x="2" y="12" width="22" height="5" rx="1.5" fill={color} fillOpacity="0.75" />
          <rect x="2" y="20" width="11" height="5" rx="1.5" fill={color} fillOpacity="0.5" />
        </svg>
      );
    case "stacked_bar_horizontal":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2" y="4"  width="12" height="5" rx="1.5" fill={color} />
          <rect x="14" y="4" width="7"  height="5" rx="0"   fill={color} fillOpacity="0.55" />
          <rect x="21" y="4" width="4"  height="5" rx="0 1.5 1.5 0" fill={color} fillOpacity="0.3" />
          <rect x="2" y="12" width="16" height="5" rx="1.5" fill={color} />
          <rect x="18" y="12" width="7" height="5" rx="0"   fill={color} fillOpacity="0.55" />
          <rect x="2" y="20" width="9"  height="5" rx="1.5" fill={color} />
          <rect x="11" y="20" width="10" height="5" rx="0"  fill={color} fillOpacity="0.55" />
          <rect x="21" y="20" width="4"  height="5" rx="0 1.5 1.5 0" fill={color} fillOpacity="0.3" />
        </svg>
      );
    case "pie":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <circle cx="14" cy="14" r="11" fill="none" stroke={color} strokeWidth="11" strokeDasharray="22 48" strokeDashoffset="0" opacity="0.85" />
          <circle cx="14" cy="14" r="11" fill="none" stroke={color} strokeWidth="11" strokeDasharray="14 56" strokeDashoffset="-22" opacity="0.5" />
          <circle cx="14" cy="14" r="11" fill="none" stroke={color} strokeWidth="11" strokeDasharray="12 58" strokeDashoffset="-36" opacity="0.3" />
          <circle cx="14" cy="14" r="4" fill="var(--bg-surface)" />
        </svg>
      );
    case "donut":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <circle cx="14" cy="14" r="10" fill="none" stroke={color} strokeWidth="5" strokeDasharray="20 43" opacity="0.85" />
          <circle cx="14" cy="14" r="10" fill="none" stroke={color} strokeWidth="5" strokeDasharray="13 50" strokeDashoffset="-20" opacity="0.5" />
          <circle cx="14" cy="14" r="10" fill="none" stroke={color} strokeWidth="5" strokeDasharray="10 53" strokeDashoffset="-33" opacity="0.3" />
        </svg>
      );
    case "scatter":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          {[[5,20],[9,12],[13,18],[17,7],[21,14],[24,9]].map(([x, y], i) => (
            <circle key={i} cx={x} cy={y} r="2.5" fill={color} fillOpacity={0.6 + i * 0.07} />
          ))}
        </svg>
      );
    case "bubble":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <circle cx="7" cy="19" r="3.5" fill={color} fillOpacity="0.45" />
          <circle cx="15" cy="10" r="5.5" fill={color} fillOpacity="0.65" />
          <circle cx="23" cy="17" r="2.5" fill={color} fillOpacity="0.5" />
        </svg>
      );
    case "heatmap":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          {[0,1,2,3].map(row =>
            [0,1,2,3].map(col => {
              const intensity = (row + col) / 6;
              return <rect key={`${row}-${col}`} x={2 + col * 6.5} y={2 + row * 6.5} width="5.5" height="5.5" rx="1" fill={color} fillOpacity={0.15 + intensity * 0.85} />;
            })
          )}
        </svg>
      );
    case "radar":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <polygon points="14,3 24,10 21,22 7,22 4,10" fill={color} fillOpacity="0.2" stroke={color} strokeWidth="1.5" strokeLinejoin="round" />
          <polygon points="14,7 20,12 18,20 10,20 8,12" fill={color} fillOpacity="0.35" stroke={color} strokeWidth="1.2" strokeLinejoin="round" strokeDasharray="2 1" />
        </svg>
      );
    case "funnel":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <path d="M3,4 L25,4 L20,11 L20,24 L8,24 L8,11 Z" fill={color} fillOpacity="0.2" stroke={color} strokeWidth="1.5" strokeLinejoin="round" />
          <rect x="8" y="11" width="12" height="4" fill={color} fillOpacity="0.45" rx="1" />
          <rect x="10" y="17" width="8" height="3.5" fill={color} fillOpacity="0.65" rx="1" />
          <rect x="12" y="22" width="4" height="2.5" fill={color} fillOpacity="0.85" rx="1" />
        </svg>
      );
    case "gauge":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <path d="M4,20 A10,10 0 0,1 24,20" fill="none" stroke={color} strokeWidth="3" strokeOpacity="0.25" strokeLinecap="round" />
          <path d="M4,20 A10,10 0 0,1 18,11" fill="none" stroke={color} strokeWidth="3" strokeLinecap="round" />
          <line x1="14" y1="20" x2="18" y2="12" stroke={color} strokeWidth="2" strokeLinecap="round" />
          <circle cx="14" cy="20" r="2.5" fill={color} />
        </svg>
      );
    case "boxplot":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <line x1="7" y1="5" x2="7" y2="23" stroke={color} strokeWidth="1.5" strokeOpacity="0.4" />
          <rect x="4" y="10" width="6" height="9" rx="1" fill={color} fillOpacity="0.25" stroke={color} strokeWidth="1.5" />
          <line x1="4" y1="14" x2="10" y2="14" stroke={color} strokeWidth="2" />
          <line x1="16" y1="8" x2="16" y2="22" stroke={color} strokeWidth="1.5" strokeOpacity="0.4" />
          <rect x="13" y="12" width="6" height="8" rx="1" fill={color} fillOpacity="0.25" stroke={color} strokeWidth="1.5" />
          <line x1="13" y1="16" x2="19" y2="16" stroke={color} strokeWidth="2" />
        </svg>
      );
    case "candlestick":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          {[[5,6,10,16],[11,9,14,20],[17,4,12,19],[23,11,8,14]].map(([x, top, h, wh], i) => (
            <g key={i}>
              <line x1={x} y1={top} x2={x} y2={top + wh} stroke={color} strokeWidth="1.2" strokeOpacity="0.5" />
              <rect x={x - 2.5} y={top + 4} width="5" height={h - 4} rx="1" fill={i % 2 === 0 ? color : "var(--bg-surface)"} stroke={color} strokeWidth="1.2" />
            </g>
          ))}
        </svg>
      );
    case "treemap":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2" y="2" width="14" height="14" rx="1.5" fill={color} fillOpacity="0.7" />
          <rect x="18" y="2" width="8" height="6" rx="1.5" fill={color} fillOpacity="0.45" />
          <rect x="18" y="10" width="8" height="6" rx="1.5" fill={color} fillOpacity="0.3" />
          <rect x="2" y="18" width="9" height="8" rx="1.5" fill={color} fillOpacity="0.4" />
          <rect x="13" y="18" width="13" height="8" rx="1.5" fill={color} fillOpacity="0.55" />
        </svg>
      );
    case "sunburst":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <circle cx="14" cy="14" r="4" fill={color} fillOpacity="0.9" />
          <circle cx="14" cy="14" r="8" fill="none" stroke={color} strokeWidth="4.5" strokeDasharray="12 4 8 3" strokeOpacity="0.6" />
          <circle cx="14" cy="14" r="12.5" fill="none" stroke={color} strokeWidth="3" strokeDasharray="9 3 6 3 4 3" strokeOpacity="0.35" />
        </svg>
      );
    case "pictorial_bar":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          {[3, 10, 17].map((x, i) => (
            <g key={i}>
              {[0,1,2,3].slice(0, 4 - i).map(j => (
                <rect key={j} x={x} y={22 - j * 5.5} width="5" height="4" rx="2.5" fill={color} fillOpacity={0.4 + j * 0.15} />
              ))}
            </g>
          ))}
        </svg>
      );
    case "theme_river":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <path d="M2,10 C7,8 12,6 14,7 C16,8 21,10 26,9" fill="none" stroke={color} strokeWidth="4" strokeLinecap="round" strokeOpacity="0.7" />
          <path d="M2,16 C7,18 12,20 14,19 C16,18 21,16 26,17" fill="none" stroke={color} strokeWidth="4" strokeLinecap="round" strokeOpacity="0.4" />
          <path d="M2,22 C7,23 12,22 14,23 C16,24 21,22 26,24" fill="none" stroke={color} strokeWidth="3" strokeLinecap="round" strokeOpacity="0.25" />
        </svg>
      );
    case "mixed_line_bar":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2" y="14" width="5" height="10" rx="1.5" fill={color} fillOpacity="0.7" />
          <rect x="9" y="10" width="5" height="14" rx="1.5" fill={color} fillOpacity="0.5" />
          <rect x="16" y="16" width="5" height="8" rx="1.5" fill={color} fillOpacity="0.35" />
          <polyline points="4,13 11,7 18,10 25,4" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      );
    case "waterfall":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2" y="10" width="5" height="8" rx="1.5" fill={color} fillOpacity="0.75" />
          <rect x="9" y="7" width="5" height="5" rx="1.5" fill={color} fillOpacity="0.85" />
          <rect x="16" y="12" width="5" height="4" rx="1.5" fill="var(--error)" fillOpacity="0.75" />
          <rect x="23" y="8" width="3" height="8" rx="1.5" fill={color} fillOpacity="0.6" />
        </svg>
      );
    case "nightingale_rose":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          {[0,1,2,3,4,5].map(i => {
            const angle = (i * 60 - 90) * Math.PI / 180;
            const r = 5 + i * 1.8;
            const x1 = 14, y1 = 14;
            const x2 = 14 + Math.cos(angle) * r;
            const y2 = 14 + Math.sin(angle) * r;
            const x3 = 14 + Math.cos(angle + 60 * Math.PI / 180) * r;
            const y3 = 14 + Math.sin(angle + 60 * Math.PI / 180) * r;
            return <path key={i} d={`M${x1},${y1} L${x2},${y2} A${r},${r} 0 0,1 ${x3},${y3} Z`} fill={color} fillOpacity={0.35 + i * 0.1} />;
          })}
          <circle cx="14" cy="14" r="2.5" fill={color} />
        </svg>
      );
    case "histogram":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2"  y="20" width="3.5" height="4"  rx="1" fill={color} fillOpacity="0.4" />
          <rect x="6"  y="16" width="3.5" height="8"  rx="1" fill={color} fillOpacity="0.55" />
          <rect x="10" y="10" width="3.5" height="14" rx="1" fill={color} fillOpacity="0.75" />
          <rect x="14" y="7"  width="3.5" height="17" rx="1" fill={color} />
          <rect x="18" y="11" width="3.5" height="13" rx="1" fill={color} fillOpacity="0.75" />
          <rect x="22" y="17" width="3.5" height="7"  rx="1" fill={color} fillOpacity="0.45" />
        </svg>
      );
    case "sankey":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2" y="4"  width="4" height="8"  rx="1.5" fill={color} />
          <rect x="2" y="16" width="4" height="8"  rx="1.5" fill={color} fillOpacity="0.6" />
          <rect x="22" y="3"  width="4" height="5"  rx="1.5" fill={color} />
          <rect x="22" y="11" width="4" height="5"  rx="1.5" fill={color} fillOpacity="0.75" />
          <rect x="22" y="19" width="4" height="6"  rx="1.5" fill={color} fillOpacity="0.5" />
          <path d="M6,8 C14,8 14,5.5 22,5.5"  fill="none" stroke={color} strokeWidth="2.5" strokeOpacity="0.6" />
          <path d="M6,12 C14,12 14,13.5 22,13.5" fill="none" stroke={color} strokeWidth="2" strokeOpacity="0.5" />
          <path d="M6,20 C14,20 14,21.5 22,21.5" fill="none" stroke={color} strokeWidth="2.5" strokeOpacity="0.4" />
        </svg>
      );
    case "calendar_heatmap":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          {[0,1,2,3].map(row =>
            [0,1,2,3,4,5,6].map(col => {
              const intensity = Math.random(); // deterministic enough for icon
              const val = ((row * 7 + col) * 17) % 10 / 10;
              return <rect key={`${row}-${col}`} x={1 + col * 4} y={6 + row * 5} width="3.5" height="4" rx="0.5" fill={color} fillOpacity={0.1 + val * 0.85} />;
            })
          )}
          <rect x="1" y="2" width="26" height="2.5" rx="1" fill={color} fillOpacity="0.3" />
        </svg>
      );
    case "parallel_coordinates":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          {[4, 10, 16, 22].map(x => (
            <line key={x} x1={x} y1="4" x2={x} y2="24" stroke={color} strokeWidth="1.5" strokeOpacity="0.35" />
          ))}
          <polyline points="4,8 10,16 16,10 22,18"  fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" />
          <polyline points="4,14 10,7  16,19 22,11" fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeOpacity="0.6" />
          <polyline points="4,20 10,13 16,14 22,6"  fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeOpacity="0.4" />
        </svg>
      );
    case "world_map":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <ellipse cx="14" cy="14" rx="11" ry="11" fill="none" stroke={color} strokeWidth="1.4" strokeOpacity="0.4" />
          <ellipse cx="14" cy="14" rx="5.5" ry="11" fill="none" stroke={color} strokeWidth="1.2" strokeOpacity="0.35" />
          <line x1="3" y1="14" x2="25" y2="14" stroke={color} strokeWidth="1.2" strokeOpacity="0.35" />
          <line x1="5" y1="9" x2="23" y2="9" stroke={color} strokeWidth="1" strokeOpacity="0.25" />
          <line x1="5" y1="19" x2="23" y2="19" stroke={color} strokeWidth="1" strokeOpacity="0.25" />
          <path d="M8,6 Q11,10 10,14 Q9,18 11,22" fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" />
          <path d="M16,6 Q18,10 18,14 Q18,18 17,22" fill="none" stroke={color} strokeWidth="1.3" strokeLinecap="round" strokeOpacity="0.6" />
        </svg>
      );
    case "big_number":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <text x="14" y="20" textAnchor="middle" fontSize="18" fontWeight="800" fontFamily="Inter,sans-serif" fill={color}>42</text>
        </svg>
      );
    case "big_number_trend":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <text x="14" y="15" textAnchor="middle" fontSize="13" fontWeight="800" fontFamily="Inter,sans-serif" fill={color}>42</text>
          <polyline points="3,25 8,21 13,23 19,18 25,16" fill="none" stroke={color} strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      );
    case "table":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2" y="4" width="24" height="5" rx="1.5" fill={color} fillOpacity="0.8" />
          {[11, 17, 23].map((y, i) => (
            <rect key={i} x="2" y={y} width="24" height="4" rx="1" fill={color} fillOpacity={0.2 + i * 0.05} />
          ))}
        </svg>
      );
    case "pivot_table":
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <rect x="2" y="2" width="11" height="5" rx="1.5" fill={color} fillOpacity="0.8" />
          <rect x="15" y="2" width="11" height="5" rx="1.5" fill={color} fillOpacity="0.8" />
          {[9, 15, 21].map((y, i) => (
            <React.Fragment key={i}>
              <rect x="2" y={y} width="11" height="4.5" rx="1" fill={color} fillOpacity={0.3 - i * 0.05} />
              <rect x="15" y={y} width="11" height="4.5" rx="1" fill={color} fillOpacity={0.2 - i * 0.03} />
            </React.Fragment>
          ))}
        </svg>
      );
    default:
      return (
        <svg viewBox="0 0 28 28" style={s}>
          <circle cx="14" cy="14" r="10" fill={color} fillOpacity="0.25" stroke={color} strokeWidth="1.5" />
        </svg>
      );
  }
};

// Category → accent color
export const CATEGORY_COLOR: Record<string, string> = {
  Line:        "#0078d4",
  Bar:         "#7c3aed",
  Pie:         "#db2777",
  Scatter:     "#0891b2",
  Heatmap:     "#059669",
  Radar:       "#d97706",
  Funnel:      "#dc2626",
  Gauge:       "#7c3aed",
  Boxplot:     "#0891b2",
  Candlestick: "#b45309",
  Treemap:     "#059669",
  Sunburst:    "#db2777",
  PictorialBar:"#0891b2",
  ThemeRiver:  "#0078d4",
  Flow:        "#0ea5e9",
  Map:         "#0078d4",
  Custom:      "#6366f1",
  Dataset:     "#475569",
};

const DEFAULT_COLOR = "#0078d4";

// ── Component ────────────────────────────────────────────────────────────────
const ChartTypePicker: React.FC = () => {
  const { categories, templates, chartType, setChartType } = useChartBuilder();

  // Derive all category IDs that actually have templates (including uncatalogued ones)
  const allCategories = useMemo(() => {
    const fromTemplates = Array.from(new Set(templates.map(t => t.category)));
    const registered = categories.map(c => c.id);
    const merged = [...registered, ...fromTemplates.filter(c => !registered.includes(c))];
    return [{ id: "all", label: "All" }, ...merged.map(id => {
      const cat = categories.find(c => c.id === id);
      return { id, label: cat?.label ?? id };
    })];
  }, [categories, templates]);

  const [selectedCat, setSelectedCat] = useState<string>("all");
  // Collapsed by default — open when user wants to change the chart type
  const [open, setOpen] = useState<boolean>(false);

  const filteredTemplates = useMemo(
    () => templates.filter(t => selectedCat === "all" || t.category === selectedCat),
    [templates, selectedCat],
  );

  const selectedTemplate = templates.find(t => t.id === chartType);
  const selectedColor = selectedTemplate ? (CATEGORY_COLOR[selectedTemplate.category] ?? DEFAULT_COLOR) : "var(--text-muted)";

  // When editing an existing chart, highlight its category tab
  useEffect(() => {
    if (!chartType) return;
    const tmpl = templates.find(t => t.id === chartType);
    if (tmpl) setSelectedCat(prev => prev === "all" ? tmpl.category : prev);
  }, [chartType, templates]);

  const handleSelect = (id: string) => {
    setChartType(id as any);
    setOpen(false);
  };

  return (
    <div className="chart-builder-field-group">
      <label className="chart-builder-label">Chart type</label>

      {/* ── Collapsed trigger ────────────────────────────────────────────── */}
      <button
        type="button"
        onClick={() => setOpen(o => !o)}
        style={{
          width: "100%",
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "7px 10px",
          borderRadius: 7,
          border: open ? `1.5px solid ${selectedColor}` : "1.5px solid var(--border)",
          background: open ? `${selectedColor}08` : "var(--bg-surface)",
          cursor: "pointer",
          transition: "all 0.15s ease",
          textAlign: "left",
          boxShadow: open ? `0 0 0 3px ${selectedColor}15` : "none",
        }}
        onMouseEnter={e => { if (!open) { (e.currentTarget as HTMLButtonElement).style.borderColor = "var(--text-muted)"; } }}
        onMouseLeave={e => { if (!open) { (e.currentTarget as HTMLButtonElement).style.borderColor = "var(--border)"; } }}
      >
        {/* Mini icon */}
        <div style={{
          width: 28, height: 28, borderRadius: 6, flexShrink: 0,
          background: selectedTemplate ? `${selectedColor}15` : "var(--bg-hover)",
          display: "flex", alignItems: "center", justifyContent: "center",
        }}>
          {selectedTemplate
            ? <ChartIcon id={selectedTemplate.id} color={selectedColor} />
            : <svg viewBox="0 0 28 28" width="16" height="16"><rect x="3" y="12" width="5" height="12" rx="1.5" fill="var(--text-muted)"/><rect x="11" y="7" width="5" height="17" rx="1.5" fill="var(--text-muted)" fillOpacity="0.6"/><rect x="19" y="15" width="5" height="9" rx="1.5" fill="var(--text-muted)" fillOpacity="0.4"/></svg>
          }
        </div>

        {/* Label */}
        <span style={{
          flex: 1,
          fontSize: 12.5,
          fontWeight: selectedTemplate ? 500 : 400,
          color: selectedTemplate ? "var(--text-primary)" : "var(--text-muted)",
          fontFamily: "Inter, -apple-system, sans-serif",
        }}>
          {selectedTemplate?.name ?? "Select a chart type..."}
        </span>

        {/* Chevron */}
        <svg
          viewBox="0 0 16 16" width="14" height="14"
          style={{ flexShrink: 0, transition: "transform 0.2s", transform: open ? "rotate(180deg)" : "none" }}
        >
          <polyline points="3,5 8,11 13,5" fill="none" stroke="var(--text-muted)" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>

      {/* ── Expanded panel ───────────────────────────────────────────────── */}
      {open && (
        <div style={{
          marginTop: 6,
          border: "1.5px solid var(--border)",
          borderRadius: 10,
          background: "var(--bg-surface)",
          padding: "10px 10px 8px",
          boxShadow: "var(--shadow-md)",
        }}>
          {/* Category pill strip */}
          <div style={{ display: "flex", flexWrap: "wrap", gap: 5, marginBottom: 10 }}>
            {allCategories.map(cat => {
              const active = selectedCat === cat.id;
              const color = CATEGORY_COLOR[cat.id] ?? DEFAULT_COLOR;
              return (
                <button
                  key={cat.id}
                  onClick={() => setSelectedCat(cat.id)}
                  style={{
                    display: "inline-flex", alignItems: "center",
                    padding: "3px 9px", borderRadius: 20,
                    border: active ? `1.5px solid ${color}` : "1.5px solid var(--border)",
                    background: active ? `${color}15` : "var(--bg-primary)",
                    color: active ? color : "var(--text-muted)",
                    fontSize: 11, fontWeight: active ? 600 : 400,
                    fontFamily: "Inter, -apple-system, sans-serif",
                    cursor: "pointer", transition: "all 0.15s ease", whiteSpace: "nowrap",
                  }}
                >
                  {cat.label}
                </button>
              );
            })}
          </div>

          {/* Chart type card grid */}
          <div style={{
            display: "grid",
            gridTemplateColumns: "repeat(3, 1fr)",
            gap: 6,
            maxHeight: 300,
            overflowY: "auto",
            paddingRight: 1,
          }}>
        {filteredTemplates.map(tmpl => {
          const selected = chartType === tmpl.id;
          const color = CATEGORY_COLOR[tmpl.category] ?? DEFAULT_COLOR;
          return (
            <button
              key={tmpl.id}
              title={tmpl.description}
              onClick={() => handleSelect(tmpl.id)}
              style={{
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                justifyContent: "center",
                gap: 7,
                padding: "10px 6px 9px",
                borderRadius: 10,
                border: selected ? `2px solid ${color}` : "1.5px solid var(--border)",
                background: selected ? `${color}0d` : "var(--bg-surface)",
                cursor: "pointer",
                transition: "all 0.15s ease",
                boxShadow: selected ? `0 0 0 3px ${color}22` : "none",
                minHeight: 76,
                position: "relative",
              }}
              onMouseEnter={e => {
                if (!selected) {
                  (e.currentTarget as HTMLButtonElement).style.borderColor = color;
                  (e.currentTarget as HTMLButtonElement).style.background = `${color}08`;
                  (e.currentTarget as HTMLButtonElement).style.boxShadow = `var(--shadow-sm)`;
                }
              }}
              onMouseLeave={e => {
                if (!selected) {
                  (e.currentTarget as HTMLButtonElement).style.borderColor = "var(--border)";
                  (e.currentTarget as HTMLButtonElement).style.background = "var(--bg-surface)";
                  (e.currentTarget as HTMLButtonElement).style.boxShadow = "none";
                }
              }}
            >
              {/* Icon */}
              <div style={{
                width: 40,
                height: 40,
                borderRadius: 8,
                background: selected ? `${color}18` : `${color}0f`,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                flexShrink: 0,
              }}>
                <ChartIcon id={tmpl.id} color={color} />
              </div>

              {/* Name */}
              <span style={{
                fontSize: 11,
                fontWeight: selected ? 600 : 500,
                color: selected ? color : "var(--text-secondary)",
                textAlign: "center",
                lineHeight: 1.3,
                fontFamily: 'Inter, -apple-system, sans-serif',
                letterSpacing: "-0.01em",
              }}>
                {tmpl.name}
              </span>

              {/* Selected checkmark */}
              {selected && (
                <div style={{
                  position: "absolute",
                  top: 5,
                  right: 6,
                  width: 14,
                  height: 14,
                  borderRadius: "50%",
                  background: color,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                }}>
                  <svg viewBox="0 0 10 10" width="8" height="8">
                    <polyline points="1.5,5 4,7.5 8.5,2.5" fill="none" stroke="var(--bg-surface)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                  </svg>
                </div>
              )}
            </button>
          );
        })}
          </div>
        </div>
      )}
    </div>
  );
};

export default ChartTypePicker;
