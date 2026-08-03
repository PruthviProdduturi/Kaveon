import React, { useRef, useCallback, useEffect } from "react";
import dynamic from "next/dynamic";
import { useChartBuilder } from "./ChartBuilderContext";
import { getPlugin } from "./chartPluginRegistry";
import { normaliseCountryName } from "../../utils/countryAliases";

const WorldMapGlobe = dynamic(() => import("./WorldMapGlobe"), { ssr: false });

const ReactECharts = dynamic(() => import("echarts-for-react"), { ssr: false });

const formatDuration = (ms: number): string => {
  if (!Number.isFinite(ms) || ms <= 0) return "";

  const totalSeconds = ms / 1000;
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;

  const minutesStr = String(minutes).padStart(2, "0");
  const secondsStr = seconds.toFixed(2).padStart(5, "0");

  if (hours > 0) {
    const hoursStr = String(hours).padStart(2, "0");
    return `${hoursStr}:${minutesStr}:${secondsStr}`;
  }

  return `${minutesStr}:${secondsStr}`;
};

interface BigNumberKpiCardProps {
  options: any;
  rows: (string | number | null)[][];
  columns: string[];
  chartType?: string;
}

// Inline sparkline SVG for big_number_trend with hover tooltip
const SparkLine: React.FC<{ values: number[]; color: string; rowLabels?: string[]; formatValue?: (v: number) => string; fill?: boolean }> = ({ values, color, rowLabels, formatValue, fill }) => {
  const [hoverIdx, setHoverIdx] = React.useState<number | null>(null);
  if (values.length < 2) return null;
  const W = 240, H = fill ? 120 : 56, PAD = fill ? 0 : 4;
  const min = Math.min(...values);
  const max = Math.max(...values);
  const range = max - min || 1;
  const toX = (i: number) => PAD + (i / (values.length - 1)) * (W - PAD * 2);
  const toY = (v: number) => PAD + (1 - (v - min) / range) * (H - PAD * 2);
  const linePts = values.map((v, i) => `${toX(i)},${toY(v)}`).join(' ');
  const fillPts = `${toX(0)},${H - PAD} ${linePts} ${toX(values.length - 1)},${H - PAD}`;
  const lastX = toX(values.length - 1);
  const lastY = toY(values[values.length - 1]);

  const hov = hoverIdx !== null ? hoverIdx : null;

  const gradId = `spark-grad-${fill ? 'f' : 's'}`;
  return (
    <div style={{ position: 'relative', display: fill ? 'block' : 'inline-block', width: fill ? '100%' : undefined, height: fill ? '100%' : undefined }}>
      {/* Hover tooltip */}
      {hov !== null && !fill && (
        <div style={{
          position: 'absolute',
          left: toX(hov) - 30,
          top: toY(values[hov]) - 34,
          background: '#1e293b',
          color: '#fff',
          fontSize: 11,
          fontWeight: 600,
          padding: '3px 8px',
          borderRadius: 5,
          whiteSpace: 'nowrap',
          pointerEvents: 'none',
          zIndex: 10,
          fontFamily: 'Inter, -apple-system, sans-serif',
        }}>
          {rowLabels?.[hov] ? `${rowLabels[hov]}: ` : ''}{formatValue ? formatValue(values[hov]) : values[hov].toLocaleString(undefined, { maximumFractionDigits: 2 })}
        </div>
      )}
      <svg
        width={fill ? '100%' : W} height={fill ? '100%' : H}
        viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none"
        style={{ overflow: 'visible', display: 'block', cursor: 'crosshair' }}
      >
        <defs>
          <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={color} stopOpacity={fill ? 0.35 : 0.22} />
            <stop offset="100%" stopColor={color} stopOpacity="0.02" />
          </linearGradient>
        </defs>
        <polygon points={fillPts} fill={`url(#${gradId})`} />
        <polyline points={linePts} fill="none" stroke={color} strokeWidth={fill ? 2 : 2.2} vectorEffect="non-scaling-stroke" strokeLinecap="round" strokeLinejoin="round" />
        {!fill && <circle cx={lastX} cy={lastY} r="3.5" fill={color} />}
        {/* Hover dot */}
        {hov !== null && (
          <circle cx={toX(hov)} cy={toY(values[hov])} r="4.5" fill={color} stroke="#fff" strokeWidth="1.5" />
        )}
        {/* Invisible hover zones for each data point */}
        {values.map((_, i) => (
          <rect
            key={i}
            x={toX(i) - (W - PAD * 2) / (values.length - 1) / 2}
            y={0}
            width={(W - PAD * 2) / (values.length - 1)}
            height={H}
            fill="transparent"
            onMouseEnter={() => setHoverIdx(i)}
            onMouseLeave={() => setHoverIdx(null)}
          />
        ))}
      </svg>
    </div>
  );
};

