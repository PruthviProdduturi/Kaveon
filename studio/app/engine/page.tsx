"use client";

import Link from "next/link";
import { KaveonMark } from "../../components/KaveonMark";
import { LastKnown, UnknownPanel, reasonFromStatus } from "../../components/Reading";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import s from "./engine.module.css";
import {
  Cluster, EngineUnavailable, QueryRecord,
  absoluteTime, bytes, clientLabel, errorExcerpt, fetchCluster, fetchQueries,
  firstLine, inProgress, ms, relativeTime, shortUser, uptime, userLabel,
} from "./lib";

const POLL_MS = 5000;
const MAX_SAMPLES = 48;
type Filter = "ALL" | "RUNNING" | "FINISHED" | "FAILED" | "CANCELED";
/** The filter a record falls under: queued counts as running.
 *
 * A cancellation used to count as a failure, which made the Failed tile
 * disagree with the rows underneath it -- the tile called a cancelled query
 * failed while the row rendered it as a success. It is neither. */
const bucket = (q: QueryRecord): Filter =>
  inProgress(q.state) ? "RUNNING"
    : q.state === "FINISHED" ? "FINISHED"
    : q.state === "CANCELED" ? "CANCELED"
    : "FAILED";

function Sparkline({ values }: { values: number[] }) {
  if (values.length < 2) return <svg className={s.cellSpark} viewBox="0 0 100 26" preserveAspectRatio="none" aria-hidden="true" />;
  const max = Math.max(...values, 1), min = Math.min(...values);
  const span = Math.max(max - min, 1);
  const step = 100 / (values.length - 1);
  const pts = values.map((v, i) => `${(i * step).toFixed(1)},${(24 - ((v - min) / span) * 20).toFixed(1)}`).join(" ");
  return (
    <svg className={s.cellSpark} viewBox="0 0 100 26" preserveAspectRatio="none" aria-hidden="true">
      <polyline points={pts} />
    </svg>
  );
}

