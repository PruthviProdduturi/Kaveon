// Static imports — this file is loaded as a single webpack chunk via dynamic()
// so echarts-gl extends the same echarts instance that ReactECharts uses.
import "echarts-gl";
import * as echarts from "echarts";
import React from "react";

const MAP_DEFAULT_COLORS = ["#e0f2fe", "#0ea5e9", "#0369a1"];

interface Props {
  rows: (string | number | null)[][];
  columns: string[];
  geoJson: any;
  advancedOptions?: any;
}

const WorldMapGlobe: React.FC<Props> = ({ rows, columns, geoJson, advancedOptions }) => {
  const containerRef = React.useRef<HTMLDivElement>(null);
  const chartRef     = React.useRef<echarts.ECharts | null>(null);

  const ctOpts      = advancedOptions?.chartTypeOptions || {};
  const colorScheme = advancedOptions?.color?.length ? advancedOptions.color : MAP_DEFAULT_COLORS;
  const showLabels  = ctOpts.mapShowLabels === true;
  const numFmt      = ctOpts.mapNumberFormat || "none";

  const fmtVal = (v: number): string => {
    if (numFmt === "k") return `${(v / 1e3).toFixed(1)}K`;
    if (numFmt === "m") return `${(v / 1e6).toFixed(1)}M`;
    if (numFmt === "b") return `${(v / 1e9).toFixed(1)}B`;
    if (numFmt === "t") return `${(v / 1e12).toFixed(1)}T`;
    return v.toLocaleString(undefined, { maximumFractionDigits: 2 });
  };

  const nameIdx = columns.findIndex((_, i) => rows.some(r => isNaN(Number(r[i]))));
  const valIdx  = columns.length - 1 === nameIdx ? columns.length - 2 : columns.length - 1;

  const data = React.useMemo(() =>
    nameIdx >= 0 && valIdx >= 0
      ? rows
          .map(r => ({ name: String(r[nameIdx] ?? ""), value: Number(r[valIdx] ?? 0) }))
          .filter(d => d.name && !isNaN(d.value))
      : [],
  // eslint-disable-next-line react-hooks/exhaustive-deps
  [JSON.stringify(rows), nameIdx, valIdx]);

  const values = data.map(d => d.value);
  const minV   = values.length ? Math.min(...values) : 0;
  const maxV   = values.length ? Math.max(...values) : 1;

  const titleOpt = advancedOptions?.title?.text
    ? { title: { text: advancedOptions.title.text, left: "center", top: 5, textStyle: { color: "#e2e8f0", fontSize: Number(advancedOptions.titleSize) || 20, fontFamily: advancedOptions.titleFont || "sans-serif" } } }
    : {};

  const option = React.useMemo(() => ({
    ...titleOpt,
    backgroundColor: "#0d1117",
    tooltip: {
      trigger: "item",
      appendToBody: true,
      backgroundColor: "rgba(255,255,255,0.97)",
      borderColor: "#e2e8f0",
      borderWidth: 1,
      padding: [10, 14],
      textStyle: { color: "#1e293b", fontSize: 12, fontFamily: "Inter, -apple-system, sans-serif" },
      extraCssText: "box-shadow: 0 8px 24px rgba(0,0,0,0.12); border-radius: 8px;",
      formatter: (p: any) => {
        const v = Number(p.value);
        return `<b>${p.name}</b><br/>${p.value != null && !isNaN(v) ? fmtVal(v) : "—"}`;
      },
    },
    visualMap: {
      min: minV, max: maxV,
      left: "left", bottom: 30,
      text: [fmtVal(maxV), fmtVal(minV)],
      inRange: { color: colorScheme },
      calculable: true,
      textStyle: { fontSize: 11, color: "#94a3b8" },
    },
    globe: {
      baseTexture: "#0c1e35",
      shading: "lambert",
      light: {
        ambient: { intensity: 0.6 },
        main: { intensity: 1.2, shadow: false },
      },
      atmosphere: { show: true },
      viewControl: {
        autoRotate: false,
        distance: 160,
        minDistance: 80,
        maxDistance: 320,
        rotateSensitivity: 1,
        zoomSensitivity: 1,
        panSensitivity: 0,
      },
    },
    series: [{
      type: "map3D",
      coordinateSystem: "globe",
      map: "world",
      data,
      shading: "lambert",
      emphasis: {
        label: { show: true, textStyle: { color: "#fff", fontSize: 11, fontFamily: "Inter, sans-serif" } },
        itemStyle: { color: "#fbbf24" },
      },
      itemStyle: { borderWidth: 0.4, borderColor: "rgba(255,255,255,0.15)" },
      label: { show: showLabels, textStyle: { color: "#fff", fontSize: 9, fontFamily: "Inter, sans-serif" } },
    }],
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }), [data, minV, maxV, JSON.stringify(colorScheme), showLabels, numFmt, JSON.stringify(titleOpt)]);

  // ── Lifecycle ──────────────────────────────────────────────────────────────
  // echarts-for-react's lifecycle is incompatible with echarts-gl's WebGL
  // renderer. We manage the instance manually:
  //
  //  • echarts.init() creates the canvas/context once when geoJson arrives.
  //  • setOption is deferred via requestAnimationFrame so the browser has
  //    painted the container and the GL context is fully ready before we
  //    touch it (synchronous setOption causes "null renderer" crashes).
  //  • The RAF id is cancelled in the effect cleanup so rapid option changes
  //    don't stack up.
  //  • A separate unmount-only effect disposes the instance and removes the
  //    resize listener exactly once.

  React.useEffect(() => {
    if (!geoJson || !containerRef.current) return;

    echarts.registerMap("world", geoJson);

    // Create instance on first run
    if (!chartRef.current) {
      try {
        chartRef.current = echarts.init(containerRef.current);
      } catch (_) {
        return; // WebGL unavailable
      }
    }

    const instance = chartRef.current;

    // Double-rAF: first frame lets the browser flush layout so the container
    // has real pixel dimensions; second frame is when the GL context is safe
    // to write into. Also call resize() after setOption so echarts picks up
    // the correct canvas size even if init() ran on a zero-height container.
    let rafId = requestAnimationFrame(() => {
      rafId = requestAnimationFrame(() => {
        if (!instance || instance.isDisposed()) return;
        try {
          instance.setOption(option, { notMerge: true });
          instance.resize();
        } catch (_) {}
      });
    });

    return () => { cancelAnimationFrame(rafId); };
  }, [geoJson, option]);

  // Dispose + resize listener — unmount only
  React.useEffect(() => {
    const onResize = () => {
      requestAnimationFrame(() => {
        try { chartRef.current?.resize(); } catch (_) {}
      });
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      if (chartRef.current) {
        try { chartRef.current.dispose(); } catch (_) {}
        chartRef.current = null;
      }
    };
  }, []);

  return (
    <div
      ref={containerRef}
      style={{ position: "absolute", inset: 0, background: "#0d1117" }}
    />
  );
};

export default WorldMapGlobe;