const BigNumberKpiCard: React.FC<BigNumberKpiCardProps> = ({ options, rows, columns, chartType }) => {
  const lastRow = rows[rows.length - 1];
  const kpiOpts = options?.kpiOptions || {};
  const showTrend = chartType === "big_number_trend";

  // Find the metric column (last numeric column)
  let value: number | null = null;
  let label = "";
  let metricIdx = -1;

  if (lastRow) {
    for (let i = columns.length - 1; i >= 0; i--) {
      const v = lastRow[i];
      if (v !== null && v !== undefined && !isNaN(Number(v))) {
        value = Number(v);
        label = columns[i] || "";
        metricIdx = i;
        break;
      }
    }
  }

  // % change vs prior period (last two rows)
  let trend: number | null = null;
  if (rows.length >= 2 && metricIdx >= 0) {
    const prevRow = rows[rows.length - 2];
    const prev = Number(prevRow[metricIdx]);
    if (!isNaN(prev) && prev !== 0 && value !== null) {
      trend = ((value - prev) / Math.abs(prev)) * 100;
    }
  }

  // Sparkline values + labels (time/category column for tooltip)
  const timeOrCatIdx = columns.findIndex((c, i) => i !== metricIdx && (c.toLowerCase().includes('date') || c.toLowerCase().includes('time') || c.toLowerCase().includes('month') || c.toLowerCase().includes('week') || c.toLowerCase().includes('year') || c.toLowerCase().includes('period')));
  const sparkValues: number[] = showTrend && metricIdx >= 0
    ? rows.map(r => Number(r[metricIdx])).filter(v => !isNaN(v))
    : [];
  const sparkLabels: string[] = showTrend && timeOrCatIdx >= 0
    ? rows.map(r => String(r[timeOrCatIdx] ?? ''))
    : [];

  const numFmt = kpiOpts.numberFormat || "auto";
  const prefix = kpiOpts.prefix || "";
  const suffix = kpiOpts.suffix || "";

  const formatKpiValue = (v: number): string => {
    if (numFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
    if (numFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
    if (numFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
    if (numFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
    if (numFmt === "fixed") return v.toFixed(kpiOpts.decimalPlaces ?? 2);
    if (Math.abs(v) >= 1e9) return `${(v / 1e9).toFixed(1)}B`;
    if (Math.abs(v) >= 1e6) return `${(v / 1e6).toFixed(1)}M`;
    if (Math.abs(v) >= 1e3) return `${(v / 1e3).toFixed(1)}K`;
    return v.toLocaleString(undefined, { maximumFractionDigits: 2 });
  };

  const formatted = value === null ? "—" : `${prefix}${formatKpiValue(value)}${suffix}`;
  const trendColor = trend === null ? "#6b7280" : trend >= 0 ? "#16a34a" : "#dc2626";
  const trendIcon = trend === null ? null : trend >= 0 ? "▲" : "▼";
  // Custom label overrides auto-detected column name
  const displayLabel = kpiOpts.labelText || label;

  const hasSpark = showTrend && sparkValues.length >= 2;

  return (
    <div style={{ position: "relative", height: "100%", width: "100%", overflow: "hidden", display: "flex", flexDirection: "column" }}>
      {/* Number + label — own zone, never overlapping the trend. Uses clamp()
          so the value scales with the card and fills the available space. */}
      <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 6, padding: hasSpark ? "10px 20px 2px" : "12px 20px" }}>
        {options?.title?.text && (
          <div style={{ fontSize: 13, color: "#6b7280", fontWeight: 500, textAlign: "center" }}>{options.title.text}</div>
        )}
        <div style={{ fontSize: hasSpark ? "clamp(30px, 4.6vw, 56px)" : "clamp(36px, 5.4vw, 68px)", fontWeight: 800, color: "#0f172a", lineHeight: 1, letterSpacing: "-2px", whiteSpace: "nowrap", maxWidth: "100%" }}>{formatted}</div>
        {displayLabel && <div style={{ fontSize: 13, color: "#6b7280", textTransform: "uppercase", letterSpacing: "0.08em", fontWeight: 600, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", maxWidth: "100%" }}>{displayLabel}</div>}
        {trend !== null && (
          <div style={{ fontSize: 13, color: trendColor, fontWeight: 600, display: "flex", alignItems: "center", gap: 4 }}>
            <span>{trendIcon}</span>
            <span>{Math.abs(trend).toFixed(1)}% vs prior period</span>
          </div>
        )}
      </div>
      {/* Trend band — separate zone at the bottom, no overlap with the text */}
      {hasSpark && (
        <div style={{ height: "42%", minHeight: 54, flexShrink: 0, pointerEvents: "none" }}>
          <SparkLine values={sparkValues} color={trendColor} fill />
        </div>
      )}
    </div>
  );
};

interface TableRendererProps {
  rows: (string | number | null)[][];
  columns: string[];
  isPivot?: boolean;
  advancedOptions?: any;
}

// Interpolate between two hex colors by ratio 0–1
function lerpColor(a: string, b: string, t: number): string {
  const parse = (h: string) => [parseInt(h.slice(1,3),16), parseInt(h.slice(3,5),16), parseInt(h.slice(5,7),16)];
  const [ar,ag,ab] = parse(a);
  const [br,bg,bb] = parse(b);
  const r = Math.round(ar + (br-ar)*t);
  const g = Math.round(ag + (bg-ag)*t);
  const bl = Math.round(ab + (bb-ab)*t);
  return `rgb(${r},${g},${bl})`;
}

const TableRenderer: React.FC<TableRendererProps> = ({ rows, columns, isPivot, advancedOptions }) => {
  const [sortCol, setSortCol] = React.useState<number | null>(null);
  const [sortDir, setSortDir] = React.useState<"asc" | "desc">("asc");
  const [search, setSearch] = React.useState("");
  const [page, setPage] = React.useState(0);
  const ctOpts = advancedOptions?.chartTypeOptions || {};
  const pageSize: number = ctOpts.tablePageSize ?? 25;
  const condFmt: boolean = ctOpts.tableCondFmt ?? false;
  const condFmtLow: string = ctOpts.tableCondFmtLow ?? "#fee2e2";
  const condFmtHigh: string = ctOpts.tableCondFmtHigh ?? "#dcfce7";

  const handleSort = (idx: number) => {
    if (sortCol === idx) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortCol(idx);
      setSortDir("asc");
      setPage(0);
    }
  };

  // Pivot transform
  let baseRows = rows;
  let baseCols = columns;
  if (isPivot && columns.length >= 3) {
    const dimCol = columns[0];
    const pivotCats = [...new Set(rows.map((r) => String(r[1] ?? "")))];
    const pivotDims = [...new Set(rows.map((r) => String(r[0] ?? "")))];
    const pivotMap = new Map<string, Map<string, string | number | null>>();
    rows.forEach((r) => {
      const dim = String(r[0] ?? "");
      const cat = String(r[1] ?? "");
      if (!pivotMap.has(dim)) pivotMap.set(dim, new Map());
      pivotMap.get(dim)!.set(cat, r[2]);
    });
    baseCols = [dimCol, ...pivotCats];
    baseRows = pivotDims.map((dim) => {
      const row: (string | number | null)[] = [dim];
      pivotCats.forEach((cat) => row.push(pivotMap.get(dim)?.get(cat) ?? null));
      return row;
    });
  }

  // Search filter
  const searchLower = search.toLowerCase();
  const filtered = searchLower
    ? baseRows.filter(r => r.some(c => String(c ?? "").toLowerCase().includes(searchLower)))
    : baseRows;

  // Sort
  let sorted = [...filtered];
  if (sortCol !== null) {
    sorted.sort((a, b) => {
      const av = a[sortCol], bv = b[sortCol];
      const an = Number(av), bn = Number(bv);
      const cmp = !isNaN(an) && !isNaN(bn) ? an - bn : String(av ?? "").localeCompare(String(bv ?? ""));
      return sortDir === "asc" ? cmp : -cmp;
    });
  }

  // Pagination
  const totalPages = Math.max(1, Math.ceil(sorted.length / pageSize));
  const safePage = Math.min(page, totalPages - 1);
  const pageRows = sorted.slice(safePage * pageSize, (safePage + 1) * pageSize);

  // Conditional formatting: per-column min/max for numeric columns
  const colStats = React.useMemo(() => {
    return baseCols.map((_, ci) => {
      const vals = baseRows.map(r => Number(r[ci])).filter(v => !isNaN(v));
      if (!vals.length) return null;
      return { min: Math.min(...vals), max: Math.max(...vals) };
    });
  }, [baseRows, baseCols]);

  const isNumeric = (v: string | number | null) => v !== null && v !== "" && !isNaN(Number(v));

  return (
    <div style={{ display: "flex", flexDirection: "column", width: "100%", height: "100%" }}>
      {/* Search bar */}
      <div style={{ padding: "6px 8px", borderBottom: "1px solid #e2e8f0", flexShrink: 0 }}>
        <input
          type="text"
          placeholder="Search…"
          value={search}
          onChange={e => { setSearch(e.target.value); setPage(0); }}
          style={{
            width: "100%", boxSizing: "border-box",
            padding: "4px 8px", fontSize: 12,
            border: "1px solid #e2e8f0", borderRadius: 5,
            outline: "none", color: "#374151",
            fontFamily: "Inter, -apple-system, sans-serif",
          }}
        />
      </div>

      {/* Table */}
      <div style={{ flex: 1, overflow: "auto" }}>
        <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 12, tableLayout: "auto" }}>
          <thead>
            <tr>
              {baseCols.map((col, i) => (
                <th
                  key={i}
                  onClick={() => handleSort(i)}
                  style={{
                    padding: "6px 10px",
                    background: "#f8fafc",
                    borderBottom: "2px solid #e2e8f0",
                    textAlign: "left",
                    fontWeight: 600,
                    color: "#374151",
                    whiteSpace: "nowrap",
                    cursor: "pointer",
                    userSelect: "none",
                    position: "sticky",
                    top: 0,
                    zIndex: 1,
                  }}
                >
                  {col.split(".").pop() ?? col}
                  {sortCol === i ? (sortDir === "asc" ? " ▲" : " ▼") : ""}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {pageRows.map((row, ri) => (
              <tr key={ri} style={{ background: ri % 2 === 0 ? "#fff" : "#f9fafb" }}>
                {row.map((cell, ci) => {
                  const numeric = isNumeric(cell);
                  let bg: string | undefined;
                  if (condFmt && numeric && colStats[ci]) {
                    const { min, max } = colStats[ci]!;
                    const t = max === min ? 0.5 : (Number(cell) - min) / (max - min);
                    bg = lerpColor(condFmtLow, condFmtHigh, t);
                  }
                  return (
                    <td
                      key={ci}
                      style={{
                        padding: "5px 10px",
                        borderBottom: "1px solid #e2e8f0",
                        textAlign: numeric ? "right" : "left",
                        color: "#1f2937",
                        whiteSpace: "nowrap",
                        fontVariantNumeric: "tabular-nums",
                        background: bg,
                      }}
                    >
                      {cell === null || cell === undefined ? (
                        <span style={{ color: "#9ca3af" }}>null</span>
                      ) : numeric ? (
                        Number(cell).toLocaleString()
                      ) : (
                        String(cell)
                      )}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Pagination */}
      <div style={{
        display: "flex", alignItems: "center", justifyContent: "space-between",
        padding: "5px 10px", borderTop: "1px solid #e2e8f0", flexShrink: 0,
        fontSize: 11, color: "#64748b",
      }}>
        <span>{filtered.length} row{filtered.length !== 1 ? "s" : ""}{search ? ` (filtered from ${baseRows.length})` : ""}</span>
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <button
            onClick={() => setPage(p => Math.max(0, p - 1))}
            disabled={safePage === 0}
            style={{ padding: "2px 7px", borderRadius: 4, border: "1px solid #e2e8f0", background: "#fff", cursor: safePage === 0 ? "default" : "pointer", opacity: safePage === 0 ? 0.4 : 1 }}
          >‹</button>
          <span>Page {safePage + 1} of {totalPages}</span>
          <button
            onClick={() => setPage(p => Math.min(totalPages - 1, p + 1))}
            disabled={safePage >= totalPages - 1}
            style={{ padding: "2px 7px", borderRadius: 4, border: "1px solid #e2e8f0", background: "#fff", cursor: safePage >= totalPages - 1 ? "default" : "pointer", opacity: safePage >= totalPages - 1 ? 0.4 : 1 }}
          >›</button>
        </div>
      </div>
    </div>
  );
};

// Module-level GeoJSON cache so we only fetch once per page load
let worldGeoJsonCache: any = null;
let worldGeoJsonFetching: Promise<any> | null = null;

// Vivid "turbo"-style heat ramp so choropleths read as colourful, not all-blue.
const MAP_DEFAULT_COLORS = ["#3b82f6", "#22d3ee", "#10b981", "#facc15", "#f97316", "#ef4444"];

const WorldMapRenderer: React.FC<{
  rows: (string | number | null)[][];
  columns: string[];
  advancedOptions?: any;
  onCrossFilter?: (value: string) => void;
}> = ({ rows, columns, advancedOptions, onCrossFilter }) => {
  const [ready, setReady] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const eRef = React.useRef<any>(null);

  const ctOpts = advancedOptions?.chartTypeOptions || {};
  const isGlobe = ctOpts.mapStyle === "globe";

  React.useEffect(() => {
    setReady(false);
    const load = async () => {
      try {
        if (!worldGeoJsonCache) {
          if (!worldGeoJsonFetching) {
            worldGeoJsonFetching = fetch("/geo/world.json")
              .then(r => r.json())
              .then(data => {
                // echarts-gl 2.0.9 has several null-safety bugs in its mesh builder:
                // 1. Crashes on non-Polygon/MultiPolygon geometry types
                // 2. Crashes if any coordinate ring contains null/undefined points
                //    (dataToPoint(undefined) → undefined[0] crash)
                // Sanitise strictly: only keep features with fully-valid coordinate rings.
                const isValidPt = (pt: any) =>
                  Array.isArray(pt) && pt.length >= 2
                  && typeof pt[0] === "number" && isFinite(pt[0])
                  && typeof pt[1] === "number" && isFinite(pt[1]);
                const isValidRing = (ring: any) =>
                  Array.isArray(ring) && ring.length >= 4 && ring.every(isValidPt);
                const isValidPolygon = (coords: any) =>
                  Array.isArray(coords) && coords.length > 0 && coords.every(isValidRing);
                const isValidMultiPolygon = (coords: any) =>
                  Array.isArray(coords) && coords.length > 0 && coords.every(isValidPolygon);

                worldGeoJsonCache = {
                  ...data,
                  features: (data.features ?? []).filter((f: any) => {
                    const g = f?.geometry;
                    if (!g) return false;
                    if (g.type === "Polygon") return isValidPolygon(g.coordinates);
                    if (g.type === "MultiPolygon") return isValidMultiPolygon(g.coordinates);
                    return false;
                  }),
                };
                return worldGeoJsonCache;
              });
          }
          await worldGeoJsonFetching;
        }
        if (!isGlobe) {
          // Flat map: register map on the main echarts instance
          const echarts = await import("echarts");
          echarts.registerMap("world", worldGeoJsonCache);
        }
        setReady(true);
      } catch (e) {
        setError("Failed to load world map data");
      }
    };
    load();
  }, [isGlobe]);

  const option = React.useMemo(() => {
    if (!ready || !rows.length || columns.length < 2) return null;

    const colorScheme: string[] = advancedOptions?.color?.length ? advancedOptions.color : MAP_DEFAULT_COLORS;
    const showRoam: boolean = ctOpts.mapRoam !== false;
    const showLabels: boolean = ctOpts.mapShowLabels === true;
    const numFmt: string = ctOpts.mapNumberFormat || "none";

    const fmtVal = (v: number): string => {
      if (numFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
      if (numFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
      if (numFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
      if (numFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
      return v.toLocaleString(undefined, { maximumFractionDigits: 2 });
    };

    // Heuristic: first non-numeric col = country name, last numeric col = value
    const nameIdx = columns.findIndex((_, i) => rows.some(r => isNaN(Number(r[i]))));
    const valIdx = columns.length - 1 === nameIdx ? columns.length - 2 : columns.length - 1;
    if (nameIdx < 0 || valIdx < 0) return null;

    const data = rows
      .map(r => ({ name: normaliseCountryName(String(r[nameIdx] ?? "")), value: Number(r[valIdx] ?? 0) }))
      .filter(d => d.name && !isNaN(d.value));

    const values = data.map(d => d.value);
    const minV = Math.min(...values);
    const maxV = Math.max(...values);
    // Heavily-skewed data (e.g. US dwarfs everyone) makes a linear scale all one
    // colour. Cap the colour range at the ~85th percentile so mid countries get
    // vivid hues; countries above clamp to the hottest colour.
    // Build log-decade tiers (…, 100K–1M, 1M–10M, 10M+). A linear scale on this
    // hugely-skewed data paints almost everything one colour AND a percentile cap
    // over-paints the top tier — both mislead. Tiered log buckets are colourful
    // *and* truthful: each order of magnitude gets its own hue.
    const positive = values.filter((v) => v > 0);
    const hiV = positive.length ? Math.max(...positive) : 1;
    const loV = positive.length ? Math.min(...positive) : 1;
    let edges: number[] = [];
    const startExp = Math.max(0, Math.floor(Math.log10(Math.max(loV, 1))));
    const endExp = Math.ceil(Math.log10(Math.max(hiV, 10)));
    for (let e = startExp; e <= endExp; e++) edges.push(Math.pow(10, e));
    while (edges.length > 7) edges = edges.filter((_, idx) => idx % 2 === 0 || idx === edges.length - 1);
    const nPieces = Math.max(1, edges.length - 1);
    const sampleColor = (k: number) => {
      const scheme = colorScheme.length >= 2 ? colorScheme : MAP_DEFAULT_COLORS;
      const pos = nPieces === 1 ? 0 : (k / (nPieces - 1)) * (scheme.length - 1);
      const lo = Math.floor(pos), hi = Math.min(scheme.length - 1, lo + 1);
      return lerpColor(scheme[lo], scheme[hi], pos - lo);
    };
    const pieces = Array.from({ length: nPieces }, (_, k) => {
      const lo = edges[k];
      const hi = k === nPieces - 1 ? undefined : edges[k + 1];
      return {
        gte: lo, ...(hi !== undefined ? { lt: hi } : {}),
        color: sampleColor(k),
        label: hi !== undefined ? `${fmtVal(lo)} – ${fmtVal(hi)}` : `${fmtVal(lo)}+`,
      };
    }).reverse(); // largest tier on top of the legend

    // Data labels for the top ~12 countries only (labelling all ~196 is noise).
    const topN = ctOpts.mapLabelTopN ?? 12;
    const labelThreshold = [...values].sort((a, b) => b - a)[Math.min(topN - 1, values.length - 1)] ?? Infinity;
    const dataLabel = {
      show: true,
      fontSize: 9,
      color: "#0f172a",
      formatter: (p: any) => {
        const v = Number(p.value);
        return !isNaN(v) && v >= labelThreshold ? `${p.name}\n${fmtVal(v)}` : "";
      },
    };

    const titleOpt = advancedOptions?.title?.text
      ? { title: { text: advancedOptions.title.text, left: "center", top: 5, textStyle: { fontSize: Number(advancedOptions.titleSize) || 20, fontFamily: advancedOptions.titleFont || "sans-serif" } } }
      : {};

    // Piecewise (tiered) legend — accurate for skewed data and reads cleanly.
    const visualMap = {
      type: "piecewise",
      pieces,
      left: "left", bottom: 20,
      itemWidth: 14, itemHeight: 12, itemGap: 4,
      textStyle: { fontSize: 11, color: isGlobe ? "#94a3b8" : "#475569" },
    };

    const tooltipStyle = {
      trigger: "item",
      appendToBody: true,
      backgroundColor: "rgba(255,255,255,0.97)",
      borderColor: "#e2e8f0",
      borderWidth: 1,
      padding: [10, 14],
      textStyle: { color: "#1e293b", fontSize: 12, fontFamily: 'Inter, -apple-system, sans-serif' },
      extraCssText: "box-shadow: 0 8px 24px rgba(0,0,0,0.12); border-radius: 8px;",
      formatter: (p: any) => {
        const v = Number(p.value);
        return `<b>${p.name}</b><br/>${p.value != null && !isNaN(v) ? fmtVal(v) : "—"}`;
      },
    };

    if (isGlobe) {
      return {
        ...titleOpt,
        backgroundColor: "#0d1117",
        tooltip: tooltipStyle,
        visualMap,
        globe: {
          baseTexture: "#0c1e35",
          shading: "lambert",
          light: {
            ambient: { intensity: 0.6 },
            main: { intensity: 1.2, shadow: false },
          },
          atmosphere: {
            show: true,
          },
          viewControl: {
            autoRotate: false,
            distance: 160,
            minDistance: 80,
            maxDistance: 320,
            rotateSensitivity: 1,
            zoomSensitivity: 1,
            panSensitivity: 0,
          },
          itemStyle: {
            borderWidth: 0,
          },
          regionQuery: {},
        },
        series: [{
          type: "map3D",
          coordinateSystem: "globe",
          map: "world",
          data,
          shading: "lambert",
          emphasis: {
            label: { show: true, textStyle: { color: "#fff", fontSize: 11, fontFamily: 'Inter, sans-serif' } },
            itemStyle: { color: "#fbbf24" },
          },
          itemStyle: {
            borderWidth: 0.4,
            borderColor: "rgba(255,255,255,0.15)",
          },
          label: {
            show: showLabels,
            textStyle: { color: "#fff", fontSize: 9, fontFamily: 'Inter, sans-serif' },
          },
        }],
      };
    }

    // Flat map — layoutCenter/layoutSize make the map fill wide, short tiles
    // instead of collapsing to a tiny centred thumbnail.
    return {
      ...titleOpt,
      tooltip: tooltipStyle,
      visualMap: { ...visualMap, left: 8, bottom: 14, itemWidth: 11, itemHeight: 90 },
      series: [{
        type: "map",
        map: "world",
        roam: showRoam,
        layoutCenter: ["50%", "52%"],
        layoutSize: "155%",
        aspectScale: 0.9,
        data,
        select: { itemStyle: { areaColor: "#f59e0b" } },
        emphasis: { label: { show: true, fontSize: 11, color: "#0f172a", fontWeight: "bold" }, itemStyle: { areaColor: "#fbbf24" } },
        itemStyle: { borderColor: "#fff", borderWidth: 0.5, areaColor: "#eef2f7" },
        labelLayout: { hideOverlap: true },
        label: showLabels ? { show: true, fontSize: 10, color: "#1e293b" } : dataLabel,
      }],
    };
  }, [ready, rows, columns, advancedOptions, isGlobe]);

  if (error) return <div style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100%", color: "#ef4444", fontSize: 13 }}>{error}</div>;
  if (!ready) return <div style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100%", color: "#94a3b8", fontSize: 13 }}>Loading map…</div>;

  // Globe: delegate to the separate chunk that statically imports echarts-gl.
  // Wrap in a positioned container so the globe's inset:0 has an anchor —
  // the parent flex container (.chart-builder-preview-inner) uses
  // align-items:center which breaks height:100% on direct children.
  if (isGlobe) {
    return (
      <div style={{ position: "relative", width: "100%", height: "100%", flex: 1, alignSelf: "stretch" }}>
        <WorldMapGlobe
          rows={rows}
          columns={columns}
          geoJson={worldGeoJsonCache}
          advancedOptions={advancedOptions}
        />
      </div>
    );
  }

  if (!option) return null;

  // Wrap in a stretch container: the parent .chart-builder-preview-inner uses
  // align-items:center, which otherwise collapses a height:100% ECharts child to
  // near-zero (making the map a tiny thumbnail).
  return (
    <div style={{ position: "relative", width: "100%", height: "100%", flex: 1, alignSelf: "stretch", minHeight: 240 }}>
      <ReactECharts
        option={option}
        style={{ width: "100%", height: "100%", cursor: onCrossFilter ? "pointer" : undefined }}
        onChartReady={(instance: any) => { eRef.current = instance; }}
        onEvents={onCrossFilter ? {
          click: (params: any) => {
            const val = params?.name || (Array.isArray(params?.value) ? String(params.value[0]) : "");
            if (val) onCrossFilter(String(val));
          },
        } : undefined}
      />
    </div>
  );
};

interface ChartPreviewProps {
  /** Called when user clicks a data point in dashboard view — enables cross-filtering */
  onCrossFilter?: (value: string) => void;
  /** Called after downloadPng/downloadCsv are defined, so parent can invoke them */
  onRegisterExports?: (exports: { downloadPng: () => void; downloadCsv: () => void }) => void;
}

const ChartPreview: React.FC<ChartPreviewProps> = ({ onCrossFilter, onRegisterExports }) => {
  const { selectedTemplate, selectedDatasetId, previewOptions, sqlPreview, description, chartType, advancedOptions, cancelRunningQuery, runContext } = useChartBuilder();
  const echartsInstanceRef = useRef<any>(null);

  // ── Download helpers ─────────────────────────────────────────────────────────
  const downloadPng = useCallback(() => {
    if (!echartsInstanceRef.current) return;
    const url = echartsInstanceRef.current.getDataURL({ type: 'png', pixelRatio: 2, backgroundColor: '#fff' });
    const a = document.createElement('a');
    a.href = url;
    a.download = 'chart.png';
    a.click();
  }, []);

  const downloadCsv = useCallback(() => {
    const cols = sqlPreview.dataColumns;
    const rows = sqlPreview.dataRows;
    if (!cols?.length || !rows?.length) return;
    const escape = (v: any) => {
      const s = v === null || v === undefined ? '' : String(v);
      return s.includes(',') || s.includes('"') || s.includes('\n') ? `"${s.replace(/"/g, '""')}"` : s;
    };
    const csv = [cols.map(escape).join(','), ...rows.map(r => r.map(escape).join(','))].join('\n');
    const blob = new Blob([csv], { type: 'text/csv' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'chart.csv';
    a.click();
    URL.revokeObjectURL(url);
  }, [sqlPreview.dataColumns, sqlPreview.dataRows]);

  useEffect(() => {
    onRegisterExports?.({ downloadPng, downloadCsv });
  }, [onRegisterExports, downloadPng, downloadCsv]);

  const hasOption = Boolean(previewOptions);

  const subtitle = (() => {
    if (!selectedDatasetId) return "Choose a dataset";
    return "";
  })();

  const runtimeLabel = (() => {
    if (!hasOption || sqlPreview.durationMs == null || sqlPreview.durationMs <= 0) return "";
    return formatDuration(sqlPreview.durationMs);
  })();

  // Map legend position values to ECharts positions
  const legendPosMap: Record<string, { left: string; top?: string }> = {
    top: { left: "center", top: "top" },
    topCenter: { left: "center", top: "top" },
    bottom: { left: "center", top: "bottom" },
    bottomCenter: { left: "center", top: "bottom" },
    left: { left: "left", top: "middle" },
    right: { left: "right", top: "middle" },
  };

  let chartOptions = previewOptions || {};
  // Always ensure chartOptions.title is an object with a text property
  let titleText = "";
  if (typeof chartOptions.title === "object" && chartOptions.title !== null && "text" in chartOptions.title) {
    titleText = chartOptions.title.text;
  } else if (typeof chartOptions.title === "string") {
    titleText = chartOptions.title;
  }
  chartOptions = {
    ...chartOptions,
    title: {
      text: titleText,
      left: "center",
      top: 5,
      textStyle: {
        fontSize: Number(chartOptions.titleSize) || 20,
        fontWeight: "bold",
        fontFamily: chartOptions.titleFont || "sans-serif",
      },
    },
  };
  // Set legend position and clean up non-standard properties
  if (chartOptions.legend) {
    const { order, ...legendProps } = chartOptions.legend;
    const pos = legendProps.left ? (legendPosMap[legendProps.left] || { left: "center", top: "top" }) : { left: "center", top: "top" };
    chartOptions = {
      ...chartOptions,
      legend: {
        ...legendProps,
        left: pos.left,
        top: pos.top,
        data: order && Array.isArray(order) && order.length > 0 ? order : legendProps.data,
        textStyle: {
          fontSize: 12,
          color: "#475569",
          fontFamily: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
          ...(legendProps.textStyle || {}),
        },
        itemGap: 16,
        itemWidth: 12,
        itemHeight: 12,
        icon: legendProps.icon || "roundRect",
      },
    };
  }

  // Prevent label overlapping - handle both single and array xAxis/yAxis
  if (chartOptions.xAxis) {
    if (Array.isArray(chartOptions.xAxis)) {
      chartOptions = {
        ...chartOptions,
        xAxis: chartOptions.xAxis.map((axis: any) => ({
          ...axis,
          axisLabel: {
            ...(axis.axisLabel || {}),
            hideOverlap: true,
            overflow: 'truncate',
            rotate: 0,
            margin: 12,
          },
        })),
      };
    } else {
      chartOptions = {
        ...chartOptions,
        xAxis: {
          ...chartOptions.xAxis,
          axisLabel: {
            ...(chartOptions.xAxis.axisLabel || {}),
            hideOverlap: true,
            overflow: 'truncate',
            rotate: 0,
            margin: 12,
          },
        },
      };
    }
  }

  if (chartOptions.yAxis) {
    if (Array.isArray(chartOptions.yAxis)) {
      chartOptions = {
        ...chartOptions,
        yAxis: chartOptions.yAxis.map((axis: any) => ({
          ...axis,
          axisLabel: {
            ...(axis.axisLabel || {}),
            hideOverlap: true,
            overflow: 'truncate',
            margin: 10,
          },
        })),
      };
    } else {
      chartOptions = {
        ...chartOptions,
        yAxis: {
          ...chartOptions.yAxis,
          axisLabel: {
            ...(chartOptions.yAxis.axisLabel || {}),
            hideOverlap: true,
            overflow: 'truncate',
            margin: 10,
          },
        },
      };
    }
  }

  // ── Global aesthetics ────────────────────────────────────────────────────────
  // Tooltip — polished card style applied to every chart type
  if (chartOptions.tooltip) {
    const tooltipBase = {
      appendToBody: true,
      confine: true,
      backgroundColor: "rgba(255, 255, 255, 0.97)",
      borderColor: "#e2e8f0",
      borderWidth: 1,
      padding: [10, 14],
      textStyle: {
        color: "#1e293b",
        fontSize: 12,
        lineHeight: 20,
        fontFamily: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
      },
      extraCssText:
        "box-shadow: 0 8px 24px rgba(0,0,0,0.12); border-radius: 8px; pointer-events: none; z-index: 99999;",
    };
    if (Array.isArray(chartOptions.tooltip)) {
      chartOptions = {
        ...chartOptions,
        tooltip: chartOptions.tooltip.map((t: any) => ({ ...tooltipBase, ...t })),
      };
    } else {
      chartOptions = {
        ...chartOptions,
        tooltip: { ...tooltipBase, ...chartOptions.tooltip },
      };
    }
  }

  // Data labels — professional typography with position-aware contrast
  const labelStep: number = (advancedOptions as any)?.chartTypeOptions?.labelStep ?? 1;
  const labelNumFmt: string = (advancedOptions as any)?.chartTypeOptions?.labelNumberFormat || "none";
  const isShareChart = chartType?.endsWith("_share") || false;
  const fmtLabelNum = (v: number): string => {
    if (isShareChart) return `${v.toFixed(1)}%`;
    if (labelNumFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
    if (labelNumFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
    if (labelNumFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
    if (labelNumFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
    return v.toLocaleString();
  };

  // chartTypeOptions flags are the authoritative source for line chart labels/markers.
  // These are always correctly set via the checkbox handlers regardless of what
  // happens to previewOptions.series during state updates.
  const ctOpts = (advancedOptions as any)?.chartTypeOptions || {};
  const ctShowDataLabels: boolean | undefined = ctOpts.showDataLabels;
  const ctShowMarkers: boolean | undefined = ctOpts.showMarkers;

  if (chartOptions.series && Array.isArray(chartOptions.series)) {
    chartOptions = {
      ...chartOptions,
      series: chartOptions.series.map((series: any) => {
        const isLineSeries = series.type === 'line';

        // For line series, chartTypeOptions.showDataLabels is authoritative.
        // For other series, fall back to series.label?.show.
        const labelShouldShow = isLineSeries && ctShowDataLabels !== undefined
          ? ctShowDataLabels === true
          : Boolean(series.label?.show);

        if (!labelShouldShow) {
          return {
            ...series,
            label: { ...(series.label || {}), show: false },
          };
        }

        const pos = (series.label?.position) || "top";
        const isInside =
          pos === "inside" || pos === "insideTop" || pos === "insideBottom" ||
          pos === "insideLeft" || pos === "insideRight" || pos === "insideTopLeft" ||
          pos === "insideTopRight" || pos === "insideBottomLeft" || pos === "insideBottomRight";

        const font = 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
        const existingFormatter = series.label?.formatter;

        // Shared pill style — same values whether applied directly or via rich text
        const pillStyle = {
          backgroundColor: "rgba(255,255,255,0.92)",
          borderRadius: 4,
          padding: [3, 6] as [number, number],
          borderColor: "#e2e8f0",
          borderWidth: 1,
          color: "#334155",
          fontSize: 11,
          fontWeight: "600",
          fontFamily: font,
        };

        const styledLabel: any = {
          ...(series.label || {}),
          show: true,  // always true — we only reach here when label should show
          position: pos,
          fontSize: 11,
          fontWeight: isInside ? "700" : "600",
          fontFamily: font,
          color: series.label?.color || (isInside ? "#ffffff" : pillStyle.color),
          textShadowColor: isInside ? "rgba(0,0,0,0.45)" : "rgba(255,255,255,0.9)",
          textShadowBlur: isInside ? 4 : 3,
          ...(!isInside && { ...pillStyle }),
        };

        // For line series, always set a clean formatter driven by labelStep.
        // The original series formatter is marker-position-based (returns "" at most points)
        // and must not leak through — we fully own the formatting here.
        if (isLineSeries) {
          if (labelStep > 1 && !isInside) {
            delete styledLabel.backgroundColor;
            delete styledLabel.borderRadius;
            delete styledLabel.padding;
            delete styledLabel.borderColor;
            delete styledLabel.borderWidth;
            styledLabel.rich = { pill: { ...pillStyle, padding: [3, 6] } };
            styledLabel.formatter = (params: any) => {
              if (params.dataIndex % labelStep !== 0) return "";
              return `{pill|${fmtLabelNum(Number(params.value ?? 0))}}`;
            };
          } else if (labelStep > 1) {
            styledLabel.formatter = (params: any) => {
              if (params.dataIndex % labelStep !== 0) return "";
              return fmtLabelNum(Number(params.value ?? 0));
            };
          } else {
            styledLabel.formatter = (params: any) => fmtLabelNum(Number(params.value ?? 0));
          }
        } else if (labelStep > 1 && !isInside && typeof existingFormatter !== "string") {
          // Non-line density + outside pill
          delete styledLabel.backgroundColor;
          delete styledLabel.borderRadius;
          delete styledLabel.padding;
          delete styledLabel.borderColor;
          delete styledLabel.borderWidth;
          styledLabel.rich = { pill: { ...pillStyle, padding: [3, 6] } };
          styledLabel.formatter = (params: any) => {
            if (params.dataIndex % labelStep !== 0) return "";
            const val = typeof existingFormatter === "function"
              ? String(existingFormatter(params))
              : fmtLabelNum(Number(params.value ?? 0));
            return `{pill|${val}}`;
          };
        } else if (labelStep > 1) {
          styledLabel.formatter = (params: any) => {
            if (params.dataIndex % labelStep !== 0) return "";
            if (typeof existingFormatter === "function") return existingFormatter(params);
            if (typeof existingFormatter === "string") return existingFormatter;
            return fmtLabelNum(Number(params.value ?? 0));
          };
        } else if (!styledLabel.formatter) {
          styledLabel.formatter = (params: any) => fmtLabelNum(Number(params.value ?? 0));
        }

        // For line series, ctShowMarkers is authoritative when set.
        // For other series, read from showSymbol state.
        const markersEnabled = isLineSeries && ctShowMarkers !== undefined
          ? ctShowMarkers === true
          : (series.showSymbol !== false && series.showSymbol !== undefined);

        // Sync marker dots to exactly the same positions as visible labels
        // (labelStep=1 → every point, labelStep=N → every Nth point).
        const showSymbolFinal = markersEnabled
          ? (dataIndex: number) => dataIndex % labelStep === 0
          : false;

        // ECharts cannot anchor labels when symbol is 'none' — use a hidden circle.
        const symbolForRender = series.symbol === 'none' ? 'circle' : series.symbol;

        return {
          ...series,
          symbol: symbolForRender,
          showSymbol: showSymbolFinal,
          labelLayout: { hideOverlap: true },
          label: styledLabel,
          emphasis: {
            ...(series.emphasis || {}),
            label: { show: false },
          },
        };
      }),
    };
  }

  // ── Reference lines (markLine) ───────────────────────────────────────────────
  const referenceLines: any[] = (advancedOptions as any)?.chartTypeOptions?.referenceLines || (advancedOptions as any)?.referenceLines || [];
  if (referenceLines.length > 0 && Array.isArray(chartOptions.series) && chartOptions.series.length > 0) {
    const markLineData = referenceLines
      .filter((rl: any) => rl.value !== '' && rl.value !== null && rl.value !== undefined)
      .map((rl: any) => ({
        name: rl.label || '',
        yAxis: Number(rl.value),
        lineStyle: {
          color: rl.color || '#ef4444',
          type: rl.style || 'dashed',
          width: 2,
        },
        label: {
          show: Boolean(rl.label),
          formatter: rl.label || '',
          position: 'end',
          color: rl.color || '#ef4444',
          fontSize: 11,
          fontWeight: 600,
        },
      }));

    if (markLineData.length > 0) {
      chartOptions = {
        ...chartOptions,
        series: chartOptions.series.map((s: any, i: number) => {
          if (i !== 0) return s; // Only attach to first series to avoid duplicate lines
          return {
            ...s,
            markLine: {
              silent: true,
              animation: false,
              symbol: ['none', 'none'],
              data: markLineData,
            },
          };
        }),
      };
    }
  }

  // Configure grid spacing to reduce top space and add bottom space
  if (!chartOptions.grid) {
    chartOptions = {
      ...chartOptions,
      grid: {
        left: '10%',
        right: '10%',
        top: titleText ? '12%' : '8%',
        bottom: '12%',
        containLabel: true,
      },
    };
  } else {
    chartOptions = {
      ...chartOptions,
      grid: {
        ...chartOptions.grid,
        top: chartOptions.grid.top || (titleText ? '12%' : '8%'),
        bottom: chartOptions.grid.bottom || '12%',
        containLabel: true,
      },
    };
  }

  const hasData = hasOption && !sqlPreview.isRunning && !!sqlPreview.dataRows?.length;

  return (
    <div
      className="chart-builder-preview-card"
    >
      <div className="chart-builder-preview-header">
        <div className="chart-builder-preview-meta">
          {subtitle && <div>{subtitle}</div>}
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          {description && description.trim() && (
            <div className="chart-info-icon-container">
              <i
                className="fas fa-info-circle chart-info-icon"
                title={description}
              />
              <div className="chart-info-tooltip">
                {description}
              </div>
            </div>
          )}
        </div>
      </div>
      <div
        className={
          hasOption && !sqlPreview.isRunning
            ? "chart-builder-preview-inner"
            : "chart-builder-preview-inner chart-builder-preview-inner-empty"
        }
      >
        {hasOption && !sqlPreview.isRunning && (chartType === "table" || chartType === "pivot_table") ? (
          <TableRenderer
            rows={(previewOptions as any)?._rows ?? sqlPreview.dataRows}
            columns={(previewOptions as any)?._columns ?? sqlPreview.dataColumns}
            isPivot={chartType === "pivot_table"}
            advancedOptions={advancedOptions}
          />
        ) : hasOption && !sqlPreview.isRunning && (chartType === "big_number" || chartType === "big_number_trend") ? (
          <BigNumberKpiCard options={chartOptions} rows={sqlPreview.dataRows} columns={sqlPreview.dataColumns} chartType={chartType} />
        ) : hasOption && !sqlPreview.isRunning && chartType === "world_map" ? (
          <WorldMapRenderer
            rows={(previewOptions as any)?._rows ?? sqlPreview.dataRows}
            columns={(previewOptions as any)?._columns ?? sqlPreview.dataColumns}
            advancedOptions={advancedOptions}
            onCrossFilter={onCrossFilter}
          />
        ) : hasOption && !sqlPreview.isRunning && chartType && getPlugin(chartType)?.Renderer ? (
          (() => {
            const PluginRenderer = getPlugin(chartType)!.Renderer!;
            return <PluginRenderer options={chartOptions} rows={sqlPreview.dataRows} columns={sqlPreview.dataColumns} />;
          })()
        ) : hasOption && !sqlPreview.isRunning ? (
          <ReactECharts
            option={chartOptions}
            style={{ width: "100%", height: "100%", cursor: onCrossFilter ? 'pointer' : undefined }}
            key={
              // Force re-render when xAxis date format changes
              (chartOptions.xAxis && chartOptions.xAxis.axisLabel && chartOptions.xAxis.axisLabel.formatter ? chartOptions.xAxisDateFormat || chartOptions.xAxis.axisLabel.formatter.toString() : 'default')
            }
            onChartReady={(instance: any) => { echartsInstanceRef.current = instance; }}
            onEvents={onCrossFilter ? {
              click: (params: any) => {
                const val = params.name || (Array.isArray(params.value) ? String(params.value[0]) : String(params.value ?? ''));
                if (val) onCrossFilter(val);
              },
            } : undefined}
          />
        ) : sqlPreview.isRunning ? (
          <div className="chart-preview-loading-state">
            <div className="chart-loading-animation">
              <div className="chart-loading-bars">
                <div className="chart-loading-bar" style={{ animationDelay: '0s' }}></div>
                <div className="chart-loading-bar" style={{ animationDelay: '0.1s' }}></div>
                <div className="chart-loading-bar" style={{ animationDelay: '0.2s' }}></div>
                <div className="chart-loading-bar" style={{ animationDelay: '0.3s' }}></div>
                <div className="chart-loading-bar" style={{ animationDelay: '0.4s' }}></div>
              </div>
              <div className="chart-loading-spinner"></div>
            </div>
            <div className="chart-loading-text">
              {(runContext || '').startsWith('dashboard') ? 'Querying…' : 'Rendering your chart...'}
            </div>
            {(runContext || '').startsWith('dashboard') && (
              <button
                onClick={cancelRunningQuery}
                style={{
                  marginTop: 10,
                  padding: '5px 14px',
                  background: 'transparent',
                  border: '1px solid #cbd5e1',
                  borderRadius: 6,
                  cursor: 'pointer',
                  fontSize: 12,
                  color: '#64748b',
                }}
              >
                Cancel
              </button>
            )}
          </div>
        ) : (
          <div className="chart-preview-empty-state">
            <svg className="chart-preview-placeholder-icon" width="120" height="120" viewBox="0 0 120 120" fill="none" xmlns="http://www.w3.org/2000/svg">
              <rect x="10" y="70" width="15" height="40" rx="2" fill="#cbd5e1" opacity="0.6">
                <animate attributeName="height" values="40;60;40" dur="1.5s" repeatCount="indefinite" />
                <animate attributeName="y" values="70;50;70" dur="1.5s" repeatCount="indefinite" />
              </rect>
              <rect x="35" y="50" width="15" height="60" rx="2" fill="#cbd5e1" opacity="0.7">
                <animate attributeName="height" values="60;80;60" dur="1.5s" begin="0.2s" repeatCount="indefinite" />
                <animate attributeName="y" values="50;30;50" dur="1.5s" begin="0.2s" repeatCount="indefinite" />
              </rect>
              <rect x="60" y="40" width="15" height="70" rx="2" fill="#94a3b8" opacity="0.8">
                <animate attributeName="height" values="70;90;70" dur="1.5s" begin="0.4s" repeatCount="indefinite" />
                <animate attributeName="y" values="40;20;40" dur="1.5s" begin="0.4s" repeatCount="indefinite" />
              </rect>
              <rect x="85" y="55" width="15" height="55" rx="2" fill="#cbd5e1" opacity="0.7">
                <animate attributeName="height" values="55;75;55" dur="1.5s" begin="0.6s" repeatCount="indefinite" />
                <animate attributeName="y" values="55;35;55" dur="1.5s" begin="0.6s" repeatCount="indefinite" />
              </rect>
            </svg>
            <div className="chart-preview-empty-title">Configure your chart to see preview</div>
          </div>
        )}
      </div>
    </div>
  );
};

export default ChartPreview;
