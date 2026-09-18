"use client";

import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import type { ClickBenchFigure, ClickBenchStatement } from "../../utils/clickbench";
import styles from "./BenchmarkFigure.module.css";

/**
 * Per-statement time for the 43 ClickBench statements, in upstream order,
 * on a log time axis. Hand-built SVG: 43 marks, two reference lines and four
 * ticks need no charting runtime, stay crisp at any width, and every mark can
 * be a real focusable element with its own description. Wide containers draw
 * statements as columns; below 640 px the figure turns into rows so all 43
 * stay legible without horizontal scrolling.
 */

const ROW_BREAKPOINT = 640;
const FLOOR_SECONDS = 0.1;
const THRESHOLDS = [1, 10] as const;

export function formatSeconds(seconds: number): string {
  if (seconds < 10) return `${seconds.toFixed(2)} s`;
  if (seconds < 100) return `${seconds.toFixed(1)} s`;
  return `${Math.round(seconds)} s`;
}

function spokenSeconds(seconds: number): string {
  const value = seconds < 10 ? seconds.toFixed(2) : seconds < 100 ? seconds.toFixed(1) : String(Math.round(seconds));
  return `${value} seconds`;
}

function tickLabel(seconds: number): string {
  return `${seconds} s`;
}

function useContainerWidth(): [React.RefObject<HTMLDivElement | null>, number | null] {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setWidth(Math.max(280, Math.round(el.getBoundingClientRect().width)));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  return [ref, width];
}

function useRevealOnce(ref: React.RefObject<HTMLDivElement | null>): boolean {
  const [revealed, setRevealed] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || revealed) return;
    if (typeof IntersectionObserver === "undefined") { setRevealed(true); return; }
    const observer = new IntersectionObserver(([entry]) => {
      if (entry.isIntersecting) { setRevealed(true); observer.disconnect(); }
    }, { threshold: 0.25 });
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref, revealed]);
  return revealed;
}

interface Tip { index: number; x: number; y: number }

export function BenchmarkFigure({ figure }: { figure: ClickBenchFigure }) {
  const [containerRef, width] = useContainerWidth();
  const revealed = useRevealOnce(containerRef);
  const [tip, setTip] = useState<Tip | null>(null);
  const [tipSize, setTipSize] = useState<{ w: number; h: number }>({ w: 320, h: 96 });
  const tipRef = useRef<HTMLDivElement>(null);
  const describedBy = useId();

  useLayoutEffect(() => {
    if (tip && tipRef.current) {
      const r = tipRef.current.getBoundingClientRect();
      setTipSize({ w: r.width, h: r.height });
    }
  }, [tip]);

  // A tap opens a statement's detail; a tap anywhere else closes it.
  useEffect(() => {
    if (!tip) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) setTip(null);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [tip, containerRef]);

  const statements = figure.statements;
  const timed = statements.filter((s): s is ClickBenchStatement & { seconds: number } => s.seconds !== null);
  const max = timed.reduce((m, s) => Math.max(m, s.seconds), 1);
  const top = max * 1.5;
  const logFloor = Math.log10(FLOOR_SECONDS);
  const logTop = Math.log10(top);
  const fraction = (seconds: number) => (Math.log10(Math.max(seconds, FLOOR_SECONDS)) - logFloor) / (logTop - logFloor);
  const ticks: number[] = [];
  for (let p = logFloor; p <= logTop; p += 1) ticks.push(Number((10 ** p).toPrecision(1)));
  const counts = THRESHOLDS.map((t) => ({ t, n: timed.filter((s) => s.seconds < t).length }));

  const columns = width === null ? true : width >= ROW_BREAKPOINT;
  // A resize moves every mark; drop the detail rather than leave it stranded.
  useEffect(() => { setTip(null); }, [width]);
  const onKey = (event: KeyboardEvent<SVGGElement>) => {
    if (event.key === "Escape") setTip(null);
  };

  const summary = `${figure.suite}, ${statements.length} statements on ${figure.engine}: ` +
    counts.map(({ t, n }) => `${n} under ${t} second${t === 1 ? "" : "s"}`).join(", ") +
    (figure.summary.slowest ? `; slowest ${figure.summary.slowest.id} at ${spokenSeconds(figure.summary.slowest.seconds)}.` : ".");

  const tipStatement = tip !== null ? statements[tip.index] : null;
  const tipStyle = (() => {
    if (!tip || width === null) return undefined;
    const pad = 8;
    if (columns) {
      const left = Math.min(Math.max(tip.x - tipSize.w / 2, pad), width - tipSize.w - pad);
      const above = tip.y - tipSize.h - 14;
      return { left, top: above > 0 ? above : tip.y + 16 };
    }
    const left = Math.min(Math.max(tip.x + 14, pad), width - tipSize.w - pad);
    return { left, top: Math.max(tip.y - tipSize.h / 2, 0) };
  })();

  return (
    <div ref={containerRef} className={styles.figure}>
      {width === null ? (
        <div className={styles.placeholder} aria-hidden="true" />
      ) : columns ? (
        <ColumnsChart
          width={width} statements={statements} fraction={fraction} ticks={ticks} counts={counts}
          revealed={revealed} active={tip?.index ?? null} setTip={setTip} onKey={onKey} describedBy={describedBy}
        />
      ) : (
        <RowsChart
          width={width} statements={statements} fraction={fraction} ticks={ticks} counts={counts}
          revealed={revealed} active={tip?.index ?? null} setTip={setTip} onKey={onKey} describedBy={describedBy}
        />
      )}
      <p id={describedBy} className={styles.srOnly}>{summary}</p>
      {tipStatement && tipStyle && (
        <div ref={tipRef} className={styles.tip} style={tipStyle} role="status">
          <div className={styles.tipHead}>
            <span>{tipStatement.id}</span>
            <span className={styles.tipTime}>{tipStatement.seconds !== null ? formatSeconds(tipStatement.seconds) : "did not finish"}</span>
          </div>
          <p className={styles.tipLabel}>{tipStatement.label}</p>
          <p className={styles.tipSql}>{tipStatement.sql}</p>
          {tipStatement.seconds === null && <p className={styles.tipNote}>Failed in at least one round, so it has no figure.</p>}
        </div>
      )}
    </div>
  );
}

