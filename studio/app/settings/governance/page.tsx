"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import { readError } from "../readError";
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

export default function GovernancePage() {
  const { isAdmin, loading } = useRole();
  if (loading || !isAdmin) return null;
  return <ResourceGroupsCard />;
}
