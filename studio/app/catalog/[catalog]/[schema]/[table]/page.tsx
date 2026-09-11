"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useEffect, useState } from "react";
import { useRole } from "../../../../../hooks/useRole";
import { useCatalogTree } from "../../../CatalogShell";
import s from "../../../catalog.module.css";
import {
  CatalogError, Sample, TableDef, TableStatistic, Usage,
  fetchSample, fetchStatistic, fetchTable, fetchUsage, labHref, shortLocation,
} from "../../../lib";

const fmt = (n: number) => n.toLocaleString();
const when = (iso: string | null) => iso ? new Date(iso).toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" }) : null;

export default function CatalogTablePage() {
  const p = useParams<{ catalog: string; schema: string; table: string }>();
  const catalog = decodeURIComponent(p.catalog), schema = decodeURIComponent(p.schema), table = decodeURIComponent(p.table);
  const { isAnalyst, isAdmin } = useRole();
  const { catalogs, sourceFor } = useCatalogTree();
  const source = sourceFor(catalog)?.id ?? null;
  const fullName = `${catalog}.${schema}.${table}`;

  const [def, setDef] = useState<TableDef | null>(null);
  const [defError, setDefError] = useState<CatalogError | null>(null);
  const [usage, setUsage] = useState<Usage | null>(null);
  const [stat, setStat] = useState<TableStatistic | null | undefined>(undefined);
  const [sample, setSample] = useState<Sample | null>(null);
  const [sampleError, setSampleError] = useState<CatalogError | null>(null);
  const [sampleLoading, setSampleLoading] = useState(false);
  const [copied, setCopied] = useState("");

  useEffect(() => {
    let cancelled = false;
    setDef(null); setDefError(null); setUsage(null); setStat(undefined); setSample(null); setSampleError(null);
    if (!source) {
      if (catalogs) setDefError(new CatalogError(404, `${catalog} is not a registered catalog.`));
      return;
    }
    fetchTable(source, schema, table)
      .then(d => { if (!cancelled) setDef(d); })
      .catch(e => { if (!cancelled) setDefError(e instanceof CatalogError ? e : new CatalogError(0, "The definition could not be read.")); });
    fetchUsage(source, schema, table)
      .then(u => { if (!cancelled) setUsage(u); })
      .catch(() => { if (!cancelled) setUsage({ datasets: [], charts: [], dashboards: [], dlm: [] }); });
    if (isAdmin) fetchStatistic(fullName).then(st => { if (!cancelled) setStat(st); }).catch(() => { if (!cancelled) setStat(null); });
    else setStat(null);
    return () => { cancelled = true; };
  }, [source, catalogs, catalog, schema, table, fullName, isAdmin]);

  const loadSample = async () => {
    if (!source) return;
    setSampleLoading(true); setSampleError(null);
    try { setSample(await fetchSample(source, schema, table)); }
    catch (e) { setSampleError(e instanceof CatalogError ? e : new CatalogError(0, "The sample could not be read.")); }
    finally { setSampleLoading(false); }
  };

  const copy = async () => {
    try { await navigator.clipboard.writeText(fullName); setCopied("Copied"); } catch { setCopied("Copy unavailable"); }
    setTimeout(() => setCopied(""), 1600);
  };

  const crumbs = (
    <div className={s.crumbs}>
      <Link href="/catalog">Catalog</Link><i>›</i><b>{catalog}</b><i>›</i>
      <Link href={`/catalog/${encodeURIComponent(catalog)}/${encodeURIComponent(schema)}`}>{schema}</Link><i>›</i><b>{table}</b>
    </div>
  );

  if (defError) {
    return <>{crumbs}<div className={`${s.note} ${s.noteErr}`} role="alert"><b>{defError.status === 404 ? "Not in the catalog." : "Definition unavailable."}</b> {defError.message}</div></>;
  }
  if (!def) return <>{crumbs}<div className={s.note}>Reading definition…</div></>;

  const loc = def.location ? shortLocation(def.location) : null;
  const dlmReady = usage?.dlm.find(d => d.status === "ready") ?? null;
  const askDataset = dlmReady ? Number(dlmReady.datasetId) : usage?.datasets[0]?.id ?? null;
  const dlmRows = dlmReady?.rowCount ?? null;
  // Row count: KaveonDB statistics when the reader may see them, else the DLM's exact count from its last build.
  const rows = stat ? { n: stat.row_count, note: stat.current ? "current" : "stale", cls: stat.current ? s.ok : s.warn }
    : dlmRows != null ? { n: dlmRows, note: dlmReady?.rowCountSource === "kaveon_engine_exact" ? "at last DLM build" : "estimated", cls: "" }
    : null;
  const usedCount = usage ? usage.datasets.length + usage.charts.length + usage.dashboards.length : null;

  return (
    <>
      {crumbs}

      <header className={s.head}>
        <div style={{ minWidth: 0 }}>
          <h1 className={s.title}>{def.name}</h1>
          <p className={s.subtitle}>{fullName}{def.revision != null ? ` · definition revision ${def.revision}` : ""}{def.lifecycle && def.lifecycle !== "active" ? ` · ${def.lifecycle}` : ""}</p>
          <div className={s.chips}>
            {def.format && <span className={s.chip}>{def.format}</span>}
            {def.access === "Shortcut" && <span className={`${s.chip} ${s.chipShortcut}`} title="Read in place from an external location">Shortcut</span>}
            {def.access === "Optimized" && <span className={s.chip} title="Stored in the catalog's own location">Optimized</span>}
            {loc?.host && <span className={s.chip}><span>on</span>{loc.host.replace(/^[a-z0-9+.-]+:\/\//i, "")}</span>}
          </div>
        </div>
        <div className={s.actions}>
          <button type="button" className={s.ghost} onClick={copy}>Copy name</button>
          {askDataset != null && <Link href={`/?dataset=${askDataset}`} className={s.ghost}><i className="fas fa-comment" aria-hidden="true" /> Ask</Link>}
          <Link href={labHref(def.catalog, def.schema, def.name)} className={`${s.ghost} ${s.primary}`}><i className="fas fa-code" aria-hidden="true" /> Query in SQL Lab</Link>
          <span className={s.copied} role="status" aria-live="polite">{copied}</span>
        </div>
      </header>

      <section className={s.signals} aria-label="Table signals">
        <div className={s.signal}>
          <div className={s.signalLabel}>Rows</div>
          <div className={s.signalValue}>
            {rows ? <>{fmt(rows.n)}<small className={rows.cls}>{rows.note}</small></> : <span className={s.muted}>—<small>not measured</small></span>}
          </div>
        </div>
        <div className={s.signal}>
          <div className={s.signalLabel}>Columns</div>
          <div className={s.signalValue}>{def.columns.length}<small>{def.columns.filter(c => !c.isNullable).length} required</small></div>
        </div>
        <div className={s.signal}>
          <div className={s.signalLabel}>Used by</div>
          <div className={s.signalValue}>
            {usage ? (usedCount ? <>{usedCount}<small>{usage.datasets.length} dataset{usage.datasets.length === 1 ? "" : "s"} · {usage.charts.length} chart{usage.charts.length === 1 ? "" : "s"} · {usage.dashboards.length} dashboard{usage.dashboards.length === 1 ? "" : "s"}</small></> : <span className={s.muted}>0<small>nothing reads it yet</small></span>) : <span className={s.muted}>…</span>}
          </div>
        </div>
        <div className={s.signal}>
          <div className={s.signalLabel}>Ask in plain English</div>
          <div className={s.signalValue}>
            {usage ? (dlmReady ? <span className={s.ok}>Ready<small>DLM context compiled</small></span> : usage.datasets.length ? <span className={s.warn}>Not compiled<small>dataset exists, context missing</small></span> : <span className={s.muted}>No<small>needs a dataset</small></span>) : <span className={s.muted}>…</span>}
          </div>
        </div>
      </section>

      <div className={s.body}>
        <div className={s.col}>
          <section className={s.panel} style={{ marginBottom: 0 }}>
            <div className={s.panelHead}><h2 className={s.panelTitle}>Columns</h2><span className={s.panelMeta}>from the KaveonDB definition</span></div>
            <div className={s.grid}>
              <table>
                <thead><tr><th className={s.ord}>#</th><th>Name</th><th>Type</th><th>Nullable</th></tr></thead>
                <tbody>
                  {def.columns.map((c, i) => (
                    <tr key={c.name}><td className={s.ord}>{i + 1}</td><td>{c.name}</td><td className={s.type}>{c.dataType}</td><td className={s.nullable}>{c.isNullable ? "yes" : "no"}</td></tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>

          <section className={s.panel} style={{ marginBottom: 0 }}>
            <div className={s.panelHead}>
              <h2 className={s.panelTitle}>Sample rows</h2>
              {sample && <span className={s.panelMeta}>{sample.rows.length} rows · {sample.executionTime.toFixed(2)} s</span>}
            </div>
            {!isAnalyst ? (
              <div className={s.note}>Sampling runs a query against KaveonDB and needs the Analyst role. Your role can browse definitions.</div>
            ) : sample ? (
              sample.rows.length ? (
                <div className={s.grid}>
                  <table>
                    <thead><tr>{sample.columns.map(c => <th key={c}>{c}</th>)}</tr></thead>
                    <tbody>{sample.rows.map((row, i) => (
                      <tr key={i}>{row.map((cell, j) => cell === null ? <td key={j} className={s.null}>NULL</td> : <td key={j}>{String(cell)}</td>)}</tr>
                    ))}</tbody>
                  </table>
                </div>
              ) : <div className={s.note}>The table is empty.</div>
            ) : sampleError ? (
              <div className={`${s.note} ${s.noteErr}`}><b>Sample failed.</b> {sampleError.message}</div>
            ) : (
              <div className={s.note} style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12, flexWrap: "wrap" }}>
                <span>Runs <code>SELECT * … LIMIT 20</code> against KaveonDB and records it in query history.</span>
                <button type="button" className={s.ghost} onClick={loadSample} disabled={sampleLoading}>{sampleLoading ? "Running…" : "Load sample"}</button>
              </div>
            )}
          </section>
        </div>

        <div className={s.col}>
          <section className={s.panel} style={{ marginBottom: 0 }}>
            <div className={s.panelHead}><h2 className={s.panelTitle}>Language context</h2><span className={s.panelMeta}>Kaveon DLM</span></div>
            <div className={s.dlm}>
              <div className={`${s.dlmMark} ${dlmReady ? s.dlmMarkOn : ""}`}><i className={`fas ${dlmReady ? "fa-check" : "fa-comment"}`} aria-hidden="true" /></div>
              <div style={{ minWidth: 0 }}>
                {dlmReady ? (
                  <>
                    <div className={s.dlmTitle}>The DLM has this table in context</div>
                    <div className={s.dlmBody}>
                      Compiled {when(dlmReady.builtAt) ?? "recently"}{dlmRows != null ? <> over <b>{fmt(dlmRows)}</b> rows</> : null}. Questions resolve deterministically; answers from context need no scan.
                    </div>
                  </>
                ) : usage?.datasets.length ? (
                  <>
                    <div className={s.dlmTitle}>Dataset exists, context not compiled</div>
                    <div className={s.dlmBody}>Open the dataset and build its DLM context to ask questions without writing SQL.</div>
                  </>
                ) : (
                  <>
                    <div className={s.dlmTitle}>No dataset reads this table</div>
                    <div className={s.dlmBody}>Create a dataset on it to chart it, put it on a dashboard, and ask it questions.</div>
                  </>
                )}
                {!usage?.datasets.length && isAnalyst && (
                  <div style={{ marginTop: 10 }}><Link href="/datasets/new" className={s.ghost}><i className="fas fa-plus" aria-hidden="true" /> Create dataset</Link></div>
                )}
              </div>
            </div>
          </section>

          <section className={s.panel} style={{ marginBottom: 0 }}>
            <div className={s.panelHead}><h2 className={s.panelTitle}>Used by</h2>{usage && usedCount ? <span className={s.panelMeta}>{usedCount}</span> : null}</div>
            {!usage ? <div className={s.note}>Looking up datasets, charts and dashboards…</div>
              : !usedCount ? <div className={s.note}>Nothing in the Library reads this table yet.</div>
              : (
                <>
                  {usage.datasets.length > 0 && <><div className={s.group}>Datasets</div><ul className={s.list}>{usage.datasets.map(d => <li key={d.id}><i className="fas fa-layer-group" /><Link href={`/datasets/${d.id}`}>{d.name}</Link>{d.visibility !== "published" && <span className={s.tag}>{d.visibility}</span>}</li>)}</ul></>}
                  {usage.charts.length > 0 && <><div className={s.group}>Charts</div><ul className={s.list}>{usage.charts.map(c => <li key={c.id}><i className="fas fa-chart-bar" /><Link href={`/charts/${c.id}`}>{c.name}</Link></li>)}</ul></>}
                  {usage.dashboards.length > 0 && <><div className={s.group}>Dashboards</div><ul className={s.list}>{usage.dashboards.map(d => <li key={String(d.id)}><i className="fas fa-table-columns" /><Link href={`/dashboards/${d.id}`}>{d.name}</Link></li>)}</ul></>}
                </>
              )}
          </section>

          <section className={s.panel} style={{ marginBottom: 0 }}>
            <div className={s.panelHead}><h2 className={s.panelTitle}>Location</h2>{def.format && <span className={s.panelMeta}>{def.format}{def.access ? ` · ${def.access.toLowerCase()}` : ""}</span>}</div>
            {loc ? (
              <dl className={s.loc}>
                {loc.host ? <><dt>Account</dt><dd>{loc.host}</dd><dt>Path</dt><dd>{loc.path}</dd></>
                  : <><dt>Path</dt><dd>{loc.path}</dd><dt /><dd className={s.nullable}>relative to the catalog&rsquo;s storage container</dd></>}
              </dl>
            ) : <div className={s.note}>The definition does not carry a storage location.</div>}
          </section>
        </div>
      </div>
    </>
  );
}
