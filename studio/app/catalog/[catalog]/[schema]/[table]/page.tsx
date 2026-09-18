"use client";

import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useEffect, useMemo, useState } from "react";
import { useRole } from "../../../../../hooks/useRole";
import { RemoveTableDialog } from "../../../CatalogEditor";
import { useCatalogTree } from "../../../CatalogShell";
import s from "../../../catalog.module.css";
import {
  CatalogError, Sample, TableDef, Usage,
  deleteTable, enc, fetchSample, fetchTable, fetchUsage, labHref, shortLocation,
} from "../../../lib";

const fmt = (n: number) => n.toLocaleString();
const when = (iso: string | null) => iso ? new Date(iso).toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" }) : null;

/** Up to three distinct sample values for a column, in the order they were seen. */
function distinctValues(sample: Sample | null, column: string, limit = 3): string[] {
  if (!sample) return [];
  const i = sample.columns.indexOf(column);
  if (i < 0) return [];
  const seen: string[] = [];
  for (const row of sample.rows) {
    const v = row[i];
    const text = v === null || v === undefined ? "NULL" : String(v);
    if (!seen.includes(text)) seen.push(text);
    if (seen.length >= limit) break;
  }
  return seen;
}

export default function CatalogTablePage() {
  const p = useParams<{ catalog: string; schema: string; table: string }>();
  const catalog = decodeURIComponent(p.catalog), schema = decodeURIComponent(p.schema), table = decodeURIComponent(p.table);
  const { isAnalyst, isEditor } = useRole();
  const { catalogs, sourceFor, refreshTables } = useCatalogTree();
  const router = useRouter();
  const source = sourceFor(catalog)?.id ?? null;
  const fullName = `${catalog}.${schema}.${table}`;

  const [def, setDef] = useState<TableDef | null>(null);
  const [defError, setDefError] = useState<CatalogError | null>(null);
  const [usage, setUsage] = useState<Usage | null>(null);
  const [sample, setSample] = useState<Sample | null>(null);
  const [sampleState, setSampleState] = useState<"idle" | "loading" | "done" | "failed">("idle");
  const [copied, setCopied] = useState(false);
  const [removing, setRemoving] = useState<{ busy: boolean; error: string | null } | null>(null);

  const remove = async () => {
    if (!def?.id || def.revision == null) return;
    setRemoving({ busy: true, error: null });
    try {
      await deleteTable(def.id, def.revision);
      await refreshTables(catalog, schema);
      router.push(`/catalog/${enc(catalog)}/${enc(schema)}`);
    } catch (e) {
      setRemoving({ busy: false, error: e instanceof CatalogError ? e.message : "The table could not be removed." });
    }
  };

  useEffect(() => {
    let cancelled = false;
    setDef(null); setDefError(null); setUsage(null); setSample(null); setSampleState("idle");
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
    // The sample is part of reading a table, not a separate act: one bounded
    // SELECT … LIMIT 20 on the Engine, shown beside the columns it describes.
    if (isAnalyst) {
      setSampleState("loading");
      fetchSample(source, schema, table)
        .then(sm => { if (!cancelled) { setSample(sm); setSampleState("done"); } })
        .catch(() => { if (!cancelled) setSampleState("failed"); });
    }
    return () => { cancelled = true; };
  }, [source, catalogs, catalog, schema, table, isAnalyst]);

  const copy = async () => {
    try { await navigator.clipboard.writeText(fullName); setCopied(true); setTimeout(() => setCopied(false), 1400); } catch { /* clipboard unavailable */ }
  };

  const crumbs = (
    <div className={s.crumbs}>
      <Link href="/catalog">Catalog</Link><i>›</i><b>{catalog}</b><i>›</i>
      <Link href={`/catalog/${encodeURIComponent(catalog)}/${encodeURIComponent(schema)}`}>{schema}</Link><i>›</i><b>{table}</b>
    </div>
  );

  const dlmReady = usage?.dlm.find(d => d.status === "ready") ?? null;
  const askDataset = dlmReady ? Number(dlmReady.datasetId) : usage?.datasets[0]?.id ?? null;
  const usedCount = usage ? usage.datasets.length + usage.charts.length + usage.dashboards.length : 0;
  const loc = useMemo(() => def?.location ? shortLocation(def.location) : null, [def]);

  if (defError) {
    return <>{crumbs}<div className={`${s.note} ${s.noteErr}`} role="alert"><b>{defError.status === 404 ? "Not in the catalog." : "Definition unavailable."}</b> {defError.message}</div></>;
  }
  if (!def) return <>{crumbs}<div className={s.note}>Reading definition…</div></>;

  return (
    <div className={s.table}>
      {crumbs}

      <header className={s.hero}>
        <div className={s.heroMain}>
          <h1 className={s.heroName}>
            {def.name}
            <button type="button" className={s.copyBtn} onClick={copy} aria-label="Copy qualified name" title="Copy qualified name">
              <i className={`fas ${copied ? "fa-check" : "fa-copy"}`} aria-hidden="true" />
            </button>
          </h1>
          <p className={s.heroPath}>{fullName}</p>
          <div className={s.heroFacts}>
            <span className={s.heroStat}>{def.rowCount != null ? fmt(def.rowCount) : "—"}<small>rows{def.rowCount != null ? "" : ", not yet counted"}</small></span>
            <span className={s.heroStat}>{def.columns.length}<small>columns</small></span>
            {def.format && <span className={s.heroFact}>{def.format}{def.access ? ` · ${def.access.toLowerCase()}` : ""}</span>}
            {def.revision != null && <span className={s.heroFact}>revision {def.revision}</span>}
            {dlmReady && <span className={`${s.heroFact} ${s.heroFactOk}`}><i className="fas fa-check" aria-hidden="true" /> ask-ready{when(dlmReady.builtAt) ? ` · compiled ${when(dlmReady.builtAt)}` : ""}</span>}
          </div>
        </div>
        <div className={s.heroActions}>
          {askDataset != null && <Link href={`/home?dataset=${askDataset}`} className={s.ghost}><i className="fas fa-comment" aria-hidden="true" /> Ask</Link>}
          {isAnalyst && !usage?.datasets.length && <Link href="/datasets/new" className={s.ghost}><i className="fas fa-plus" aria-hidden="true" /> Create dataset</Link>}
          <Link href={labHref(def.catalog, def.schema, def.name)} className={`${s.ghost} ${s.primary}`}><i className="fas fa-code" aria-hidden="true" /> Query in SQL Lab</Link>
          {isEditor && def.id && def.revision != null && (
            <button type="button" className={`${s.ghost} ${s.danger}`} onClick={() => setRemoving({ busy: false, error: null })} title="Remove the table from the catalog"><i className="fas fa-trash-can" aria-hidden="true" /> Remove</button>
          )}
        </div>
        {loc && (
          <p className={s.heroLocation}>
            <span>{loc.host ? "Stored at" : "Stored in the catalog at"}</span> {loc.host ? `${loc.host}/` : ""}{loc.path}
          </p>
        )}
      </header>

      <section className={s.columnsSection} aria-label="Columns">
        <div className={s.sectionHead}>
          <h2 className={s.sectionTitle}>Columns</h2>
          <span className={s.sectionMeta}>
            {sampleState === "loading" ? "reading a sample…"
              : sampleState === "done" && sample ? `sample of ${sample.rows.length} rows · ${sample.executionTime.toFixed(2)} s on KaveonDB`
              : sampleState === "failed" ? "sample unavailable"
              : !isAnalyst ? "samples need the Analyst role" : ""}
          </span>
        </div>
        <table className={s.columns}>
          <thead>
            <tr><th className={s.ord}>#</th><th>Name</th><th>Type</th><th>Sample values</th></tr>
          </thead>
          <tbody>
            {def.columns.map((c, i) => {
              const values = distinctValues(sample, c.name);
              return (
                <tr key={c.name}>
                  <td className={s.ord}>{i + 1}</td>
                  <td className={s.colName}>{c.name}{!c.isNullable && <span className={s.required} title="Not nullable">required</span>}</td>
                  <td className={s.colType}>{c.dataType}</td>
                  <td className={s.colSample}>
                    {values.length ? values.map((v, k) => <span key={k} className={v === "NULL" ? s.null : ""}>{v}</span>) : <span className={s.faint}>{sampleState === "loading" ? "…" : ""}</span>}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </section>

      <section className={s.usedSection} aria-label="Used by">
        <div className={s.sectionHead}>
          <h2 className={s.sectionTitle}>Used by</h2>
          {usage && usedCount > 0 && <span className={s.sectionMeta}>{usage.datasets.length} dataset{usage.datasets.length === 1 ? "" : "s"} · {usage.charts.length} chart{usage.charts.length === 1 ? "" : "s"} · {usage.dashboards.length} dashboard{usage.dashboards.length === 1 ? "" : "s"}</span>}
        </div>
        {!usage ? (
          <p className={s.quiet}>Looking up datasets, charts and dashboards…</p>
        ) : usedCount === 0 ? (
          <p className={s.quiet}>
            Nothing in the Library reads this table yet.
            {isAnalyst ? " Create a dataset on it to chart it, put it on a dashboard, and ask it questions in plain language." : ""}
          </p>
        ) : (
          <div className={s.usedGroups}>
            {usage.datasets.length > 0 && (
              <div className={s.usedGroup}><div className={s.group}>Datasets</div><ul className={s.list}>{usage.datasets.map(d => <li key={d.id}><i className="fas fa-layer-group" /><Link href={`/datasets/${d.id}`}>{d.name}</Link>{d.visibility !== "published" && <span className={s.tag}>{d.visibility}</span>}</li>)}</ul></div>
            )}
            {usage.charts.length > 0 && (
              <div className={s.usedGroup}><div className={s.group}>Charts</div><ul className={s.list}>{usage.charts.map(c => <li key={c.id}><i className="fas fa-chart-bar" /><Link href={`/charts/${c.id}`}>{c.name}</Link></li>)}</ul></div>
            )}
            {usage.dashboards.length > 0 && (
              <div className={s.usedGroup}><div className={s.group}>Dashboards</div><ul className={s.list}>{usage.dashboards.map(d => <li key={String(d.id)}><i className="fas fa-table-columns" /><Link href={`/dashboards/${d.id}`}>{d.name}</Link></li>)}</ul></div>
            )}
          </div>
        )}
      </section>

      {removing && (
        <RemoveTableDialog fullName={fullName} busy={removing.busy} error={removing.error}
          onCancel={() => setRemoving(null)} onConfirm={remove} />
      )}
    </div>
  );
}
