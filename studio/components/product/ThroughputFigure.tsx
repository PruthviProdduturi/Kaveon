"use client";

import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import type { ThroughputFigure as ThroughputData, ThroughputRound } from "../../utils/benchmarkTypes";
import styles from "./BenchmarkFigure.module.css";

/**
 * Successful exact executions per second per client count: every round a
 * hollow mark, the median over rounds the emphasised bar with its value. The
 * same SVG language as the latency figure — same ticks, grid, marks, tooltip
 * and one reveal — on a linear axis from zero.
 */

const MAX_WIDTH = 560;

export function formatRate(rate: number): string {
  return rate.toFixed(3);
}

function niceStep(max: number): number {
  const raw = max / 4;
  const power = 10 ** Math.floor(Math.log10(raw));
  const candidates = [1, 2, 2.5, 5, 10].map((m) => m * power);
  return candidates.find((c) => c >= raw) ?? candidates[candidates.length - 1];
}

function formatSpan(seconds: number | null): string {
  return seconds === null ? "" : ` in ${Math.round(seconds)} s`;
}

function roundLabel(clients: number, r: ThroughputRound): string {
  const rate = r.executions_per_second === null ? "no rate" : `${formatRate(r.executions_per_second)} executions per second`;
  return `${clients} clients, round ${r.round}: ${rate}; ${r.successful} successful${formatSpan(r.elapsed_seconds)}, ${r.failures} failures, ${r.rejections} admission rejections${formatTies(r.ties)}.`;
}

/** A tie: an ORDER BY … LIMIT statement that returned a different row set of the same size, counted as an execution too. */
function formatTies(ties: number): string {
  return ties === 0 ? "" : `, ${ties} ${ties === 1 ? "tie" : "ties"} at an ORDER BY cut`;
}

interface Tip { x: number; y: number; clients: number; round: ThroughputRound }

