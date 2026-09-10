"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useCallback, useEffect, useState } from "react";
import s from "../../engine.module.css";
import {
  EngineUnavailable, PlanNode, QueryRecord,
  absoluteTime, bytes, clientLabel, fetchQuery, ms, ns, rate, us, userLabel,
} from "../../lib";

type Tab = "overview" | "plan" | "stages" | "results" | "raw";
const TABS: { id: Tab; label: string }[] = [
  { id: "overview", label: "Overview" }, { id: "plan", label: "Plan & storage" },
  { id: "stages", label: "Stages" }, { id: "results", label: "Results" }, { id: "raw", label: "Raw record" },
];

const Value = ({ v, fallback = "Not provided" }: { v?: unknown; fallback?: string }) =>
  v === undefined || v === null || v === "" ? <span className={s.na}>{fallback}</span> : <>{String(v)}</>;

function Definitions({ rows }: { rows: [string, React.ReactNode][] }) {
  return <dl className={s.dl}>{rows.map(([k, v]) => <div key={k} style={{ display: "contents" }}><dt>{k}</dt><dd>{v}</dd></div>)}</dl>;
}

function Timeline({ q }: { q: QueryRecord }) {
  const t = q.timings || {};
  const parts = [
    { key: "Analysis", v: t.analysis_us, color: "var(--text-faint)" },
    { key: "Planning", v: t.planning_us, color: "var(--text-muted)" },
    { key: "Execution", v: t.execution_us, color: "var(--accent)" },
    { key: "Serialization", v: t.result_serialization_us, color: "var(--text-secondary)" },
  ];
  const measured = parts.filter(p => p.v != null);
  const total = measured.reduce((sum, p) => sum + (p.v as number), 0);
  if (!measured.length) return <div className={s.note}><b>Not measured.</b> KaveonDB did not attach phase timings to this query.</div>;
  return (
    <>
      <div className={s.tl} role="img" aria-label={parts.map(p => `${p.key} ${us(p.v)}`).join(", ")}>
        {measured.map(p => <span key={p.key} className={s.tlSeg} style={{ width: `${Math.max(1, ((p.v as number) / Math.max(total, 1)) * 100)}%`, background: p.color }} />)}
      </div>
      <div className={s.tlLegend}>
        {parts.map(p => <span key={p.key} className={s.tlKey}><i style={{ background: p.color }} />{p.key} <b>{us(p.v)}</b></span>)}
      </div>
    </>
  );
}

function Plan({ node }: { node: PlanNode }) {
  const attrs = node.attributes && typeof node.attributes === "object" ? Object.entries(node.attributes) : [];
  return (
    <li>
      <div className={s.node}>
        <span className={s.nodeOp}><Value v={node.operator} fallback="Unknown operator" /></span>
        <span className={s.nodePhase}><Value v={node.phase} fallback="plan" /></span>
        {attrs.length > 0 && <div className={s.nodeAttrs}>{attrs.map(([k, v]) => <span key={k} className={s.nodeAttr}>{k}: {String(v)}</span>)}</div>}
      </div>
      {node.children?.length ? <ul>{node.children.map((c, i) => <Plan key={c.id || i} node={c} />)}</ul> : null}
    </li>
  );
}

