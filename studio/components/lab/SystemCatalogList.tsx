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
  SYSTEM_CATALOG_IDENTIFIER, SYSTEM_CATALOG_LABEL, SystemCatalogError,
  SystemCatalogReading, fetchSystemCatalog,
} from "../../utils/systemCatalog";
import { ReadFailure, UnknownLine, reasonFromStatus } from "../Reading";

export function SystemCatalogList({ isAdmin, identifier }: {
  isAdmin: boolean;
  /** What a statement calls this catalog, from the Engine's own source record. */
  identifier: string;
}) {
  const [reading, setReading] = useState<SystemCatalogReading | null>(null);
  const [unknown, setUnknown] = useState<ReadFailure | null>(null);

  useEffect(() => {
    if (!isAdmin) return;
    let cancelled = false;
    fetchSystemCatalog()
      .then(result => { if (!cancelled) { setReading(result); setUnknown(null); } })
      .catch(failure => {
        if (cancelled) return;
        // A read that did not answer is an unknown, not a finding: the status
        // says whether this server was never told about the catalog or simply
        // did not answer, and the server's own message is kept verbatim rather
        // than replaced by a fixed line per status.
        setUnknown({
          what: SYSTEM_CATALOG_LABEL,
          says: "which record families it holds",
          reason: failure instanceof SystemCatalogError ? reasonFromStatus(failure.status) : null,
          detail: failure instanceof SystemCatalogError ? failure.message : null,
        });
      });
    return () => { cancelled = true; };
  }, [isAdmin]);

  // The Lab passes the catalog name from the Engine's source record, which is
  // an empty string before that record has loaded — the sentence below then
  // read "A statement names this catalog ." with an empty code box after it.
  // The reading's own answer is preferred; the module's constant is the floor.
  const catalogName = reading?.catalog.identifier || identifier || SYSTEM_CATALOG_IDENTIFIER;

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

  if (unknown) {
    return (
      <div className="system-catalog">
        <p className="system-catalog__note">
          The platform&rsquo;s own records, written only through {SYSTEM_CATALOG_LABEL}&rsquo;s
          transaction boundary. A statement names this catalog{" "}
          <code className="system-catalog__prefix">{catalogName}</code>; KaveonDB is what the
          product calls it.
        </p>
        <p className="system-catalog__note">
          <UnknownLine {...unknown} />
        </p>
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
        query planner, so they cannot be selected from here. A statement names the
        catalog <code className="system-catalog__prefix">{catalogName}</code>.
      </p>
      {!reading && <p className="system-catalog__note">Reading…</p>}
      {reading && (
        <>
          <div className="system-catalog__schema">
            <i className="fas fa-layer-group" aria-hidden="true" />
            <span>{reading.catalog.schema}</span>
            <span className="system-catalog__count">
              {reading.tables.length} {reading.tables.length === 1 ? "table" : "tables"}
            </span>
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