export function ThroughputFigure({ figure }: { figure: ThroughputData }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [tip, setTip] = useState<Tip | null>(null);
  const [tipSize, setTipSize] = useState({ w: 300, h: 80 });
  const tipRef = useRef<HTMLDivElement>(null);
  const describedBy = useId();

  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const measure = () => setWidth(Math.max(260, Math.min(MAX_WIDTH, Math.round(el.getBoundingClientRect().width))));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const el = containerRef.current;
    if (!el || revealed) return;
    if (typeof IntersectionObserver === "undefined") { setRevealed(true); return; }
    const observer = new IntersectionObserver(([entry]) => {
      if (entry.isIntersecting) { setRevealed(true); observer.disconnect(); }
    }, { threshold: 0.25 });
    observer.observe(el);
    return () => observer.disconnect();
  }, [revealed]);

  useLayoutEffect(() => {
    if (tip && tipRef.current) {
      const r = tipRef.current.getBoundingClientRect();
      setTipSize({ w: r.width, h: r.height });
    }
  }, [tip]);

  useEffect(() => { setTip(null); }, [width]);

  useEffect(() => {
    if (!tip) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) setTip(null);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [tip]);

  const groups = figure.groups;
  const rates = groups.flatMap((g) => g.rounds.map((r) => r.executions_per_second)).filter((r): r is number => r !== null);
  const max = Math.max(...rates, 0.001) * 1.25;
  const step = niceStep(max);
  const ticks: number[] = [];
  for (let t = 0; t <= max + 1e-9; t += step) ticks.push(Number(t.toFixed(6)));

  const height = 300;
  const ml = 54, mr = 16, mt = 22, mb = 40;
  const w = width ?? MAX_WIDTH;
  const plotW = w - ml - mr;
  const plotH = height - mt - mb;
  const y = (rate: number) => mt + plotH - (rate / max) * plotH;
  const groupX = (k: number) => ml + plotW * ((k + 0.5) / groups.length);
  const barHalf = Math.min(28, plotW / groups.length / 4);
  const markGap = 14;

  const summary = `${figure.suite} concurrency on ${figure.engine}: ` + groups.map((g) =>
    `${g.clients} clients, median ${g.median_executions_per_second === null ? "no rate" : `${formatRate(g.median_executions_per_second)} executions per second`} over ${g.rounds.length} round${g.rounds.length === 1 ? "" : "s"}`).join("; ") + ".";

  const onKey = (event: KeyboardEvent<SVGGElement>) => { if (event.key === "Escape") setTip(null); };

  const tipStyle = (() => {
    if (!tip || width === null) return undefined;
    const pad = 8;
    const left = Math.min(Math.max(tip.x - tipSize.w / 2, pad), width - tipSize.w - pad);
    const above = tip.y - tipSize.h - 14;
    return { left, top: above > 0 ? above : tip.y + 16 };
  })();

  return (
    <div ref={containerRef} className={styles.figure} style={{ maxWidth: MAX_WIDTH, margin: "0 auto" }}>
      {width === null ? (
        <div className={styles.placeholder} aria-hidden="true" style={{ minHeight: height }} />
      ) : (
        <svg
          className={`${styles.svg} ${styles.columns} ${revealed ? styles.revealed : styles.pending}`}
          viewBox={`0 0 ${w} ${height}`} width={w} height={height}
          role="list" aria-label="Successful executions per second by client count, each round and the median" aria-describedby={describedBy}
        >
          {ticks.map((t) => (
            <g key={t}>
              <line className={styles.grid} x1={ml} x2={w - mr} y1={y(t)} y2={y(t)} />
              <text className={styles.tick} x={ml - 8} y={y(t) + 4} textAnchor="end">{t === 0 ? "0" : formatRate(t)}</text>
            </g>
          ))}
          <text className={styles.axisTitle} x={ml} y={12} aria-hidden="true">executions per second</text>
          <g aria-hidden="true">
            <circle className={styles.dotHollow} cx={w - mr - 118} cy={9} r={4} style={{ opacity: 1, transition: "none" }} />
            <text className={styles.axisTitle} x={w - mr - 108} y={12}>round</text>
            <line className={styles.stem} x1={w - mr - 58} x2={w - mr - 40} y1={9} y2={9} style={{ strokeOpacity: 0.9, strokeWidth: 3, transform: "none", transition: "none" }} />
            <text className={styles.axisTitle} x={w - mr - 34} y={12}>median</text>
          </g>
          {groups.map((g, k) => {
            const gx = groupX(k);
            const n = g.rounds.length;
            return (
              <g key={g.clients}>
                {g.median_executions_per_second !== null && (
                  <g style={{ ["--i" as string]: k * n }}>
                    <line className={styles.stem} x1={gx - barHalf} x2={gx + barHalf} y1={y(g.median_executions_per_second)} y2={y(g.median_executions_per_second)} style={{ strokeOpacity: 0.9, strokeWidth: 3 }} />
                    <text className={styles.thresholdLabel} x={gx + barHalf + 8} y={y(g.median_executions_per_second) + 4} textAnchor="start" aria-hidden="true">
                      <tspan className={styles.thresholdCount}>{formatRate(g.median_executions_per_second)}</tspan>
                    </text>
                  </g>
                )}
                {g.rounds.map((r, i) => {
                  if (r.executions_per_second === null) return null;
                  const x = gx + (i - (n - 1) / 2) * markGap;
                  const yv = y(r.executions_per_second);
                  const tipAt = () => setTip({ x, y: yv, clients: g.clients, round: r });
                  const isMedian = r.executions_per_second === g.median_executions_per_second;
                  return (
                    <g
                      key={r.round} role="listitem" tabIndex={0} aria-label={roundLabel(g.clients, r)}
                      className={`${styles.item} ${tip?.round === r ? styles.itemActive : ""}`}
                      style={{ ["--i" as string]: k * n + i }}
                      onPointerEnter={tipAt} onPointerLeave={() => setTip(null)}
                      onClick={tipAt} onFocus={tipAt} onBlur={() => setTip(null)} onKeyDown={onKey}
                    >
                      <rect className={styles.hit} x={x - markGap / 2} y={mt} width={markGap} height={plotH} />
                      <circle className={styles.ring} cx={x} cy={yv} r={9} />
                      <circle className={isMedian ? styles.dot : styles.dotHollow} cx={x} cy={yv} r={isMedian ? 5.5 : 4.5} />
                    </g>
                  );
                })}
                <text className={styles.thresholdLabel} x={gx} y={height - mb + 20} textAnchor="middle" aria-hidden="true">{g.clients} clients</text>
                <text className={styles.tick} x={gx} y={height - mb + 36} textAnchor="middle" aria-hidden="true">{n} round{n === 1 ? "" : "s"}</text>
              </g>
            );
          })}
        </svg>
      )}
      <p id={describedBy} className={styles.srOnly}>{summary}</p>
      {tip && tipStyle && (
        <div ref={tipRef} className={styles.tip} style={tipStyle} role="status">
          <div className={styles.tipHead}>
            <span>{tip.clients} clients, round {tip.round.round}</span>
            <span className={styles.tipTime}>{tip.round.executions_per_second === null ? "no rate" : `${formatRate(tip.round.executions_per_second)} / s`}</span>
          </div>
          <p className={styles.tipLabel}>
            {tip.round.successful} successful{formatSpan(tip.round.elapsed_seconds)}, {tip.round.failures} failed, {tip.round.rejections} refused by admission{formatTies(tip.round.ties)}.
          </p>
        </div>
      )}
    </div>
  );
}
