"use client";

import Link from "next/link";
import { useCallback, useEffect, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import s from "../settings.module.css";
import g from "../governance/governance.module.css";
import { readError } from "../readError";

interface AuditRecord {
  seq: number; ts_ms: number; kind: string;
  principal?: string; display_identity?: string; role?: string; route?: string; query_id?: string;
  client?: string; source?: string; catalog?: string; schema?: string;
  statement?: string; resource_group?: string;
  admission_wait_ms?: number; elapsed_ms?: number; rows?: number; bytes_scanned?: number; mode?: string;
  error_code?: string; error?: string;
  object_type?: string; object_id?: string; revision_before?: number; revision_after?: number;
  details?: Record<string, unknown>;
}
interface AuditResponse { records: AuditRecord[]; next_cursor?: number }

const MIB = 1024 * 1024;

function bytesText(value?: number): string {
  if (value == null) return "";
  if (value < 1024) return `${value} B`;
  if (value < MIB) return `${(value / 1024).toFixed(1)} KiB`;
  if (value < 1024 * MIB) return `${(value / MIB).toFixed(1)} MiB`;
  return `${(value / (1024 * MIB)).toFixed(2)} GiB`;
}
function when(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { year: "numeric", month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

const KIND_OPTIONS: { value: string; label: string }[] = [
  { value: "", label: "Every kind" },
  { value: "statement", label: "Statements" },
  { value: "statement.finished", label: "Statements · finished" },
  { value: "statement.failed,statement.rejected", label: "Statements · failed or rejected" },
  { value: "statement.canceled", label: "Statements · cancelled" },
  { value: "catalog", label: "Catalog changes" },
  { value: "settings", label: "Settings changes" },
  { value: "auth", label: "Authentication failures" },
];

function kindClass(kind: string): string {
  if (kind === "statement.failed" || kind === "statement.rejected") return g.kindFailed;
  if (kind.startsWith("statement")) return g.kindStatement;
  if (kind.startsWith("catalog")) return g.kindCatalog;
  if (kind.startsWith("settings")) return g.kindSettings;
  return g.kindAuth;
}

function What({ r }: { r: AuditRecord }) {
  if (r.kind.startsWith("statement")) {
    return (
      <>
        {r.query_id ? <Link href={`/engine/queries/${encodeURIComponent(r.query_id)}`} className={g.link}><span className={g.stmt}>{r.statement || r.query_id}</span></Link> : <span className={g.stmt}>{r.statement}</span>}
        <span className={g.detail}>
          {[r.catalog && r.schema ? `${r.catalog}.${r.schema}` : null, r.resource_group ? `group ${r.resource_group}` : null,
            r.mode, r.elapsed_ms != null ? `${r.elapsed_ms} ms` : null, r.admission_wait_ms ? `waited ${r.admission_wait_ms} ms` : null,
            r.rows != null ? `${r.rows.toLocaleString()} rows` : null, r.bytes_scanned ? `${bytesText(r.bytes_scanned)} scanned` : null,
            r.error_code, r.error].filter(Boolean).join(" · ")}
        </span>
      </>
    );
  }
  if (r.kind.startsWith("catalog")) {
    return <span className={g.detail}>{r.object_type} <b>{r.object_id}</b>{r.revision_before != null || r.revision_after != null ? ` · revision ${r.revision_before ?? "–"} → ${r.revision_after ?? "–"}` : ""}{r.details ? ` · ${JSON.stringify(r.details)}` : ""}</span>;
  }
  if (r.kind.startsWith("settings")) return <span className={g.detail}>{r.details ? JSON.stringify(r.details) : ""}</span>;
  return <span className={g.detail}>{r.route}{r.error_code ? ` · ${r.error_code}` : ""}</span>;
}

interface AuditFilters { principal: string; kind: string; since: string; until: string }
const NO_FILTERS: AuditFilters = { principal: "", kind: "", since: "", until: "" };

function auditParams(filters: AuditFilters, cursor?: number): URLSearchParams {
  const params = new URLSearchParams();
  if (filters.principal.trim()) params.set("principal", filters.principal.trim());
  if (filters.kind) params.set("kind", filters.kind);
  if (filters.since) params.set("since", filters.since);
  if (filters.until) params.set("until", `${filters.until}T23:59:59.999Z`);
  params.set("limit", "100");
  if (cursor != null) params.set("cursor", String(cursor));
  return params;
}

function AuditCard() {
  const [draft, setDraft] = useState<AuditFilters>(NO_FILTERS);
  // The filters in force: the first page is read whenever they change.
  const [applied, setApplied] = useState<AuditFilters>(NO_FILTERS);
  const [records, setRecords] = useState<AuditRecord[]>([]);
  const [cursor, setCursor] = useState<number | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const fetchPage = useCallback(async (filters: AuditFilters, after: number | undefined, append: boolean) => {
    setLoading(true); setError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/engine/audit?${auditParams(filters, after).toString()}`);
      if (!res.ok) { setError(await readError(res, "The audit ledger could not be read")); return; }
      const page: AuditResponse = await res.json();
      setRecords(prev => (append ? [...prev, ...page.records] : page.records));
      setCursor(page.next_cursor);
      setLoaded(true);
    } catch {
      setError("The API could not be reached.");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void fetchPage(applied, undefined, false); }, [applied, fetchPage]);

  const query = () => auditParams(applied);
  const exportParams = query();
  exportParams.delete("limit");
  exportParams.set("format", "jsonl");
  const exportHref = `/api/kaveon/api/v1/engine/audit?${exportParams.toString()}`;

  return (
    <section className={s.card} aria-labelledby="audit-title">
      <div className={s.cardHead}>
        <div className={s.cardId}>
          <div className={s.mark}><i className="fas fa-clipboard-list" /></div>
          <div style={{ minWidth: 0 }}>
            <h2 id="audit-title" className={s.cardTitle}>Audit</h2>
            <p className={s.cardSub}>Statements, configuration changes and sign-in attempts. Automated actions show the account that ran them.</p>
          </div>
        </div>
        <a className={s.ghost} href={exportHref} download="kaveon-audit.jsonl"><i className="fas fa-download" aria-hidden="true" />Export JSONL</a>
      </div>

      <form className={g.filters} onSubmit={e => { e.preventDefault(); setApplied({ ...draft }); }}>
        <label className={g.field}><span className={g.fieldLabel}>Principal</span><input className={`${g.input} ${g.name}`} value={draft.principal} onChange={e => setDraft({ ...draft, principal: e.target.value })} placeholder="anyone" /></label>
        <label className={g.field}><span className={g.fieldLabel}>Kind</span>
          <select className={g.select} value={draft.kind} onChange={e => setDraft({ ...draft, kind: e.target.value })}>{KIND_OPTIONS.map(opt => <option key={opt.value} value={opt.value}>{opt.label}</option>)}</select>
        </label>
        <label className={g.field}><span className={g.fieldLabel}>From</span><input type="date" className={g.input} value={draft.since} onChange={e => setDraft({ ...draft, since: e.target.value })} /></label>
        <label className={g.field}><span className={g.fieldLabel}>To</span><input type="date" className={g.input} value={draft.until} onChange={e => setDraft({ ...draft, until: e.target.value })} /></label>
        <button type="submit" className={`${s.ghost} ${s.primary}`} disabled={loading}>{loading ? "Reading…" : "Apply filters"}</button>
      </form>

      {error && <div className={g.problem} role="alert">{error}</div>}
      <div className={g.tableWrap}>
        <table className={g.table}>
          <thead><tr><th>When</th><th>Kind</th><th>Account</th><th>What</th></tr></thead>
          <tbody>
            {loaded && records.length === 0 && <tr><td colSpan={4} className={g.empty}>Nothing in the ledger matches these filters.</td></tr>}
            {records.map(r => (
              <tr key={r.seq}>
                <td className="mono" title={`seq ${r.seq}`}>{when(r.ts_ms)}</td>
                <td><span className={`${g.kind} ${kindClass(r.kind)}`}>{r.kind}</span></td>
                <td>{r.display_identity ?? r.principal ?? <span className={g.detail}>Not recorded</span>}{r.display_identity && r.principal && <div className={g.detail}>{r.principal}</div>}{r.kind.startsWith("auth.") && <div className={g.detail}>Presented at sign-in</div>}{r.role ? <span className={g.detail}> · {r.role}</span> : null}</td>
                <td className="wrap"><What r={r} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className={g.foot}>
        <span>{records.length ? `${records.length.toLocaleString()} record${records.length === 1 ? "" : "s"} shown` : ""}</span>
        {cursor != null && <button type="button" className={s.ghost} onClick={() => void fetchPage(applied, cursor, true)} disabled={loading}>{loading ? "Reading…" : "Load more"}</button>}
      </div>
    </section>
  );
}

export default function AuditPage() {
  const { isAdmin, loading } = useRole();
  if (loading || !isAdmin) return null;
  return <AuditCard />;
}
