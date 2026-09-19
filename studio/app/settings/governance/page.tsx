"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import s from "../settings.module.css";
import g from "./governance.module.css";

// ── Types: the Engine's documents, as the platform proxies them ───────────────

type Role = "reader" | "analyst" | "admin";

interface ResourceGroup {
  name: string;
  max_memory_bytes?: number;
  max_concurrent: number;
  max_queued: number;
  max_queue_wait_seconds: number;
  max_local_parallelism?: number;
  priority: number;
  default_settings?: Record<string, unknown>;
}
interface Selector { principal?: string; principal_prefix?: string; role?: Role; client_tag?: string; group: string }
interface GroupCounters {
  name: string; running: number; queued: number; admitted: number; queued_total: number; rejected: number; withdrawn: number;
  admitted_bytes: number; wait_ms_p50?: number; wait_ms_p95?: number;
}
interface GovernanceDocument {
  source: string; store_path: string; admission_limit_bytes: number;
  groups: ResourceGroup[]; selectors: Selector[]; counters: GroupCounters[];
}

interface AuditRecord {
  seq: number; ts_ms: number; kind: string;
  principal?: string; role?: string; route?: string; query_id?: string;
  client?: string; source?: string; catalog?: string; schema?: string;
  statement?: string; resource_group?: string;
  admission_wait_ms?: number; elapsed_ms?: number; rows?: number; bytes_scanned?: number; mode?: string;
  error_code?: string; error?: string;
  object_type?: string; object_id?: string; revision_before?: number; revision_after?: number;
  details?: Record<string, unknown>;
}
interface AuditPage { records: AuditRecord[]; next_cursor?: number }

// ── Editable rows: strings while typed, numbers when saved ────────────────────

interface GroupDraft {
  key: number; name: string; max_memory_mib: string; max_concurrent: string; max_queued: string;
  max_queue_wait_seconds: string; max_local_parallelism: string; priority: string; default_settings: string;
}
interface SelectorDraft { key: number; principal: string; principal_prefix: string; role: "" | Role; client_tag: string; group: string }

let nextKey = 1;
const MIB = 1024 * 1024;

function toGroupDraft(group: ResourceGroup): GroupDraft {
  return {
    key: nextKey++,
    name: group.name,
    max_memory_mib: group.max_memory_bytes != null ? String(Math.round(group.max_memory_bytes / MIB)) : "",
    max_concurrent: String(group.max_concurrent),
    max_queued: String(group.max_queued),
    max_queue_wait_seconds: String(group.max_queue_wait_seconds),
    max_local_parallelism: group.max_local_parallelism != null ? String(group.max_local_parallelism) : "",
    priority: String(group.priority),
    default_settings: group.default_settings && Object.keys(group.default_settings).length ? JSON.stringify(group.default_settings) : "",
  };
}
function toSelectorDraft(selector: Selector): SelectorDraft {
  return {
    key: nextKey++,
    principal: selector.principal ?? "", principal_prefix: selector.principal_prefix ?? "",
    role: selector.role ?? "", client_tag: selector.client_tag ?? "", group: selector.group,
  };
}
function newGroupDraft(): GroupDraft {
  return { key: nextKey++, name: "", max_memory_mib: "", max_concurrent: "4", max_queued: "16", max_queue_wait_seconds: "60", max_local_parallelism: "", priority: "1", default_settings: "" };
}

/** A positive integer within [min, max], or the reason it is not. */
function integer(label: string, value: string, min: number, max: number, optional = false): { value?: number; error?: string } {
  if (value.trim() === "") return optional ? {} : { error: `${label} is required` };
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max) return { error: `${label} must be a whole number between ${min} and ${max}` };
  return { value: parsed };
}

