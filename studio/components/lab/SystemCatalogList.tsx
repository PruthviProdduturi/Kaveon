"use client";

/**
 * KaveonDB in the SQL Lab sidebar.
 *
 * Every other row in this panel is a way to start a statement: click it and
 * its columns open, press the plus and a SELECT lands in the editor. These
 * rows are not. The Engine publishes no `product` schema into its query
 * snapshot — `kaveon.product.<table>` is the facade its parser resolves to
 * the record transactions — so a SELECT against these names is refused by the
 * analyzer, and the platform's API is the only way to read them. The panel
 * therefore shows what is there, with the real qualified name on every row,
 * and offers nothing: no click target, no insert, no drag.
 *
 * It replaces the table tree only while this catalog is the one selected, so
 * the Lab never has two kinds of row in one list.
 */

import { useEffect, useState } from "react";
import {
  SYSTEM_CATALOG_LABEL, SystemCatalogError, SystemCatalogReading, fetchSystemCatalog,
} from "../../utils/systemCatalog";

export function SystemCatalogList({ isAdmin }: { isAdmin: boolean }) {
  const [reading, setReading] = useState<SystemCatalogReading | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isAdmin) return;
    let cancelled = false;
    fetchSystemCatalog()
      .then(result => { if (!cancelled) { setReading(result); setError(null); } })
      .catch(failure => {
        if (cancelled) return;
        setError(failure instanceof SystemCatalogError
          ? failure.message
          : `${SYSTEM_CATALOG_LABEL} could not be read.`);
      });
    return () => { cancelled = true; };
  }, [isAdmin]);

  if (!isAdmin) {
    return (
      <div className="system-catalog">
        <p className="system-catalog__note">
          {SYSTEM_CATALOG_LABEL} holds the platform&rsquo;s own records. Browsing its tables
          requires the administrator role.
        </p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="system-catalog">
        <p className="system-catalog__note" role="alert">{error}</p>
      </div>
    );
  }

  const prefix = reading
    ? `${reading.catalog.identifier}.${reading.catalog.schema}.`
    : "";

  return (
    <div className="system-catalog">
      <p className="system-catalog__note">
        The platform&rsquo;s own records, written only through {SYSTEM_CATALOG_LABEL}&rsquo;s
        transaction boundary. These tables are served by its record API, not by the
        query planner, so they cannot be selected from here.
      </p>
      {!reading && <p className="system-catalog__note">Reading…</p>}
      {reading && (
        <>
          <div className="system-catalog__schema">
            <i className="fas fa-layer-group" aria-hidden="true" />
            <span>{reading.catalog.schema}</span>
            <span className="system-catalog__count">{reading.tables.length} tables</span>
          </div>
          <ul className="system-catalog__tables">
            {reading.tables.map(table => (
              <li key={table.table}>
                <i className="fas fa-table" aria-hidden="true" />
                <span className="system-catalog__name">
                  <span className="system-catalog__prefix">{prefix}</span>{table.table}
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

export default SystemCatalogList;
