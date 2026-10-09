"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import { SETUP_DB_ICONS } from "../../../components/DataSourceIcons";
import s from "../settings.module.css";

interface SystemStore {
  configured: boolean;
  mode: "local" | "adls" | null;
  durable: boolean | null;
  account: string | null;
  container: string | null;
  prefix: string | null;
  location: string | null;
}
interface MetadataConfig {
  db_type: "fabric_sql" | "azure_sql" | "postgresql" | "mysql" | "kaveondb";
  label: string; endpoint: string; host: string; port: string; database: string;
  ui_configured: boolean; schema?: string; storage?: SystemStore;
}
interface EngineStatus { configured: boolean; connected: boolean; catalog_count: number | null }
interface EngineCluster { environment: string; coordinator: { version: string; uptime_secs: number }; active_workers: number; total_nodes: number }

function uptime(secs: number): string {
  if (secs < 3600) return `${Math.max(1, Math.floor(secs / 60))}m`;
  const h = Math.floor(secs / 3600);
  return h < 24 ? `${h}h ${Math.floor((secs % 3600) / 60)}m` : `${Math.floor(h / 24)}d ${h % 24}h`;
}

function Status({ on, children }: { on: boolean | null; children: React.ReactNode }) {
  return <span className={`${s.status} ${on === true ? s.statusOn : s.statusOff}`}><span className={s.dot} />{children}</span>;
}

function Facts({ rows }: { rows: [string, string][] }) {
  return (
    <div className={s.facts}>
      {rows.filter(([, v]) => v).map(([k, v]) => (
        <div key={k} className={s.fact}>
          <div className={s.factLabel}>{k}</div>
          <div className={s.factValue} title={v}>{v}</div>
        </div>
      ))}
    </div>
  );
}

/** What the system store is, in one phrase a reader can act on. Reports the
 *  absence of a setting as an absence — the server does not guess a location,
 *  and neither does this. */
function storeSummary(store: SystemStore | undefined): string {
  if (!store) return "";
  if (!store.configured) return "Not configured";
  if (store.mode === "adls") return `${store.account} · ${store.location}`;
  return store.location ?? "";
}

