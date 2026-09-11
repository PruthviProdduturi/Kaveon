"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useEffect, useState } from "react";
import { useRole } from "../../../../../hooks/useRole";
import { useCatalogTree } from "../../../CatalogShell";
import s from "../../../catalog.module.css";
import { CatalogError, Sample, TableDef, fetchSample, fetchTable, labHref, shortLocation } from "../../../lib";

export default function CatalogTablePage() {
  const p = useParams<{ catalog: string; schema: string; table: string }>();
  const catalog = decodeURIComponent(p.catalog), schema = decodeURIComponent(p.schema), table = decodeURIComponent(p.table);
  const { isAnalyst } = useRole();
  const { catalogs, sourceFor } = useCatalogTree();
  const source = sourceFor(catalog)?.id ?? null;

  const [def, setDef] = useState<TableDef | null>(null);
  const [defError, setDefError] = useState<CatalogError | null>(null);
  const [sample, setSample] = useState<Sample | null>(null);
  const [sampleError, setSampleError] = useState<CatalogError | null>(null);
  const [sampleLoading, setSampleLoading] = useState(false);
  const [copied, setCopied] = useState("");

  useEffect(() => {
    let cancelled = false;
    setDef(null); setDefError(null); setSample(null); setSampleError(null);
    if (!source) {
      if (catalogs) setDefError(new CatalogError(404, `${catalog} is not a registered catalog.`));
      return;
    }
    fetchTable(source, schema, table)
      .then(d => { if (!cancelled) setDef(d); })
      .catch(e => { if (!cancelled) setDefError(e instanceof CatalogError ? e : new CatalogError(0, "The definition could not be read.")); });
    return () => { cancelled = true; };
  }, [source, catalogs, catalog, schema, table]);

  const loadSample = async () => {
    if (!source) return;
    setSampleLoading(true); setSampleError(null);
    try { setSample(await fetchSample(source, schema, table)); }
    catch (e) { setSampleError(e instanceof CatalogError ? e : new CatalogError(0, "The sample could not be read.")); }
    finally { setSampleLoading(false); }
  };

  const fullName = def ? `${def.catalog}.${def.schema}.${def.name}` : `${schema}.${table}`;
  const copy = async () => {
    try { await navigator.clipboard.writeText(fullName); setCopied("Copied"); } catch { setCopied("Copy unavailable"); }
    setTimeout(() => setCopied(""), 1600);
  };

  if (defError) {
    return (
      <>
        <div className={s.crumbs}><Link href="/catalog">Catalog</Link><i>›</i><b>{catalog}</b><i>›</i><Link href={`/catalog/${encodeURIComponent(catalog)}/${encodeURIComponent(schema)}`}>{schema}</Link><i>›</i><b>{table}</b></div>
        <div className={`${s.note} ${s.noteErr}`} role="alert"><b>{defError.status === 404 ? "Not in the catalog." : "Definition unavailable."}</b> {defError.message}</div>
      </>
    );
  }
  if (!def) return <div className={s.note}>Reading definition…</div>;

  const loc = def.location ? shortLocation(def.location) : null;

  return (
    <>
      <div className={s.crumbs}><Link href="/catalog">Catalog</Link><i>›</i><b>{def.catalog}</b><i>›</i><Link href={`/catalog/${encodeURIComponent(def.catalog)}/${encodeURIComponent(def.schema)}`}>{def.schema}</Link><i>›</i><b>{def.name}</b></div>

      <header className={s.head}>
        <div style={{ minWidth: 0 }}>
          <h1 className={s.title}>{def.name}</h1>
          <p className={s.subtitle}>{def.columns.length} column{def.columns.length === 1 ? "" : "s"}{def.revision != null ? ` · definition revision ${def.revision}` : ""}{def.lifecycle && def.lifecycle !== "active" ? ` · ${def.lifecycle}` : ""}</p>
          <div className={s.chips}>
            {def.format && <span className={s.chip}>{def.format}</span>}
            {def.access === "Shortcut" && <span className={`${s.chip} ${s.chipShortcut}`} title="Read in place from an external location">Shortcut</span>}
            {def.access === "Optimized" && <span className={s.chip} title="Stored in the catalog's own location">Optimized</span>}
            {loc && <span className={s.chip}><span>on</span>{loc.host.replace(/^[a-z0-9+.-]+:\/\//i, "")}</span>}
          </div>
        </div>
        <div className={s.actions}>
          <button type="button" className={s.ghost} onClick={copy}>Copy name</button>
          <Link href={labHref(def.catalog, def.schema, def.name)} className={`${s.ghost} ${s.primary}`}><i className="fas fa-code" aria-hidden="true" /> Query in SQL Lab</Link>
          <span className={s.copied} role="status" aria-live="polite">{copied}</span>
        </div>
      </header>

      <section className={s.panel}>
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

      <section className={s.panel}>
        <div className={s.panelHead}><h2 className={s.panelTitle}>Location</h2>{def.format && <span className={s.panelMeta}>{def.format}{def.access ? ` · ${def.access.toLowerCase()}` : ""}</span>}</div>
        {loc ? (
          <dl className={s.loc}>
            {loc.host && <><dt>Account</dt><dd>{loc.host}</dd></>}
            <dt>Path</dt><dd>{loc.path}</dd>
          </dl>
        ) : <div className={s.note}>The definition does not carry a storage location.</div>}
      </section>

      <section className={s.panel}>
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
    </>
  );
}
