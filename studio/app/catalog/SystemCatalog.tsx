"use client";

/**
 * KaveonDB, on the Catalog page.
 *
 * The data catalogs above this are places a reader puts tables. This one is
 * not: it is the catalog the platform keeps its own records in, and the only
 * useful thing to say about it is what it holds and that nobody edits it from
 * here. So it is a reading, never a list with actions — no register, no
 * remove, no measure, and no link into SQL Lab, because the Engine has no
 * `product` schema to scan: `kaveon.product.<table>` is the facade its parser
 * resolves to the record transactions, so a SELECT against these names is
 * refused by the analyzer. Offering the query would be offering a refusal.
 *
 * Every row carries the real qualified identifier, with the catalog and
 * schema receding and the table name leading, so the name a reader copies out
 * of this page is the one the Engine resolves. The product name KaveonDB
 * appears in the heading and nowhere a statement could pick it up.
 *
 * The structure costs nothing to read and draws at once. Counts do cost
 * something — the Engine answers a family by reading its records — so they
 * are a second read that fills the cells the rows already reserved.
 */

import { useEffect, useRef, useState } from "react";
import { useRole } from "../../hooks/useRole";
import s from "./catalog.module.css";
import {
  SYSTEM_CATALOG_LABEL, SYSTEM_TABLE_HOLDS, SystemCatalogError, SystemCatalogReading,
  fetchSystemCatalog, recordCount,
} from "../../utils/systemCatalog";

/** What the catalog is, which is true whether or not the reading arrives. */
const SYSTEM_CATALOG_BODY =
  "The platform's own records — every dataset, chart, dashboard, saved statement and audit entry — "
  + "kept in the Engine's built-in catalog, written only through its transaction boundary and read back "
  + "from a committed snapshot. Studio, the API and the CLI are the only writers; the tables themselves "
  + "are read-only, and the records inside them belong to the people who created them.";

export function SystemCatalog() {
  const { isAdmin } = useRole();
  const [reading, setReading] = useState<SystemCatalogReading | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The counts follow the structure, so the cells they will fill are drawn as
  // pending from the first paint rather than as em dashes that change their
  // mind a moment later.
  const [counting, setCounting] = useState(true);
  const live = useRef(true);

  useEffect(() => {
    live.current = true;
    return () => { live.current = false; };
  }, []);

  useEffect(() => {
    // Only an administrator may read these tables, and the rail already says
    // so to everyone else. Asking anyway would be one refused request per
    // page view for a panel that would not be shown.
    if (!isAdmin) { setCounting(false); return; }
    void (async () => {
      try {
        const structure = await fetchSystemCatalog();
        if (!live.current) return;
        setReading(structure);
        const counted = await fetchSystemCatalog({ counts: true });
        if (!live.current) return;
        setReading(counted);
      } catch (failure) {
        if (!live.current) return;
        setError(failure instanceof SystemCatalogError
          ? failure.message
          : `${SYSTEM_CATALOG_LABEL} could not be read.`);
      } finally {
        if (live.current) setCounting(false);
      }
    })();
  }, [isAdmin]);

  // A reader without the administrator role is told so by the rail, which
  // lists the catalog either way. Repeating the refusal as a panel would be
  // the second place on one page to say the same thing.
  if (!isAdmin) return null;
  // An administrator is shown the panel either way. What this surface can
  // always say is true without any reading at all — what the catalog holds,
  // who writes it, and that nobody edits it from here — and a server that
  // does not publish the reading is a fact about the server, not a reason to
  // leave an administrator looking at nothing.
  if (error || !reading) {
    return (
      <section className={s.system} aria-labelledby="system-catalog-title">
        <div className={s.systemHead}>
          <div className={s.systemIntro}>
            <p className={s.systemEyebrow}>System catalog</p>
            <h2 className={s.systemTitle} id="system-catalog-title">{SYSTEM_CATALOG_LABEL}</h2>
            <p className={s.systemBody}>{SYSTEM_CATALOG_BODY}</p>
          </div>
        </div>
        <p className={s.systemAbsent}>
          {error ?? `${SYSTEM_CATALOG_LABEL} could not be read.`}
          {" "}Its record families are not listed here until this server answers for them.
        </p>
      </section>
    );
  }

  const prefix = `${reading.catalog.identifier}.${reading.catalog.schema}.`;
  const total = reading.tables.reduce((sum, table) => sum + (table.records ?? 0), 0);
  const bounded = reading.tables.some(table => table.truncated);

  return (
    <section className={s.system} aria-labelledby="system-catalog-title">
      <div className={s.systemHead}>
        <div className={s.systemIntro}>
          <p className={s.systemEyebrow}>System catalog</p>
          <h2 className={s.systemTitle} id="system-catalog-title">{SYSTEM_CATALOG_LABEL}</h2>
          <p className={s.systemBody}>{SYSTEM_CATALOG_BODY}</p>
        </div>
        <dl className={s.systemFacts}>
          <div>
            <dt>Identifier</dt>
            <dd className={s.systemMono}>{reading.catalog.identifier}.{reading.catalog.schema}</dd>
          </div>
          <div>
            <dt>Writes</dt>
            <dd>Transactional, one commit per change</dd>
          </div>
          <div>
            <dt>This surface</dt>
            <dd>Read-only, administrators</dd>
          </div>
        </dl>
      </div>

      <div className={s.systemTableWrap}>
        <table className={s.systemTable}>
          <caption className={s.sr}>
            The record families in {prefix.replace(/\.$/, "")}, and how many records each holds.
          </caption>
          <thead>
            <tr>
              <th scope="col">Table</th>
              <th scope="col" className={s.systemCount}>Records</th>
            </tr>
          </thead>
          <tbody>
            {reading.tables.map(table => (
              <tr key={table.table}>
                <th scope="row">
                  <span className={s.systemName}>
                    <span className={s.systemPrefix}>{prefix}</span>{table.table}
                  </span>
                  <span className={s.systemHolds}>
                    {table.error ?? SYSTEM_TABLE_HOLDS[table.table] ?? ""}
                  </span>
                </th>
                <td className={s.systemCount}>
                  {table.error
                    ? <span className={s.dash}>—</span>
                    : typeof table.records === "number"
                      ? recordCount(table)
                      : counting
                        ? <span className={s.skel} style={{ width: 54 }} />
                        : <span className={s.dash}>—</span>}
                </td>
              </tr>
            ))}
          </tbody>
          <tfoot>
            <tr>
              <th scope="row">
                {reading.tables.length} tables
                {reading.snapshot.id && (
                  <span className={s.systemHolds}>
                    Read from one committed snapshot
                    {reading.snapshot.consistent ? "" : " per family — a commit landed while this was read"}.
                  </span>
                )}
              </th>
              <td className={s.systemCount}>
                {reading.counted
                  ? `${total.toLocaleString()}${bounded ? "+" : ""}`
                  : counting ? <span className={s.skel} style={{ width: 54 }} /> : <span className={s.dash}>—</span>}
              </td>
            </tr>
          </tfoot>
        </table>
      </div>
    </section>
  );
}