export default function ConnectionsPage() {
  const { isAdmin, loading: roleLoading } = useRole();
  const [meta, setMeta] = useState<MetadataConfig | null | undefined>(undefined);
  const [engine, setEngine] = useState<EngineStatus | null | undefined>(undefined);
  const [cluster, setCluster] = useState<EngineCluster | null>(null);

  useEffect(() => {
    if (roleLoading || !isAdmin) return;
    let cancelled = false;
    msalFetch(`${API_BASE}/api/v1/admin/metadata`).then(r => (r.ok ? r.json() : null))
      .then((d: MetadataConfig | null) => { if (!cancelled) setMeta(d); }).catch(() => { if (!cancelled) setMeta(null); });
    msalFetch(`${API_BASE}/api/v1/catalog-sources/engine-status`).then(r => (r.ok ? r.json() : null))
      .then((d: EngineStatus | null) => { if (!cancelled) setEngine(d); }).catch(() => { if (!cancelled) setEngine(null); });
    msalFetch(`${API_BASE}/api/v1/engine/console/cluster`).then(r => (r.ok ? r.json() : null))
      .then((d: EngineCluster | null) => { if (!cancelled) setCluster(d); }).catch(() => { if (!cancelled) setCluster(null); });
    return () => { cancelled = true; };
  }, [roleLoading, isAdmin]);

  // KaveonDB is both the query runtime and the system of record, so there is
  // one system to show, not two. Only a deployment still served by an external
  // metadata database has a second connection worth a card of its own.
  const unified = meta?.db_type === "kaveondb";
  const store = meta?.storage;

  const engineCard = (
    <section className={s.card} aria-labelledby="db-title">
      <div className={s.cardHead}>
        <div className={s.cardId}>
          <div className={s.mark} style={{ color: "var(--accent)" }}><i className="fas fa-bolt" /></div>
          <div style={{ minWidth: 0 }}>
            <h2 id="db-title" className={s.cardTitle}>KaveonDB</h2>
            <p className={s.cardSub}>
              {unified
                ? <>Distributed query runtime and system of record. Datasets, charts, dashboards and history are transactional rows in <code>{meta?.database}.{meta?.schema ?? "product"}</code>.</>
                : <>Distributed query and transaction runtime. Reads your lake in place.</>}
            </p>
          </div>
        </div>
        <Status on={engine === undefined ? null : !!engine?.connected}>{engine === undefined ? "Checking" : engine?.connected ? "Connected" : "Unavailable"}</Status>
      </div>
      {engine?.connected && cluster && (
        <Facts rows={[
          ["Environment", cluster.environment],
          ["Coordinator", `v${cluster.coordinator.version}`],
          ["Uptime", uptime(cluster.coordinator.uptime_secs)],
          ["Workers", `${cluster.active_workers} of ${cluster.total_nodes} nodes`],
          ["Catalogs", String(engine.catalog_count ?? 0)],
          ...(unified ? ([["System store", storeSummary(store)]] as [string, string][]) : []),
        ]} />
      )}
      {unified && store?.configured && store.durable === false && (
        <p className={s.note} style={{ marginTop: 12, color: "var(--warning, #f59e0b)" }}>
          The system store is a directory on the host, which does not survive the container that writes it.
          Point <code>KAVEON_PRODUCT_STORAGE_MODE</code> at object storage before this deployment holds
          anything you cannot rebuild.
        </p>
      )}
      {unified && store && !store.configured && (
        <p className={s.note} style={{ marginTop: 12 }}>
          The server has not been told where KaveonDB keeps its records, so this page will not guess.
          Set <code>KAVEON_PRODUCT_STORAGE_MODE</code> and the account, container and prefix for it.
        </p>
      )}
      <div className={s.cardFoot}>
        <p className={s.note}>
          {engine?.connected
            ? "Connection and credentials are set at deployment. The console shows live workers, memory and every query KaveonDB has recorded."
            : "The server could not verify its KaveonDB connection. Set KAVEON_ENGINE_URL and the bridge credential at deployment."}
        </p>
        {engine?.connected && (
          <div style={{ display: "flex", gap: 8 }}>
            <Link href="/catalog" className={s.ghost}>Catalog</Link>
            <Link href="/engine" className={`${s.ghost} ${s.primary}`}>Open KaveonDB console</Link>
          </div>
        )}
      </div>
    </section>
  );

  if (unified || meta === undefined) return <div className={s.stack}>{engineCard}</div>;

  // A deployment still served by an external metadata database: two systems,
  // two cards, and the PostgreSQL case says plainly that it is on its way out.
  const metaIcon = meta ? SETUP_DB_ICONS[meta.db_type] : null;
  const legacy = meta?.db_type === "postgresql";

  return (
    <div className={s.stack}>
      <section className={s.card} aria-labelledby="meta-title">
        <div className={s.cardHead}>
          <div className={s.cardId}>
            <div className={s.mark} style={metaIcon ? { background: metaIcon.bg, borderColor: metaIcon.border } : undefined}>{metaIcon?.icon ?? <i className="fas fa-database" />}</div>
            <div style={{ minWidth: 0 }}>
              <h2 id="meta-title" className={s.cardTitle}>{legacy ? "Legacy metadata database" : "Metadata database"}</h2>
              <p className={s.cardSub}>
                {meta?.ui_configured
                  ? legacy
                    ? <>Control-plane metadata is still served by <code>PostgreSQL</code>; KaveonDB is the query runtime. Retirement is pending its final migration gates.</>
                    : <>Datasets, charts, dashboards and history live here · <code>{meta.label}</code></>
                  : "Where Kaveon keeps datasets, charts, dashboards and history."}
              </p>
            </div>
          </div>
          <Status on={legacy ? false : meta === null ? null : !!meta?.ui_configured}>
            {legacy ? "Retirement pending" : meta?.ui_configured ? "Connected" : "Not configured"}
          </Status>
        </div>
        {meta?.ui_configured ? (
          <Facts rows={[["Database", meta.database], ["Host", meta.host], ["Endpoint", meta.endpoint], ["Port", meta.port]]} />
        ) : meta === null ? (
          <p className={s.note} style={{ marginTop: 12 }}>Run the setup wizard to connect a metadata database. Nothing can be saved until it exists.</p>
        ) : null}
        {legacy && (
          <p className={s.note} style={{ marginTop: 12, color: "var(--warning, #f59e0b)" }}>
            PostgreSQL remains the active control-plane authority in this deployment. Do not delete it until the 16-family replay, shadow parity, write fence, restart, and backup/restore gates pass.
          </p>
        )}
      </section>
      {engineCard}
    </div>
  );
}