/** The document to PUT, or the first problem found, in the row it was found. */
function assemble(groups: GroupDraft[], selectors: SelectorDraft[], admissionLimitBytes: number): { document?: { groups: ResourceGroup[]; selectors: Selector[] }; error?: string } {
  const names = new Set<string>();
  const built: ResourceGroup[] = [];
  for (const draft of groups) {
    const name = draft.name.trim();
    if (!name) return { error: "Every group needs a name" };
    if (name !== draft.name) return { error: `Group '${draft.name}' has leading or trailing spaces` };
    if (names.has(name)) return { error: `Group '${name}' is named twice` };
    names.add(name);
    const where = `Group '${name}': `;
    const concurrent = integer(`${where}max concurrent`, draft.max_concurrent, 1, 10000);
    const queued = integer(`${where}max queued`, draft.max_queued, 0, 10000);
    const wait = integer(`${where}max queue wait`, draft.max_queue_wait_seconds, 1, 86400);
    const priority = integer(`${where}priority`, draft.priority, 1, 1000);
    const memory = integer(`${where}memory share`, draft.max_memory_mib, 1, Math.max(1, Math.floor(admissionLimitBytes / MIB)), true);
    const parallelism = integer(`${where}max local parallelism`, draft.max_local_parallelism, 1, 1024, true);
    const problem = [concurrent, queued, wait, priority, memory, parallelism].find(check => check.error);
    if (problem) return { error: problem.error };
    let settings: Record<string, unknown> | undefined;
    if (draft.default_settings.trim()) {
      try {
        const parsed: unknown = JSON.parse(draft.default_settings);
        if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("object");
        settings = parsed as Record<string, unknown>;
      } catch {
        return { error: `${where}default settings must be a JSON object such as {"result_cache": false}` };
      }
    }
    built.push({
      name,
      max_concurrent: concurrent.value as number,
      max_queued: queued.value as number,
      max_queue_wait_seconds: wait.value as number,
      priority: priority.value as number,
      ...(memory.value != null ? { max_memory_bytes: memory.value * MIB } : {}),
      ...(parallelism.value != null ? { max_local_parallelism: parallelism.value } : {}),
      ...(settings ? { default_settings: settings } : {}),
    });
  }
  if (!names.has("default")) return { error: "A group named 'default' is required: statements no selector matches go there" };
  const builtSelectors: Selector[] = [];
  for (const [index, draft] of selectors.entries()) {
    if (!names.has(draft.group)) return { error: `Selector ${index + 1} names group '${draft.group}', which does not exist` };
    const selector: Selector = { group: draft.group };
    if (draft.principal.trim()) selector.principal = draft.principal.trim();
    if (draft.principal_prefix.trim()) selector.principal_prefix = draft.principal_prefix.trim();
    if (draft.role) selector.role = draft.role;
    if (draft.client_tag.trim()) selector.client_tag = draft.client_tag.trim();
    builtSelectors.push(selector);
  }
  const catchAll = builtSelectors.findIndex(sel => !sel.principal && !sel.principal_prefix && !sel.role && !sel.client_tag);
  if (catchAll !== -1 && catchAll !== builtSelectors.length - 1) return { error: `Selector ${catchAll + 1} matches everything, so the selectors after it can never match; move it last` };
  return { document: { groups: built, selectors: builtSelectors } };
}

function mib(value?: number): string {
  if (value == null) return "";
  return value >= 1024 * MIB ? `${(value / (1024 * MIB)).toFixed(1)} GiB` : `${Math.round(value / MIB)} MiB`;
}
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
async function readError(res: Response, fallback: string): Promise<string> {
  try {
    const body = await res.json();
    const detail = body?.detail ?? body?.error ?? body?.message;
    if (typeof detail === "string") return detail;
    if (detail && typeof detail.message === "string") return detail.message;
  } catch { /* not JSON */ }
  return `${fallback} (HTTP ${res.status})`;
}

// ── Resource groups ───────────────────────────────────────────────────────────

