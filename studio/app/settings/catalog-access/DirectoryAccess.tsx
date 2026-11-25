"use client";

import { useEffect, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import s from "../settings.module.css";

type Catalog = { catalog: string; access: string; via_app_role?: string | null };

export function DirectoryAccess() {
  const { role } = useRole();
  const [catalogs, setCatalogs] = useState<Catalog[] | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    let active = true;
    msalFetch(API_BASE + "/api/v1/engine/catalog-access/me").then(async response => {
      if (!response.ok) throw new Error();
      const result = await response.json();
      if (active) setCatalogs(result.catalogs);
    }).catch(() => { if (active) setError(true); });
    return () => { active = false; };
  }, []);
  return <section className={s.card}>
    <h2 className={s.cardTitle}>Microsoft Entra access</h2>
    <p>Assign people or security groups to Viewer, Analyst, Editor or Admin under Enterprise applications in Microsoft Entra.</p>
    <h3>Your catalog access</h3>
    {error ? <p role="alert">Could not read your effective access.</p> : catalogs === null ? <p>Checking access…</p> :
      catalogs.length ? <ul>{catalogs.map(item => <li key={item.catalog}>{item.catalog} · {item.access} through {item.via_app_role ?? role}</li>)}</ul> :
      <p>Your role has no catalog access.</p>}
    <p>AWS IAM integration · Coming soon</p>
  </section>;
}