export default function EngineQueryPage() {
  const { id } = useParams<{ id: string }>();
  const queryId = decodeURIComponent(id);
  const [q, setQ] = useState<QueryRecord | null>(null);
  const [error, setError] = useState<EngineUnavailable | null>(null);
  const [tab, setTab] = useState<Tab>("overview");
  const [copied, setCopied] = useState("");

  const load = useCallback(async () => {
    try { setQ(await fetchQuery(queryId)); setError(null); }
    catch (e) { setError(e instanceof EngineUnavailable ? e : new EngineUnavailable(0, "The console could not reach the server.")); }
  }, [queryId]);

  useEffect(() => { load(); }, [load]);
  useEffect(() => {
    if (q?.state !== "RUNNING") return;
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, [q?.state, load]);

  const copy = async (text: string, label: string) => {
    try { await navigator.clipboard.writeText(text); setCopied(`${label} copied`); }
    catch { setCopied("Copy unavailable"); }
    setTimeout(() => setCopied(""), 1800);
  };

  const c = q?.context || {};
  const stateClass = q?.state === "FAILED" ? s.pillFailed : q?.state === "RUNNING" ? s.pillRunning : "";

  return (
    <div className={`page-shell ${s.root}`}>
      <Link href="/engine" className={s.back}><i className="fas fa-arrow-left" aria-hidden="true" /> All queries</Link>

      {error && (
        <div className={`${s.state} ${s.stateErr}`} role="alert">
          <div className={s.stateTitle}>{error.status === 404 ? "Query not found" : "KaveonDB is unavailable"}</div>
          <div className={s.stateBody}>{error.message}</div>
        </div>
      )}

      {q && (
        <>
          <header className={s.dHead}>
            <div style={{ minWidth: 0 }}>
              <h1 className={s.dId}>{q.id}</h1>
              <p className={s.dMeta}>
                Submitted <b>{absoluteTime(q.submitted_at_ms)}</b>
                {q.completed_at_ms ? <> · completed <b>{absoluteTime(q.completed_at_ms)}</b></> : <> · in progress</>}
                {" "}· {clientLabel(q)} · <b>{userLabel(q)}</b>
              </p>
            </div>
            <div className={s.dActions}>
              <span className={`${s.pill} ${stateClass}`}>{q.state[0] + q.state.slice(1).toLowerCase()} · {ms(q.elapsed_ms)}</span>
              <button type="button" className={s.iconBtn} onClick={() => copy(q.id, "ID")}>Copy ID</button>
              <button type="button" className={s.iconBtn} onClick={() => copy(q.sql, "SQL")}>Copy SQL</button>
              <span className={s.copied} role="status" aria-live="polite">{copied}</span>
            </div>
          </header>

          <div className={s.tabs} role="tablist" aria-label="Query details">
            {TABS.map(t => (
              <button key={t.id} type="button" role="tab" className={s.tab} aria-selected={tab === t.id} onClick={() => setTab(t.id)}>{t.label}</button>
            ))}
          </div>

          {tab === "overview" && (
            <div role="tabpanel">
              <section className={s.panel}>
                <h2 className={s.panelTitle}>Query</h2>
                <pre className={s.code}>{q.sql}</pre>
              </section>
              {q.error && (
                <section className={s.panel}>
                  <h2 className={s.panelTitle}>Error</h2>
                  <pre className={`${s.code} ${s.codeErr}`}>{q.error}</pre>
                </section>
              )}
              <section className={s.panel}>
                <h2 className={s.panelTitle}>Where the time went</h2>
                <Timeline q={q} />
              </section>
              <div className={s.two}>
                <section className={s.panel}>
                  <h2 className={s.panelTitle}>Session</h2>
                  <Definitions rows={[
                    ["User", userLabel(q)], ["Principal", <Value key="p" v={c.principal} />],
                    ["Client", clientLabel(q)], ["Source", <Value key="s" v={c.source} />],
                    ["Catalog", <Value key="c" v={c.catalog} />], ["Schema", <Value key="sc" v={c.schema} />],
                    ["Catalog snapshot", <Value key="cs" v={c.catalog_snapshot_id} />],
                    ["Time zone", <Value key="tz" v={c.time_zone} />], ["Client address", <Value key="ca" v={c.client_address} />],
                    ["Client tags", <Value key="ct" v={c.client_tags?.length ? c.client_tags.join(", ") : null} />],
                    ["Result delivery", <Value key="rd" v={c.result_delivery} />],
                  ]} />
                </section>
                <section className={s.panel}>
                  <h2 className={s.panelTitle}>Execution</h2>
                  <Definitions rows={[
                    ["State", q.state], ["Elapsed", ms(q.elapsed_ms)],
                    ["Rows in response", `${q.rows.length.toLocaleString()}${q.rows_are_preview ? " (preview)" : ""}`],
                    ["Columns", String(q.columns.length)],
                    ["Distributed stages", q.stages.length ? String(q.stages.length) : <span key="ns" className={s.na}>Node-local</span>],
                    ["KaveonDB version", <Value key="ev" v={c.engine_version} />], ["Environment", <Value key="en" v={c.environment} />],
                  ]} />
                  {q.columns.length > 0 && (
                    <>
                      <h2 className={s.panelTitle} style={{ marginTop: 16 }}>Schema</h2>
                      <div className={s.chips}>{q.columns.map(col => <span key={col.name} className={s.chip}>{col.name}<span>{col.type}</span></span>)}</div>
                    </>
                  )}
                </section>
              </div>
            </div>
          )}

          {tab === "plan" && (
            <div role="tabpanel">
              <section className={s.panel}>
                <h2 className={s.panelTitle}>Logical plan</h2>
                {typeof q.plan?.logical === "string" ? <pre className={s.code}>{q.plan.logical}</pre>
                  : q.plan?.logical ? <ul className={s.tree}><Plan node={q.plan.logical} /></ul>
                  : <div className={s.note}><b>Not available.</b> The planner did not attach a logical plan.</div>}
              </section>
              <section className={s.panel}>
                <h2 className={s.panelTitle}>Storage scans</h2>
                {q.scans.length ? (
                  <>
                    {!q.scan_metrics_complete && <div className={s.note}><b>Partial.</b> One or more workers did not return reader counters, so no cluster-wide scan total is shown.</div>}
                    {q.scans.map((sc, i) => (
                      <div key={i} className={s.metrics} style={{ marginBottom: i < q.scans.length - 1 ? 12 : 0 }}>
                        {[
                          ["Files opened", `${sc.files_opened} / ${sc.files_considered}`],
                          ["Row groups read", `${sc.row_groups_read} / ${sc.row_groups_considered}`],
                          ["Row groups pruned", String(sc.row_groups_pruned)],
                          ["Rows emitted", sc.rows_emitted.toLocaleString()],
                          ["Rows in selected groups", sc.rows_selected.toLocaleString()],
                          ["Compressed data read", bytes(sc.compressed_bytes_selected)],
                          ["Read throughput", rate(sc.compressed_bytes_per_second, "B/s")],
                          ["Row throughput", rate(sc.rows_per_second, "rows/s")],
                          ["Delta snapshot", ns(sc.snapshot_ns)], ["Parquet footers", ns(sc.footer_ns)], ["Read and decode", ns(sc.read_ns)],
                        ].map(([k, v]) => <div key={k} className={s.metric}><div className={s.metricLabel}>{k}</div><div className={s.metricValue}>{v}</div></div>)}
                      </div>
                    ))}
                  </>
                ) : (
                  <div className={s.note}>
                    <b>{q.stages.length ? "Reader counters unavailable." : "Not measured."}</b>{" "}
                    {q.stages.length ? "This distributed query did not receive complete storage-scan metrics from every worker." : "No storage scan was attached to this query."}
                  </div>
                )}
              </section>
              <section className={s.panel}>
                <h2 className={s.panelTitle}>Operator metrics</h2>
                <div className={s.note}><b>Not measured.</b> Per-operator CPU, memory, blocked time and spill counters are not yet emitted by KaveonDB.</div>
              </section>
            </div>
          )}

          {tab === "stages" && (
            <div role="tabpanel">
              {q.stages.length ? q.stages.map(st => (
                <section key={st.stage_id} className={s.panel}>
                  <h2 className={s.panelTitle}>Stage {st.stage_id} · {st.state} · {st.completed_tasks} / {st.task_count} tasks · {us(st.elapsed_us)}</h2>
                  <div className={s.grid}>
                    <table>
                      <thead><tr><th>Task</th><th>Worker</th><th>Partition</th><th>Elapsed</th><th>Rows</th><th>Batches</th><th>Arrow bytes</th></tr></thead>
                      <tbody>{st.tasks.map(t => (
                        <tr key={t.task_id}><td>{t.task_id}</td><td>{t.node_id}</td><td>{t.partition_index}</td><td>{us(t.elapsed_us)}</td><td>{t.output_rows.toLocaleString()}</td><td>{t.output_batches.toLocaleString()}</td><td>{bytes(t.output_bytes)}</td></tr>
                      ))}</tbody>
                    </table>
                  </div>
                </section>
              )) : <div className={s.note}><b>Node-local execution.</b> No distributed stage was scheduled for this query.</div>}
            </div>
          )}

          {tab === "results" && (
            <div role="tabpanel">
              {q.rows.length && q.columns.length ? (
                <section className={s.panel}>
                  {q.rows_are_preview && <div className={s.note}><b>Preview.</b> KaveonDB retained only the first rows of this result.</div>}
                  <div className={s.grid}>
                    <table>
                      <thead><tr>{q.columns.map(col => <th key={col.name}>{col.name}</th>)}</tr></thead>
                      <tbody>{q.rows.map((row, i) => (
                        <tr key={i}>{row.map((cell, j) => cell === null ? <td key={j} className={s.null}>NULL</td> : <td key={j}>{String(cell)}</td>)}</tr>
                      ))}</tbody>
                    </table>
                  </div>
                </section>
              ) : (
                <div className={s.state}>
                  <div className={s.stateTitle}>No result rows</div>
                  <div className={s.stateBody}>
                    {q.state === "RUNNING" ? "Rows appear when execution completes." : q.state === "FAILED" ? "The query failed before producing a result." : "The query completed and returned no rows."}
                  </div>
                </div>
              )}
            </div>
          )}

          {tab === "raw" && (
            <div role="tabpanel"><pre className={s.code} style={{ maxHeight: 560 }}>{JSON.stringify(q, null, 2)}</pre></div>
          )}
        </>
      )}

      {!q && !error && <div className={s.state}><div className={s.stateTitle}>Loading query…</div></div>}
    </div>
  );
}