export default function EngineConsolePage() {
  const [cluster, setCluster] = useState<Cluster | null>(null);
  const [queries, setQueries] = useState<QueryRecord[] | null>(null);
  const [error, setError] = useState<EngineUnavailable | null>(null);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [filter, setFilter] = useState<Filter>("ALL");
  const [search, setSearch] = useState("");
  const memory = useRef<number[]>([]);
  const inflight = useRef(false);

  const refresh = useCallback(async () => {
    if (inflight.current) return;
    inflight.current = true;
    try {
      const [c, q] = await Promise.all([fetchCluster(), fetchQueries()]);
      const rss = [c.coordinator, ...c.workers].reduce((sum, n) => sum + (n.memory_rss_bytes || 0), 0);
      memory.current = [...memory.current, rss].slice(-MAX_SAMPLES);
      setCluster(c); setQueries(q); setError(null);
      const t = Date.now(); setUpdatedAt(t); setNow(t);
    } catch (e) {
      setError(e instanceof EngineUnavailable ? e : new EngineUnavailable(0, "The console could not reach the server."));
    } finally {
      inflight.current = false;
    }
  }, []);

  // Poll while the tab is visible; a hidden tab wastes Engine and API cycles.
  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | null = null;
    const start = () => { if (!timer) { refresh(); timer = setInterval(refresh, POLL_MS); } };
    const stop = () => { if (timer) { clearInterval(timer); timer = null; } };
    const onVisibility = () => (document.hidden ? stop() : start());
    start();
    document.addEventListener("visibilitychange", onVisibility);
    return () => { stop(); document.removeEventListener("visibilitychange", onVisibility); };
  }, [refresh]);

  const counts = useMemo(() => {
    const c = { ALL: 0, RUNNING: 0, FINISHED: 0, FAILED: 0 } as Record<Filter, number>;
    for (const q of queries || []) { c.ALL++; c[bucket(q)]++; }
    return c;
  }, [queries]);

  const queued = cluster?.coordinator.admission?.queue_depth || 0;

  const visible = useMemo(() => {
    const term = search.trim().toLowerCase();
    return (queries || []).filter(q => {
      if (filter !== "ALL" && bucket(q) !== filter) return false;
      if (!term) return true;
      return [q.id, q.sql, userLabel(q), clientLabel(q), q.context?.catalog, q.context?.schema, q.error]
        .some(v => (v || "").toLowerCase().includes(term));
    });
  }, [queries, filter, search]);

  const slowest = useMemo(() => Math.max(1, ...visible.map(q => q.elapsed_ms || 0)), [visible]);
  const rss = memory.current[memory.current.length - 1];
  const connected = !!cluster && !error;

  return (
    <div className={`page-shell ${s.root}`}>
      <header className={s.mast}>
        <div className={s.mastLeft}>
          <div className={s.mark} aria-hidden="true"><KaveonMark size={26} /></div>
          <div style={{ minWidth: 0 }}>
            <h1 className={s.title}>KaveonDB</h1>
            <p className={s.sub}>
              {cluster
                ? <>Environment <b>{cluster.environment}</b> · coordinator v{cluster.coordinator.version} · up {uptime(cluster.coordinator.uptime_secs)}</>
                : "Read-only view of the KaveonDB cluster and its query history."}
            </p>
          </div>
        </div>
        <div className={s.mastRight}>
          <span className={s.live} aria-live="polite">
            <span className={`${s.dot} ${connected ? "" : s.dotOff}`} />
            {/* Three states in the one label a reader glances at: live, not
                answering over a reading already in hand, and never reached. */}
            {connected ? (updatedAt ? `Live · updated ${relativeTime(updatedAt, now)}` : "Live")
              : updatedAt ? `Not answering · last reading ${relativeTime(updatedAt, now)}`
              : "Not connected"}
          </span>
          <button type="button" className={s.iconBtn} onClick={refresh}><i className="fas fa-rotate" aria-hidden="true" /> Refresh</button>
        </div>
      </header>

      {/* A poll that failed over a console that has already read once is the
          stale case, not the absent one: the figures below are a real
          reading, they have simply stopped advancing. Clearing them, or
          leaving them unlabelled as live, were both wrong — so they stay and
          they are labelled. Only a console that never read reports the
          failure as the whole of what it knows. */}
      {error && (cluster || queries) && (
        <LastKnown
          what="KaveonDB"
          taken={updatedAt ? relativeTime(updatedAt, now) : null}
          detail={error.status === 503
            ? "The server reports no KaveonDB endpoint configured."
            : `${error.message} Nothing below has been taken away.`}
          onRetry={refresh}
        />
      )}

      {error && !cluster && !queries && (
        error.status === 503 ? (
          <div className={`${s.state} ${s.stateErr}`} role="alert">
            <div className={s.stateTitle}>KaveonDB is not configured on this server</div>
            <div className={s.stateBody}>
              {error.message} The server needs <code>KAVEON_ENGINE_URL</code> and its bridge credential. Configure them under System settings.
            </div>
          </div>
        ) : (
          <UnknownPanel
            what="KaveonDB" says="what its cluster holds or what it has run"
            reason={reasonFromStatus(error.status)} detail={error.message}
            onRetry={refresh}
          />
        )
      )}

      {cluster && (
        <section className={s.strip} aria-label="Cluster status">
          <div className={s.cell}>
            <div className={s.cellLabel}>Workers</div>
            <div className={s.cellValue}>{cluster.active_workers}<small>of {cluster.total_nodes} nodes</small></div>
          </div>
          <div className={s.cell} title="Resident memory across the coordinator and every active worker">
            <div className={s.cellLabel}>Memory</div>
            <div className={s.cellValue}>{bytes(rss)}</div>
            <Sparkline values={memory.current} />
          </div>
          <button type="button" className={`${s.cell} ${s.cellBtn}`} aria-pressed={filter === "ALL"} onClick={() => setFilter("ALL")}>
            <div className={s.cellLabel}>Queries in history</div>
            <div className={s.cellValue}>{counts.ALL}<small>{counts.RUNNING ? `${counts.RUNNING} running` : "none running"}{queued ? ` · ${queued} queued for memory` : ""}</small></div>
          </button>
          <button type="button" className={`${s.cell} ${s.cellBtn} ${s.cellFailed}`} aria-pressed={filter === "FAILED"} onClick={() => setFilter(f => f === "FAILED" ? "ALL" : "FAILED")}>
            <div className={s.cellLabel}>Failed</div>
            <div className={`${s.cellValue} ${counts.FAILED ? s.failed : s.zero}`}>
              {counts.FAILED}
              <small>{counts.ALL ? `${Math.round((counts.FAILED / counts.ALL) * 100)}% of history` : ""}</small>
            </div>
          </button>
        </section>
      )}

      {queries && (
        <>
          <div className={s.toolbar}>
            <input
              className={s.search} type="search" value={search} onChange={e => setSearch(e.target.value)}
              placeholder="Search SQL, query ID, user, client, catalog or error" aria-label="Search query history"
            />
            <div className={s.seg} role="group" aria-label="Filter by state">
              {(["ALL", "RUNNING", "FINISHED", "FAILED"] as Filter[]).map(f => (
                <button key={f} type="button" className={s.segBtn} data-state={f} aria-pressed={filter === f} onClick={() => setFilter(f)}>
                  {f === "ALL" ? "All" : f[0] + f.slice(1).toLowerCase()}
                  <span className={s.segCount}>{counts[f]}</span>
                </button>
              ))}
            </div>
            {visible.length !== counts.ALL && <span className={s.meta}>{visible.length} of {counts.ALL}</span>}
          </div>

          {visible.length === 0 ? (
            <div className={s.state}>
              <div className={s.stateTitle}>{counts.ALL ? "No queries match" : "No queries yet"}</div>
              <div className={s.stateBody}>
                {counts.ALL ? "Clear the search or choose a different state." : "Queries run through Studio, the CLI, or the HTTP API appear here as KaveonDB records them."}
              </div>
            </div>
          ) : (
            <div className={s.table}>
              <div className={s.head} aria-hidden="true">
                <span /><span>Submitted</span><span>Query</span><span>Client · user</span><span>Duration</span><span>Output</span>
              </div>
              {visible.map(q => {
                // Cancelling is something a person chose to do, not a fault,
                // so it is its own state: grey rather than red, and never the
                // success styling it used to fall through to.
                const canceled = q.state === "CANCELED";
                const failed = q.state === "FAILED", running = inProgress(q.state);
                const pct = Math.max(2, Math.round(((q.elapsed_ms || 0) / slowest) * 100));
                return (
                  <Link key={q.id} href={`/engine/queries/${encodeURIComponent(q.id)}`}
                    className={`${s.row} ${failed ? s.rowFailed : ""} ${running ? s.rowRunning : ""} ${canceled ? s.rowCanceled : ""}`}
                    aria-label={`${q.state.toLowerCase()} query ${q.id}`}>
                    <span className={`${s.glyph} ${failed ? s.glyphFailed : ""} ${running ? s.glyphRunning : ""} ${canceled ? s.glyphCanceled : !failed && !running ? s.glyphFinished : ""}`} />
                    <span className={s.time} title={absoluteTime(q.submitted_at_ms)}>{relativeTime(q.submitted_at_ms, now)}</span>
                    <span className={s.sqlWrap}>
                      <span className={s.sql}>{firstLine(q.sql)}</span>
                      {failed && q.error && <span className={s.err}>{errorExcerpt(q.error)}</span>}
                    </span>
                    <span className={s.who} title={userLabel(q)}>{clientLabel(q)} <span>· {shortUser(userLabel(q))}</span></span>
                    <span className={s.dur}>
                      <span className={s.durVal}>{ms(q.elapsed_ms)}</span>
                      <span className={s.bar} aria-hidden="true"><span className={s.barFill} style={{ width: `${pct}%` }} /></span>
                    </span>
                    <span className={s.out}>
                      {running ? <span>{q.state === "QUEUED" ? "queued" : "running"}</span>
                        : canceled ? <span>cancelled</span>
                        : failed ? <span>—</span>
                        : <>{(q.row_count ?? q.rows?.length ?? 0).toLocaleString()} <span>{(q.row_count ?? q.rows?.length ?? 0) === 1 ? "row" : "rows"}</span>{(q.stage_count ?? q.stages?.length ?? 0) ? <> <span>· {q.stage_count ?? q.stages?.length} {(q.stage_count ?? q.stages?.length) === 1 ? "stage" : "stages"}</span></> : null}</>}
                    </span>
                  </Link>
                );
              })}
            </div>
          )}
        </>
      )}

      {!cluster && !error && (
        <div className={s.state}><div className={s.stateTitle}>Connecting to KaveonDB…</div></div>
      )}
    </div>
  );
}
