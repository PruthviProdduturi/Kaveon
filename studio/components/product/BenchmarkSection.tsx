"use client";

import type { ClickBenchFigure } from "../../utils/clickbench";
import { BenchmarkFigure, formatSeconds } from "./BenchmarkFigure";
import styles from "./BenchmarkSection.module.css";

const REPO_TREE = "https://github.com/PruthviProdduturi/Kaveon/tree/dev/";

function longDate(iso: string): string {
  const date = new Date(`${iso}T00:00:00Z`);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" });
}

function shortDigest(digest: string): string {
  return digest.replace(/^sha256:/, "").slice(0, 12);
}

/**
 * The benchmark section of the product page: the ClickBench figure with the
 * method it was measured under, the counts the figure itself draws, and the
 * record it was taken from. Every number comes from the generated file.
 */
export function BenchmarkSection({ figure }: { figure: ClickBenchFigure }) {
  const { summary, cluster } = figure;
  const rows = figure.rows === null ? null : figure.rows.toLocaleString("en-US");
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  const recordUrl = REPO_TREE + figure.record_path.replace(/^\/+/, "");

  return (
    <section id="benchmark" className={styles.section} aria-labelledby="benchmark-title">
      <div className={styles.inner}>
        <div className={styles.head}>
          <h2 id="benchmark-title" className={styles.title}>
            All {summary.statements} ClickBench queries,<br />timed on {figure.engine}.
          </h2>
          <p className={styles.method}>
            The {summary.statements} upstream ClickBench statements{rows ? <> over <strong>{rows} rows</strong> of the public <code>{figure.table}</code> table</> : null},
            on {cluster.workers} {cluster.node_sku} worker nodes with {cluster.memory_per_query_per_worker} per query per worker.
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
            {summary.ran_every_round < summary.statements
              ? <> {plural(summary.statements - summary.ran_every_round, "statement")} did not finish in every round and {summary.statements - summary.ran_every_round === 1 ? "has" : "have"} no figure.</>
              : null}
          </figcaption>
        </figure>

        <dl className={styles.facts}>
          {rows && (
            <div className={styles.fact}>
              <dt>Rows scanned</dt>
              <dd>{rows}</dd>
            </div>
          )}
          <div className={styles.fact}>
            <dt>Cluster</dt>
            <dd>{cluster.workers} × {cluster.node_sku} workers, {cluster.memory_per_query_per_worker} per query per worker</dd>
          </div>
          <div className={styles.fact}>
            <dt>Rounds</dt>
            <dd>{figure.rounds}, cold, {figure.executions_per_round} executions each</dd>
          </div>
          <div className={styles.fact}>
            <dt>Recorded</dt>
            <dd>{longDate(figure.date)}</dd>
          </div>
          <div className={styles.fact}>
            <dt>{figure.engine_digests.length === 1 ? "Engine build" : "Engine builds"}</dt>
            <dd>
              {figure.engine_digests.length === 0
                ? "—"
                : figure.engine_digests.map((d, i) => (
                  <span key={d} className={styles.digest} title={d}>{i > 0 ? ", " : ""}{shortDigest(d)}</span>
                ))}
            </dd>
          </div>
          <div className={styles.fact}>
            <dt>Record</dt>
            <dd><a href={recordUrl} target="_blank" rel="noopener noreferrer">Round records on GitHub</a></dd>
          </div>
        </dl>
      </div>
    </section>
  );
}