interface ChartProps {
  width: number;
  statements: ClickBenchStatement[];
  fraction: (seconds: number) => number;
  ticks: number[];
  counts: { t: number; n: number }[];
  revealed: boolean;
  active: number | null;
  setTip: (tip: Tip | null) => void;
  onKey: (event: KeyboardEvent<SVGGElement>) => void;
  describedBy: string;
}

function itemLabel(s: ClickBenchStatement): string {
  return s.seconds === null
    ? `${s.id}, did not finish in every round. ${s.label}`
    : `${s.id}, ${spokenSeconds(s.seconds)}. ${s.label}`;
}

function ColumnsChart({ width, statements, fraction, ticks, counts, revealed, active, setTip, onKey, describedBy }: ChartProps) {
  const ml = 46, mr = 14, mt = 26, mb = 34;
  const height = 340;
  const plotW = width - ml - mr;
  const plotH = height - mt - mb;
  const slot = plotW / statements.length;
  const y = (seconds: number) => mt + plotH - fraction(seconds) * plotH;
  const cx = (i: number) => ml + slot * (i + 0.5);
  const r = slot < 16 ? 3.5 : 4.5;
  const labelEvery = slot >= 22 ? 1 : slot >= 12 ? 2 : 6;

  return (
    <svg
      className={`${styles.svg} ${styles.columns} ${revealed ? styles.revealed : styles.pending}`}
      viewBox={`0 0 ${width} ${height}`} width={width} height={height}
      role="list" aria-label="Seconds per ClickBench statement, upstream order, log scale" aria-describedby={describedBy}
    >
      {ticks.map((t) => (
        <g key={t}>
          <line className={styles.grid} x1={ml} x2={width - mr} y1={y(t)} y2={y(t)} />
          <text className={styles.tick} x={ml - 8} y={y(t) + 4} textAnchor="end">{tickLabel(t)}</text>
        </g>
      ))}
      {counts.map(({ t, n }) => (
        <g key={t}>
          <line className={styles.threshold} x1={ml} x2={width - mr} y1={y(t)} y2={y(t)} />
          <text className={styles.thresholdLabel} x={width - mr} y={y(t) - 6} textAnchor="end">
            <tspan className={styles.thresholdCount}>{n}</tspan> under {t} s
          </text>
        </g>
      ))}
      {statements.map((s, i) => {
        const x = cx(i);
        const yv = s.seconds === null ? y(FLOOR_SECONDS) : y(s.seconds);
        const tipAt = () => setTip({ index: i, x, y: yv });
        return (
          <g
            key={s.id} role="listitem" tabIndex={0} aria-label={itemLabel(s)}
            className={`${styles.item} ${active === i ? styles.itemActive : ""}`}
            style={{ ["--i" as string]: i }}
            onPointerEnter={tipAt} onPointerLeave={() => setTip(null)}
            onClick={tipAt} onFocus={tipAt} onBlur={() => setTip(null)} onKeyDown={onKey}
          >
            <rect className={styles.hit} x={x - slot / 2} y={mt} width={slot} height={plotH + mb} />
            {s.seconds !== null && <line className={styles.stem} x1={x} x2={x} y1={y(FLOOR_SECONDS)} y2={yv} />}
            <circle className={styles.ring} cx={x} cy={yv} r={r + 4} />
            <circle className={s.seconds === null ? styles.dotMissing : styles.dot} cx={x} cy={yv} r={r} />
            {i % labelEvery === 0 && (
              <text className={styles.idLabel} x={x} y={height - mb + 16} textAnchor="middle" aria-hidden="true">{s.id}</text>
            )}
          </g>
        );
      })}
      <text className={styles.axisTitle} x={ml} y={height - 4} aria-hidden="true">ClickBench statement, upstream order</text>
    </svg>
  );
}