function ResourceGroupsCard() {
  const [doc, setDoc] = useState<GovernanceDocument | null | undefined>(undefined);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [groups, setGroups] = useState<GroupDraft[]>([]);
  const [selectors, setSelectors] = useState<SelectorDraft[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [dirty, setDirty] = useState(false);

  const load = useCallback(async () => {
    setLoadError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/engine/admin/resource-groups`);
      if (!res.ok) { setDoc(null); setLoadError(await readError(res, "Resource groups could not be read")); return; }
      const data: GovernanceDocument = await res.json();
      setDoc(data);
      setGroups(data.groups.map(toGroupDraft));
      setSelectors(data.selectors.map(toSelectorDraft));
      setDirty(false);
    } catch {
      setDoc(null); setLoadError("The API could not be reached.");
    }
  }, []);
  useEffect(() => { void load(); }, [load]);

  const counters = useMemo(() => new Map((doc?.counters ?? []).map(c => [c.name, c])), [doc]);
  const groupNames = groups.map(gr => gr.name.trim()).filter(Boolean);

  const editGroup = (key: number, patch: Partial<GroupDraft>) => { setGroups(prev => prev.map(gr => (gr.key === key ? { ...gr, ...patch } : gr))); setDirty(true); setSaved(null); };
  const editSelector = (key: number, patch: Partial<SelectorDraft>) => { setSelectors(prev => prev.map(sel => (sel.key === key ? { ...sel, ...patch } : sel))); setDirty(true); setSaved(null); };
  const moveSelector = (index: number, delta: number) => {
    setSelectors(prev => {
      const next = [...prev]; const target = index + delta;
      if (target < 0 || target >= next.length) return prev;
      [next[index], next[target]] = [next[target], next[index]];
      return next;
    });
    setDirty(true); setSaved(null);
  };

  const save = async () => {
    if (!doc) return;
    setProblem(null); setSaved(null);
    const assembled = assemble(groups, selectors, doc.admission_limit_bytes);
    if (assembled.error || !assembled.document) { setProblem(assembled.error ?? "The document could not be assembled"); return; }
    setSaving(true);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/engine/admin/resource-groups`, {
        method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(assembled.document),
      });
      if (!res.ok) { setProblem(await readError(res, "KaveonDB refused the resource groups")); return; }
      const data: GovernanceDocument = await res.json();
      setDoc(data); setGroups(data.groups.map(toGroupDraft)); setSelectors(data.selectors.map(toSelectorDraft));
      setDirty(false); setSaved(`Applied to the coordinator and written to ${data.store_path}. Statements admitted from now on use these groups.`);
    } catch {
      setProblem("The API could not be reached; nothing was changed.");
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className={s.card} aria-labelledby="rg-title">
      <div className={s.cardHead}>
        <div className={s.cardId}>
          <div className={s.mark}><i className="fas fa-scale-balanced" /></div>
          <div style={{ minWidth: 0 }}>
            <h2 id="rg-title" className={s.cardTitle}>Resource groups</h2>
            <p className={s.cardSub}>What each group of principals may hold of the coordinator: running statements, memory share, queue and wait. Selectors are tried in order; unmatched statements go to <code>default</code>.</p>
          </div>
        </div>
        <div style={{ display: "flex", gap: 8 }}>
          <button type="button" className={s.ghost} onClick={() => void load()} disabled={saving}>Reload</button>
          <button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => void save()} disabled={!doc || saving || !dirty}>{saving ? "Applying…" : "Apply"}</button>
        </div>
      </div>

      {doc === undefined && !loadError && <p className={s.note} style={{ marginTop: 12 }}>Reading the coordinator…</p>}
      {loadError && <div className={g.problem} role="alert">{loadError}</div>}
      {doc && (
        <>
          <div className={s.facts}>
            <div className={s.fact}><div className={s.factLabel}>Source</div><div className={s.factValue}>{doc.source.replace("_", " ")}</div></div>
            <div className={s.fact}><div className={s.factLabel}>Admission pool</div><div className={s.factValue}>{mib(doc.admission_limit_bytes)}</div></div>
            <div className={s.fact}><div className={s.factLabel}>Durable copy</div><div className={s.factValue} title={doc.store_path}>{doc.store_path}</div></div>
          </div>

          <div className={g.tableWrap}>
            <table className={g.table}>
              <thead>
                <tr>
                  <th>Group</th><th className="num">Concurrent</th><th className="num">Queued</th><th className="num">Wait (s)</th>
                  <th className="num">Memory (MiB)</th><th className="num">Threads</th><th className="num">Priority</th><th>Default settings</th>
                  <th className="num">Running · waiting</th><th className="num">Admitted · rejected</th><th className="num">Wait p50 · p95</th><th />
                </tr>
              </thead>
              <tbody>
                {groups.map(gr => {
                  const c = counters.get(gr.name.trim());
                  return (
                    <tr key={gr.key}>
                      <td><input className={`${g.input} ${g.name}`} value={gr.name} onChange={e => editGroup(gr.key, { name: e.target.value })} aria-label="Group name" aria-invalid={!gr.name.trim()} /></td>
                      <td className="num"><input className={`${g.input} ${g.num}`} inputMode="numeric" value={gr.max_concurrent} onChange={e => editGroup(gr.key, { max_concurrent: e.target.value })} aria-label="Max concurrent" /></td>
                      <td className="num"><input className={`${g.input} ${g.num}`} inputMode="numeric" value={gr.max_queued} onChange={e => editGroup(gr.key, { max_queued: e.target.value })} aria-label="Max queued" /></td>
                      <td className="num"><input className={`${g.input} ${g.num}`} inputMode="numeric" value={gr.max_queue_wait_seconds} onChange={e => editGroup(gr.key, { max_queue_wait_seconds: e.target.value })} aria-label="Max queue wait seconds" /></td>
                      <td className="num"><input className={`${g.input} ${g.num}`} inputMode="numeric" placeholder="whole pool" value={gr.max_memory_mib} onChange={e => editGroup(gr.key, { max_memory_mib: e.target.value })} aria-label="Memory share in MiB" /></td>
                      <td className="num"><input className={`${g.input} ${g.num}`} inputMode="numeric" placeholder="node's" value={gr.max_local_parallelism} onChange={e => editGroup(gr.key, { max_local_parallelism: e.target.value })} aria-label="Max local parallelism" /></td>
                      <td className="num"><input className={`${g.input} ${g.num}`} inputMode="numeric" value={gr.priority} onChange={e => editGroup(gr.key, { priority: e.target.value })} aria-label="Priority" /></td>
                      <td><input className={`${g.input} ${g.wide}`} placeholder='{"result_cache": false}' value={gr.default_settings} onChange={e => editGroup(gr.key, { default_settings: e.target.value })} aria-label="Default settings JSON" /></td>
                      <td className="num"><span className={`${g.counter} ${c && (c.running || c.queued) ? g.counterHot : ""}`}>{c ? `${c.running} · ${c.queued}` : "–"}</span></td>
                      <td className="num"><span className={g.counter}>{c ? `${c.admitted.toLocaleString()} · ${c.rejected.toLocaleString()}` : "–"}</span></td>
                      <td className="num"><span className={g.counter}>{c && c.wait_ms_p50 != null ? `${c.wait_ms_p50} · ${c.wait_ms_p95 ?? "–"} ms` : "–"}</span></td>
                      <td><div className={g.rowActions}><button type="button" className={g.iconBtn} title="Remove group" aria-label={`Remove group ${gr.name}`} disabled={gr.name.trim() === "default"} onClick={() => { setGroups(prev => prev.filter(x => x.key !== gr.key)); setDirty(true); }}><i className="fas fa-xmark" /></button></div></td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <div style={{ marginTop: 10 }}>
            <button type="button" className={s.ghost} onClick={() => { setGroups(prev => [...prev, newGroupDraft()]); setDirty(true); }}><i className="fas fa-plus" aria-hidden="true" />Add group</button>
          </div>

          <h3 className={s.cardTitle} style={{ fontSize: 13.5, marginTop: 18 }}>Selectors</h3>
          <p className={s.cardSub}>Every matcher on a row must hold. A row with no matcher is the catch-all and must be last.</p>
          <div className={g.tableWrap}>
            <table className={g.table}>
              <thead>
                <tr><th>Order</th><th>Principal</th><th>Principal prefix</th><th>Role</th><th>Client tag</th><th>Group</th><th /></tr>
              </thead>
              <tbody>
                {selectors.length === 0 && <tr><td colSpan={7} className={g.empty}>No selectors: every statement is admitted through <code>default</code>.</td></tr>}
                {selectors.map((sel, index) => (
                  <tr key={sel.key}>
                    <td className="num">{index + 1}</td>
                    <td><input className={`${g.input} ${g.name}`} value={sel.principal} onChange={e => editSelector(sel.key, { principal: e.target.value })} aria-label="Principal" placeholder="exact" /></td>
                    <td><input className={`${g.input} ${g.name}`} value={sel.principal_prefix} onChange={e => editSelector(sel.key, { principal_prefix: e.target.value })} aria-label="Principal prefix" placeholder="svc-" /></td>
                    <td>
                      <select className={g.select} value={sel.role} onChange={e => editSelector(sel.key, { role: e.target.value as SelectorDraft["role"] })} aria-label="Role">
                        <option value="">any</option><option value="reader">reader</option><option value="analyst">analyst</option><option value="admin">admin</option>
                      </select>
                    </td>
                    <td><input className={`${g.input} ${g.name}`} value={sel.client_tag} onChange={e => editSelector(sel.key, { client_tag: e.target.value })} aria-label="Client tag" placeholder="etl" /></td>
                    <td>
                      <select className={g.select} value={sel.group} onChange={e => editSelector(sel.key, { group: e.target.value })} aria-label="Group">
                        {!groupNames.includes(sel.group) && <option value={sel.group}>{sel.group}</option>}
                        {groupNames.map(name => <option key={name} value={name}>{name}</option>)}
                      </select>
                    </td>
                    <td>
                      <div className={g.rowActions}>
                        <button type="button" className={g.iconBtn} title="Move up" aria-label="Move selector up" disabled={index === 0} onClick={() => moveSelector(index, -1)}><i className="fas fa-arrow-up" /></button>
                        <button type="button" className={g.iconBtn} title="Move down" aria-label="Move selector down" disabled={index === selectors.length - 1} onClick={() => moveSelector(index, 1)}><i className="fas fa-arrow-down" /></button>
                        <button type="button" className={g.iconBtn} title="Remove selector" aria-label="Remove selector" onClick={() => { setSelectors(prev => prev.filter(x => x.key !== sel.key)); setDirty(true); }}><i className="fas fa-xmark" /></button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div style={{ marginTop: 10 }}>
            <button type="button" className={s.ghost} onClick={() => { setSelectors(prev => [...prev, { key: nextKey++, principal: "", principal_prefix: "", role: "", client_tag: "", group: groupNames[0] ?? "default" }]); setDirty(true); }}><i className="fas fa-plus" aria-hidden="true" />Add selector</button>
          </div>

          {problem && <div className={g.problem} role="alert">{problem}</div>}
          {saved && <div className={g.saved} role="status">{saved}</div>}
          <p className={s.note} style={{ marginTop: 12 }}>
            Within a group statements run in arrival order. Across groups the one furthest below its priority-weighted share of the pool is next, and the pool is held for it until its head fits. The rule and every bound are in the <Link href="/docs/engine" className={g.link}>Engine documentation</Link>.
          </p>
        </>
      )}
    </section>
  );
}

// ── Audit ─────────────────────────────────────────────────────────────────────

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
      const page: AuditPage = await res.json();
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
            <p className={s.cardSub}>Who did what on the coordinator: statements, catalog changes, settings changes and refused sign-ins, oldest first, kept for the configured retention.</p>
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
          <thead><tr><th>When</th><th>Kind</th><th>Principal</th><th>What</th></tr></thead>
          <tbody>
            {loaded && records.length === 0 && <tr><td colSpan={4} className={g.empty}>Nothing in the ledger matches these filters.</td></tr>}
            {records.map(r => (
              <tr key={r.seq}>
                <td className="mono" title={`seq ${r.seq}`}>{when(r.ts_ms)}</td>
                <td><span className={`${g.kind} ${kindClass(r.kind)}`}>{r.kind}</span></td>
                <td>{r.principal ?? <span className={g.detail}>anonymous</span>}{r.role ? <span className={g.detail}> · {r.role}</span> : null}</td>
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

export default function GovernancePage() {
  const { isAdmin, loading } = useRole();
  if (loading || !isAdmin) return null;
  return (
    <div className={s.stack}>
      <ResourceGroupsCard />
      <AuditCard />
    </div>
  );
}
