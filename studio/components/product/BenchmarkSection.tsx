"use client";

import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { BenchmarkFigures, Cluster, LatencyFigure, ThroughputFigure as ThroughputData } from "../../utils/benchmarkTypes";
import { BenchmarkFigure, formatSeconds } from "./BenchmarkFigure";
import { ThroughputFigure, formatRate } from "./ThroughputFigure";
import styles from "./BenchmarkSection.module.css";

const REPO_TREE = "https://github.com/PruthviProdduturi/Kaveon/tree/dev/";

type TabId = "clickbench" | "tpch" | "throughput";

interface Tab {
  id: TabId;
  title: string;
  desc: string;
}

function longDate(iso: string): string {
  const date = new Date(`${iso}T00:00:00Z`);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" });
}

function shortDigest(digest: string): string {
  return digest.replace(/^sha256:/, "").slice(0, 12);
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

function list(parts: (string | number)[]): string {
  const words = parts.map(String);
  if (words.length <= 1) return words.join("");
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`;
}

function recordUrl(recordPath: string): string {
  return REPO_TREE + recordPath.replace(/^\/+/, "");
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className={styles.fact}>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

function CommonFacts({ cluster, date, digests, recordPath }: { cluster: Cluster; date: string; digests: string[]; recordPath: string }) {
  return (
    <>
      <Fact label="Cluster">{cluster.workers} × {cluster.node_sku} workers, {cluster.memory_per_query_per_worker} per query per worker</Fact>
      <Fact label="Recorded">{longDate(date)}</Fact>
      <Fact label={digests.length === 1 ? "Engine build" : "Engine builds"}>
        {digests.length === 0
          ? "—"
          : digests.map((d, i) => <span key={d} className={styles.digest} title={d}>{i > 0 ? ", " : ""}{shortDigest(d)}</span>)}
      </Fact>
      <Fact label="Record"><a href={recordUrl(recordPath)} target="_blank" rel="noopener noreferrer">Round records on GitHub</a></Fact>
    </>
  );
}

function LatencyPanel({ figure }: { figure: LatencyFigure }) {
  const { summary, cluster } = figure;
  const rows = figure.rows === null ? null : figure.rows.toLocaleString("en-US");
  const missing = summary.statements - summary.ran_every_round;
  const subject = figure.scale_factor !== null
    ? <>The {summary.statements} {figure.suite} queries at scale factor {figure.scale_factor}</>
    : <>The {summary.statements} upstream {figure.suite} statements{rows && figure.table ? <> over <strong>{rows} rows</strong> of the public <code>{figure.table}</code> table</> : null}</>;

  return (
    <>
      <div className={styles.head}>
        <h3 className={styles.title}>
          All {summary.statements} {figure.suite} queries{figure.scale_factor !== null ? ` at scale factor ${figure.scale_factor}` : ""},<br />timed on {figure.engine}.
        </h3>
        <p className={styles.method}>
          {subject}, on {cluster.workers} {cluster.node_sku} worker nodes with {cluster.memory_per_query_per_worker} per query per worker.
          Each round starts cold and runs every statement {figure.executions_per_round} times; a statement&rsquo;s figure is the
          median of its round medians over {plural(figure.rounds, "round")}.
        </p>
      </div>
      <figure className={styles.card}>
        <BenchmarkFigure figure={figure} />
        <figcaption className={styles.caption}>
          <strong>{summary.under_1s}</strong> of {summary.ran_every_round} statements finish under one second and{" "}
          <strong>{summary.under_10s}</strong> under ten.
          {summary.slowest ? <> The slowest, <strong>{summary.slowest.id}</strong>, takes <strong>{formatSeconds(summary.slowest.seconds)}</strong>.</> : null}
          {missing > 0 ? <> {plural(missing, "statement")} did not finish in every round and {missing === 1 ? "has" : "have"} no figure.</> : null}
        </figcaption>
      </figure>
      <dl className={styles.facts}>
        {rows && <Fact label="Rows scanned">{rows}</Fact>}
        {figure.scale_factor !== null && <Fact label="Scale factor">{figure.scale_factor}</Fact>}
        <Fact label="Rounds">{figure.rounds}, cold, {figure.executions_per_round} executions each</Fact>
        <CommonFacts cluster={cluster} date={figure.date} digests={figure.engine_digests} recordPath={figure.record_path} />
      </dl>
    </>
  );
}

function ThroughputPanel({ figure }: { figure: ThroughputData }) {
  const { cluster, groups } = figure;
  const clients = list(groups.map((g) => g.clients));
  const rounds = list(groups.map((g) => `${plural(g.rounds.length, "round")} at ${g.clients} clients`));
  const medians = groups
    .filter((g) => g.median_executions_per_second !== null)
    .map((g) => `${formatRate(g.median_executions_per_second as number)} at ${g.clients} clients`);
  const rejections = groups.map((g) => ({ clients: g.clients, n: g.rounds.reduce((a, r) => a + r.rejections, 0), f: g.rounds.reduce((a, r) => a + r.failures, 0) }));
  const withRejections = rejections.filter((r) => r.n > 0 || r.f > 0);

  return (
    <>
      <div className={styles.head}>
        <h3 className={styles.title}>
          {figure.suite} under {clients} concurrent clients,<br />on {figure.engine}.
        </h3>
        <p className={styles.method}>
          Each client runs the {figure.suite} statements back to back for {figure.duration_seconds} seconds after a {figure.warmup_seconds}-second
          warm-up, on {cluster.workers} {cluster.node_sku} worker nodes with {cluster.memory_per_query_per_worker} per query per worker.
          The rate is successful exact executions across all clients divided by the measured span: the window plus the time the
          statements in flight at its end take to return. A statement refused by memory admission is retried, and the time the
          refusals take counts against the rate.
        </p>
      </div>
      <figure className={styles.card}>
        <ThroughputFigure figure={figure} />
        <figcaption className={styles.caption}>
          {medians.length > 0 ? <>Median over rounds: <strong>{medians.join("; ")}</strong>, from {rounds}.</> : <>No round produced a rate.</>}
          {withRejections.length > 0
            ? <> Across the rounds, {list(withRejections.map((r) => `${r.clients} clients saw ${plural(r.n, "admission rejection")} and ${plural(r.f, "failure")}`))}.</>
            : null}
        </figcaption>
      </figure>
      <dl className={styles.facts}>
        <Fact label="Window">{figure.duration_seconds} s after a {figure.warmup_seconds} s warm-up</Fact>
        <Fact label="Clients">{clients}</Fact>
        <Fact label="Rounds">{rounds}</Fact>
        <CommonFacts cluster={cluster} date={figure.date} digests={figure.engine_digests} recordPath={figure.record_path} />
      </dl>
    </>
  );
}

/**
 * The benchmark section of the product page: one figure at a time, chosen
 * with a segmented control — a suite's per-statement times, or the
 * concurrency figure — each with the method it was measured under, the
 * counts the figure itself draws, and the record it was taken from. A tab is
 * offered only when its generated file exists; every number comes from the
 * files.
 */
export function BenchmarkSection({ figures }: { figures: BenchmarkFigures }) {
  const tabs: Tab[] = [];
  if (figures.clickbench) {
    const f = figures.clickbench;
    tabs.push({ id: "clickbench", title: f.title, desc: `${plural(f.summary.statements, "statement")}, ${plural(f.rounds, "cold round")}` });
  }
  if (figures.tpch) {
    const f = figures.tpch;
    tabs.push({ id: "tpch", title: f.title, desc: `${plural(f.summary.statements, "query")}, ${plural(f.rounds, "cold round")}` });
  }
  if (figures.throughput) {
    const f = figures.throughput;
    tabs.push({ id: "throughput", title: "Concurrency", desc: `${list(f.groups.map((g) => g.clients))} clients, ${f.duration_seconds} s windows` });
  }
  const [active, setActive] = useState<TabId>(tabs[0]?.id ?? "clickbench");
  const baseId = useId();
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  if (tabs.length === 0) return null;
  const current = tabs.some((t) => t.id === active) ? active : tabs[0].id;

  const onTabKey = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const last = tabs.length - 1;
    let next: number | null = null;
    if (event.key === "ArrowRight" || event.key === "ArrowDown") next = index === last ? 0 : index + 1;
    else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = index === 0 ? last : index - 1;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = last;
    if (next === null) return;
    event.preventDefault();
    setActive(tabs[next].id);
    tabRefs.current[next]?.focus();
  };

  return (
    <section id="benchmark" className={styles.section} aria-labelledby={`${baseId}-label`}>
      <div className={styles.inner}>
        <h2 id={`${baseId}-label`} className={styles.srOnly}>Benchmarks</h2>
        {tabs.length > 1 && (
          <div className={styles.tabs} role="tablist" aria-label="Benchmark figures">
            {tabs.map((tab, i) => {
              const selected = tab.id === current;
              return (
                <button
                  key={tab.id}
                  ref={(el) => { tabRefs.current[i] = el; }}
                  type="button"
                  role="tab"
                  id={`${baseId}-tab-${tab.id}`}
                  aria-selected={selected}
                  aria-controls={`${baseId}-panel-${tab.id}`}
                  tabIndex={selected ? 0 : -1}
                  className={`${styles.tab} ${selected ? styles.tabSelected : ""}`}
                  onClick={() => setActive(tab.id)}
                  onKeyDown={(event) => onTabKey(event, i)}
                >
                  <span className={styles.tabDot} aria-hidden="true" />
                  <span className={styles.tabText}>
                    <span className={styles.tabTitle}>{tab.title}</span>
                    <span className={styles.tabDesc}>{tab.desc}</span>
                  </span>
                </button>
              );
            })}
          </div>
        )}
        {tabs.map((tab) => (
          <div
            key={tab.id}
            role={tabs.length > 1 ? "tabpanel" : undefined}
            id={`${baseId}-panel-${tab.id}`}
            aria-labelledby={tabs.length > 1 ? `${baseId}-tab-${tab.id}` : undefined}
            hidden={tab.id !== current}
          >
            {tab.id === current && (
              tab.id === "throughput"
                ? <ThroughputPanel figure={figures.throughput as ThroughputData} />
                : <LatencyPanel figure={(tab.id === "tpch" ? figures.tpch : figures.clickbench) as LatencyFigure} />
            )}
          </div>
        ))}
      </div>
    </section>
  );
}