function RowsChart({ width, statements, fraction, ticks, counts, revealed, active, setTip, onKey, describedBy }: ChartProps) {
  const ml = 38, mr = 10, mt = 44, mb = 10;
  const rowH = 17;
  const height = mt + rowH * statements.length + mb;
  const plotW = width - ml - mr;
  const x = (seconds: number) => ml + fraction(seconds) * plotW;
  const cy = (i: number) => mt + rowH * (i + 0.5);
  const r = 3.5;

  return (
    <svg
      className={`${styles.svg} ${styles.rows} ${revealed ? styles.revealed : styles.pending}`}
      viewBox={`0 0 ${width} ${height}`} width={width} height={height}
      role="list" aria-label="Seconds per ClickBench statement, upstream order, log scale" aria-describedby={describedBy}
    >
      {ticks.map((t) => (
        <g key={t}>
          <line className={styles.grid} x1={x(t)} x2={x(t)} y1={mt - 6} y2={height - mb} />
          <text className={styles.tick} x={x(t)} y={mt - 10} textAnchor="middle">{tickLabel(t)}</text>
        </g>
      ))}
      {counts.map(({ t, n }) => (
        <g key={t}>
          <line className={styles.threshold} x1={x(t)} x2={x(t)} y1={mt - 6} y2={height - mb} />
          <text className={styles.thresholdLabel} x={x(t) + 6} y={mt - 26} textAnchor="start">
            <tspan className={styles.thresholdCount}>{n}</tspan> under {t} s
          </text>
        </g>
      ))}
      {statements.map((s, i) => {
        const yv = cy(i);
        const xv = s.seconds === null ? x(FLOOR_SECONDS) : x(s.seconds);
        const tipAt = () => setTip({ index: i, x: xv, y: yv });
        return (
          <g
            key={s.id} role="listitem" tabIndex={0} aria-label={itemLabel(s)}
            className={`${styles.item} ${active === i ? styles.itemActive : ""}`}
            style={{ ["--i" as string]: i }}
            onPointerEnter={tipAt} onPointerLeave={() => setTip(null)}
            onClick={tipAt} onFocus={tipAt} onBlur={() => setTip(null)} onKeyDown={onKey}
          >
            <rect className={styles.hit} x={0} y={yv - rowH / 2} width={width} height={rowH} />
            <text className={styles.idLabel} x={ml - 8} y={yv + 4} textAnchor="end" aria-hidden="true">{s.id}</text>
            {s.seconds !== null && <line className={styles.stem} x1={x(FLOOR_SECONDS)} x2={xv} y1={yv} y2={yv} />}
            <circle className={styles.ring} cx={xv} cy={yv} r={r + 4} />
            <circle className={s.seconds === null ? styles.dotMissing : styles.dot} cx={xv} cy={yv} r={r} />
          </g>
        );
      })}
    </svg>
  );
}
