"use client";

/**
 * Catalog — the inventory of everything Kaveon can answer over.
 *
 * The page is the table list. Registration lives beside it, because a schema
 * is the only thing this list can be missing: the action appears once, in the
 * header of the list it adds to.
 */

import { useState } from "react";
import { useRole } from "../../hooks/useRole";
import { RegisterSheet } from "./CatalogEditor";
import { Inventory } from "./Inventory";
import s from "./catalog.module.css";

export default function CatalogOverviewPage() {
  const { isEditor } = useRole();
  const [adding, setAdding] = useState(false);

  return (
    <>
      <Inventory
        action={isEditor && (
          <button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => setAdding(true)}>
            <i className="fas fa-plus" aria-hidden="true" /> Add schema
          </button>
        )}
      />
      {adding && <RegisterSheet kind="schema" onClose={() => setAdding(false)} />}
    </>
  );
}
