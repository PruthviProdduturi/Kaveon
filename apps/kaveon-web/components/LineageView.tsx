"use client";

import { useCallback, useEffect, useState, useRef } from "react";
import { useRouter } from "next/navigation";
import { msalFetch } from "../utils/msalFetch";

interface DataSource { id: string | number; name: string; engine?: string; }
interface Dataset { id: string | number; name?: string; database_name?: string; table_name?: string; description?: string | null; }
interface Dashboard { id: string | number; name?: string; charts?: string; description?: string | null; }
interface DlmCoverage { dataset_id: string; dataset_name?: string; row_count?: number; values_indexed?: number; answers_precomputed?: number; }

interface LineageNode {
  id: string;
  type: "source" | "dataset" | "dlm" | "dashboard";
  label: string;
  sublabel?: string;
  col: number;
  row: number;
  meta?: Record<string, unknown>;
}
interface LineageEdge { from: string; to: string; }

function compact(n?: number | null): string {
  if (n == null) return "";
  const a = Math.abs(n);
  if (a >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + "B";
  if (a >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (a >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
}

const COL_LABELS = ["Data Sources", "Datasets", "Semantic Layer", "Dashboards"];
const COL_ICONS = ["fa-server", "fa-database", "fa-bolt", "fa-table-cells-large"];
const COL_COLORS = ["#3b82f6", "#8b5cf6", "#f59e0b", "#10b981"];

const NODE_W = 200;
const NODE_H = 64;
const COL_GAP = 100;
const ROW_GAP = 16;
const COL_WIDTH = NODE_W + COL_GAP;
const PAD_X = 60;
const PAD_Y = 80;

export function LineageView() {
  const router = useRouter();
  const [loading, setLoading] = useState(true);
  const [nodes, setNodes] = useState<LineageNode[]>([]);
  const [edges, setEdges] = useState<LineageEdge[]>([]);
  const [hovered, setHovered] = useState<string | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);

  const build = useCallback(async () => {
    setLoading(true);
    try {
      const [srcRes, dsRes, dashRes, dlmRes] = await Promise.all([
        msalFetch("/api/v1/data-sources/active"),
        msalFetch("/api/v1/datasets"),
        msalFetch("/api/v1/dashboards"),
        msalFetch("/api/v1/dlm/coverage"),
      ]);

      const sources: DataSource[] = srcRes.ok ? await srcRes.json().then((d: { result?: DataSource[] } | DataSource[]) => Array.isArray(d) ? d : d.result || []) : [];
      const datasets: Dataset[] = dsRes.ok ? await dsRes.json().then((d: { result?: Dataset[] } | Dataset[]) => Array.isArray(d) ? d : d.result || []) : [];
      const dashboards: Dashboard[] = dashRes.ok ? await dashRes.json().then((d: { result?: Dashboard[] } | Dashboard[]) => Array.isArray(d) ? d : d.result || []) : [];
      const dlmCoverage: DlmCoverage[] = dlmRes.ok ? await dlmRes.json().then((d: { datasets?: DlmCoverage[] } | DlmCoverage[]) => Array.isArray(d) ? d : d.datasets || []) : [];

      const dlmMap = new Map(dlmCoverage.map((d) => [String(d.dataset_id), d]));

      const ns: LineageNode[] = [];
      const es: LineageEdge[] = [];

      const srcMap = new Map<string, number>();
      sources.forEach((s, i) => {
        const id = `src-${s.id}`;
        srcMap.set(s.name, i);
        ns.push({ id, type: "source", label: s.name, sublabel: s.engine || "", col: 0, row: i, meta: s as unknown as Record<string, unknown> });
      });

      const dsMap = new Map<string | number, number>();
      datasets.forEach((d, i) => {
        const id = `ds-${d.id}`;
        dsMap.set(d.id, i);
        const dlm = dlmMap.get(String(d.id));
        ns.push({ id, type: "dataset", label: d.name || `Dataset ${d.id}`, sublabel: d.table_name || "", col: 1, row: i, meta: { ...d, dlm } as unknown as Record<string, unknown> });

        if (d.database_name) {
          const srcIdx = srcMap.get(d.database_name);
          if (srcIdx != null) {
            es.push({ from: `src-${sources[srcIdx].id}`, to: id });
          }
        }

        if (dlm) {
          const dlmId = `dlm-${d.id}`;
          ns.push({
            id: dlmId, type: "dlm",
            label: compact(dlm.values_indexed) + " values",
            sublabel: compact(dlm.row_count) + " rows",
            col: 2, row: i,
          });
          es.push({ from: id, to: dlmId });
        }
      });

      const chartDatasetMap = new Map<string | number, Set<string | number>>();
      for (const dash of dashboards) {
        try {
          const chartIds: number[] = JSON.parse(dash.charts || "[]");
          if (chartIds.length) {
            const chartResps = await Promise.all(
              chartIds.slice(0, 20).map((cid) => msalFetch(`/api/v1/charts/${cid}`).catch(() => null))
            );
            const dsIds = new Set<string | number>();
            for (const resp of chartResps) {
              if (resp?.ok) {
                const chart = await resp.json();
                if (chart.dataset_id) dsIds.add(chart.dataset_id);
              }
            }
            chartDatasetMap.set(dash.id, dsIds);
          }
        } catch { /* ignore parse errors */ }
      }

      let dashRow = 0;
      dashboards.forEach((d) => {
        const id = `dash-${d.id}`;
        ns.push({ id, type: "dashboard", label: d.name || `Dashboard ${d.id}`, sublabel: d.description || "", col: 3, row: dashRow, meta: d as unknown as Record<string, unknown> });

        const dsIds = chartDatasetMap.get(d.id);
        if (dsIds) {
          for (const dsId of dsIds) {
            const dlm = dlmMap.get(String(dsId));
            if (dlm) {
              es.push({ from: `dlm-${dsId}`, to: id });
            } else {
              es.push({ from: `ds-${dsId}`, to: id });
            }
          }
        }
        dashRow++;
      });

      setNodes(ns);
      setEdges(es);
    } catch { /* ignore */ }
    setLoading(false);
  }, []);

  useEffect(() => { build(); }, [build]);

  const maxRows = [0, 1, 2, 3].map((col) => nodes.filter((n) => n.col === col).length);
  const maxRow = Math.max(...maxRows, 1);
  const svgW = 4 * COL_WIDTH + PAD_X * 2 - COL_GAP;
  const svgH = maxRow * (NODE_H + ROW_GAP) + PAD_Y * 2;

  const nodePos = (n: LineageNode) => ({
    x: PAD_X + n.col * COL_WIDTH,
    y: PAD_Y + n.row * (NODE_H + ROW_GAP),
  });

  const connectedSet = new Set<string>();
  if (hovered) {
    connectedSet.add(hovered);
    for (const e of edges) {
      if (e.from === hovered) connectedSet.add(e.to);
      if (e.to === hovered) connectedSet.add(e.from);
    }
  }

  const nodeClick = (n: LineageNode) => {
    if (n.type === "dataset") router.push(`/datasets/${n.id.replace("ds-", "")}`);
    else if (n.type === "dashboard") router.push(`/dashboards/${n.id.replace("dash-", "")}/view`);
  };

  if (loading) {
    return (
      <div style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: "80px 0", color: "var(--text-muted)", fontSize: 14 }}>
        <i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />
        Building lineage graph...
      </div>
    );
  }

  if (nodes.length === 0) {
    return (
      <div style={{ textAlign: "center", padding: "80px 0", color: "var(--text-muted)", fontSize: 14 }}>
        No lineage data found. Create datasets and dashboards to see the data flow.
      </div>
    );
  }

  return (
    <div style={{ marginTop: 20 }}>
      {/* Column headers */}
      <div style={{ display: "flex", gap: COL_GAP, paddingLeft: PAD_X, marginBottom: 8 }}>
        {COL_LABELS.map((label, i) => (
          <div key={i} style={{ width: NODE_W, display: "flex", alignItems: "center", gap: 8 }}>
            <div style={{ width: 28, height: 28, borderRadius: 7, background: `${COL_COLORS[i]}18`, display: "flex", alignItems: "center", justifyContent: "center" }}>
              <i className={`fas ${COL_ICONS[i]}`} style={{ fontSize: 12, color: COL_COLORS[i] }} />
            </div>
            <span style={{ fontSize: 12, fontWeight: 700, color: "var(--text-muted)", textTransform: "uppercase", letterSpacing: "0.05em" }}>{label}</span>
          </div>
        ))}
      </div>

      {/* Graph */}
      <div style={{ overflowX: "auto", overflowY: "visible" }}>
        <svg ref={svgRef} width={svgW} height={svgH} style={{ display: "block" }}>
          {/* Edges */}
          {edges.map((e, i) => {
            const fromN = nodes.find((n) => n.id === e.from);
            const toN = nodes.find((n) => n.id === e.to);
            if (!fromN || !toN) return null;
            const fp = nodePos(fromN);
            const tp = nodePos(toN);
            const x1 = fp.x + NODE_W;
            const y1 = fp.y + NODE_H / 2;
            const x2 = tp.x;
            const y2 = tp.y + NODE_H / 2;
            const cx = (x1 + x2) / 2;
            const dimmed = hovered && !connectedSet.has(e.from) && !connectedSet.has(e.to);
            const highlighted = hovered && connectedSet.has(e.from) && connectedSet.has(e.to);
            return (
              <path
                key={i}
                d={`M${x1},${y1} C${cx},${y1} ${cx},${y2} ${x2},${y2}`}
                fill="none"
                stroke={highlighted ? "var(--accent)" : "var(--border)"}
                strokeWidth={highlighted ? 2 : 1.5}
                opacity={dimmed ? 0.15 : highlighted ? 1 : 0.5}
                style={{ transition: "opacity 0.15s, stroke 0.15s" }}
              />
            );
          })}

          {/* Nodes */}
          {nodes.map((n) => {
            const pos = nodePos(n);
            const color = COL_COLORS[n.col];
            const dimmed = hovered && !connectedSet.has(n.id);
            const isHovered = hovered === n.id;
            const clickable = n.type === "dataset" || n.type === "dashboard";
            return (
              <g
                key={n.id}
                transform={`translate(${pos.x},${pos.y})`}
                onMouseEnter={() => setHovered(n.id)}
                onMouseLeave={() => setHovered(null)}
                onClick={() => nodeClick(n)}
                style={{ cursor: clickable ? "pointer" : "default", transition: "opacity 0.15s" }}
                opacity={dimmed ? 0.25 : 1}
              >
                <rect
                  width={NODE_W} height={NODE_H} rx={10}
                  fill="var(--bg-surface)"
                  stroke={isHovered ? color : "var(--border)"}
                  strokeWidth={isHovered ? 2 : 1}
                />
                <line x1={0} y1={0} x2={0} y2={NODE_H} stroke={color} strokeWidth={3} strokeLinecap="round" />
                <text x={14} y={26} fill="var(--text-primary)" fontSize={13} fontWeight={600} fontFamily="inherit">
                  {n.label.length > 22 ? n.label.slice(0, 20) + "..." : n.label}
                </text>
                {n.sublabel && (
                  <text x={14} y={44} fill="var(--text-muted)" fontSize={11} fontFamily="inherit">
                    {n.sublabel.length > 28 ? n.sublabel.slice(0, 26) + "..." : n.sublabel}
                  </text>
                )}
              </g>
            );
          })}
        </svg>
      </div>
    </div>
  );
}
