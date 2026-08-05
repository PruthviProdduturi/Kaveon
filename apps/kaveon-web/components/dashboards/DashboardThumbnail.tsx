"use client";

/**
 * DashboardThumbnail — a real, live preview of a dashboard's hero chart, used as
 * the card thumbnail on the dashboards list. Lazy: only fetches + renders once the
 * card scrolls near the viewport, so a long list doesn't fire N queries at once.
 */
import React, { useEffect, useRef, useState } from "react";
import dynamic from "next/dynamic";
import { msalFetch } from "../../utils/msalFetch";
import { API_BASE } from "../../config";
import { ChartBuilderProvider } from "../charts/ChartBuilderContext";

const ChartHydrator = dynamic(() => import("../charts/ChartHydrator"), { ssr: false });
const ChartPreview = dynamic(() => import("../charts/ChartPreview"), { ssr: false });

interface Props {
  chartId?: number | null;
  icon?: string;
}

const DashboardThumbnail: React.FC<Props> = ({ chartId, icon = "fa-tachometer-alt" }) => {
  const ref = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const [chart, setChart] = useState<any>(null);

  useEffect(() => {
    if (!ref.current || visible) return;
    const io = new IntersectionObserver(
      (entries) => { if (entries.some((e) => e.isIntersecting)) { setVisible(true); io.disconnect(); } },
      { rootMargin: "200px" }
    );
    io.observe(ref.current);
    return () => io.disconnect();
  }, [visible]);

  useEffect(() => {
    if (!visible || !chartId || chart) return;
    let alive = true;
    msalFetch(`${API_BASE}/api/v1/charts/${chartId}`)
      .then((r) => (r.ok ? r.json() : null))
      .then((c) => { if (alive) setChart(c); })
      .catch(() => {});
    return () => { alive = false; };
  }, [visible, chartId, chart]);

  return (
    <div ref={ref} style={{ position: "relative", width: "100%", height: "100%", overflow: "hidden", background: "#f8fafc" }}>
      {chartId && visible ? (
        <div style={{ position: "absolute", inset: 0, pointerEvents: "none" }}>
          <ChartBuilderProvider runContext="thumbnail">
            <ChartHydrator chart={chart} />
            <div style={{ width: "100%", height: "100%" }}>
              <ChartPreview />
            </div>
          </ChartBuilderProvider>
        </div>
      ) : (
        <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", color: "#cbd5e1" }}>
          <i className={`fas ${icon}`} style={{ fontSize: 32 }} />
        </div>
      )}
    </div>
  );
};

export default DashboardThumbnail;
