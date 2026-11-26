"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import s from "../settings.module.css";
import g from "../governance/governance.module.css";

// ── Types: the Engine's catalog access documents, as the platform proxies them ─

type Access = "browse" | "query" | "manage";
const ACCESS_LEVELS: Access[] = ["browse", "query", "manage"];

interface Grant {
  principal: string; catalog: string; access: Access; revision: number; granted_by: string; granted_at_ms: number;
}
interface Reserved { name: string; grantable: boolean; visible_to: string; reason: string }
interface AccessDocument {
  store: { enabled: boolean; generation?: number | null; snapshot_id?: string | null };
  catalogs: string[];
  reserved: Reserved[];
  roles: { reader: Access; analyst: Access; admin: "all" };
  grants: Grant[];
}
interface EffectiveGrant extends Grant { registered: boolean; effective: { reader: Access; analyst: Access; admin: "all" } }
interface EffectiveDocument { principal: string; store_enabled: boolean; grants: EffectiveGrant[]; ungranted: string[] }
interface Proposal { principal: string; role_seen: string; catalog: string; access: Access }
interface ImportDocument {
  ledger_enabled: boolean; principals_seen: number; catalogs: string[]; proposed: Proposal[]; applied: boolean; recorded: Grant[]; generation?: number;
}

// The Engine's roles, in the platform's words. The platform's Analyst and
// Editor both reach the Engine as `analyst`; Editor keeps its own gate on
// the registration routes.
const ROLE_LABEL: Record<"reader" | "analyst", string> = { reader: "Viewer", analyst: "Analyst / Editor" };

function ceiling(access: Access, role: "reader" | "analyst", roles: AccessDocument["roles"]): Access {
  const limit = roles[role];
  return ACCESS_LEVELS.indexOf(access) <= ACCESS_LEVELS.indexOf(limit) ? access : limit;
}

function when(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleString(undefined, { year: "numeric", month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit" });
}

interface Failure { status: number; code?: string; message: string }

async function readFailure(res: Response, fallback: string): Promise<Failure> {
  try {
    const body = await res.json();
    const detail = body?.detail ?? body?.error ?? body?.message;
    if (typeof detail === "string") return { status: res.status, message: detail };
    if (detail && typeof detail.message === "string") return { status: res.status, code: detail.code, message: detail.message };
  } catch { /* not JSON */ }
  return { status: res.status, message: `${fallback} (HTTP ${res.status})` };
}

// ── Grants ───────────────────────────────────────────────────────────────────

export function Assignments() {
  const [doc, setDoc] = useState<AccessDocument | null | undefined>(undefined);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [principal, setPrincipal] = useState("");
  const [catalog, setCatalog] = useState("");
  const [access, setAccess] = useState<Access>("query");
  // Edits in progress on existing grants: the level chosen per row.
  const [edits, setEdits] = useState<Record<string, Access>>({});

  const load = useCallback(async () => {
    setLoadError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/engine/admin/catalog-access`);
      if (!res.ok) { setDoc(null); setLoadError((await readFailure(res, "Catalog access could not be read")).message); return; }
      const data: AccessDocument = await res.json();
      setDoc(data);
      setEdits({});
      setCatalog(current => (current && data.catalogs.includes(current) ? current : data.catalogs[0] ?? ""));
    } catch {
      setDoc(null); setLoadError("The API could not be reached.");
    }
  }, []);
  useEffect(() => { void load(); }, [load]);

  const key = (grant: Pick<Grant, "principal" | "catalog">) => `${grant.catalog}/${grant.principal}`;

  // A conflict means the grants moved under this page: reload, and say so.
  const afterFailure = async (failure: Failure) => {
    if (failure.status === 409) {
      setProblem(`${failure.message} The list has been reloaded; review it and apply the change again.`);
      await load();
    } else {
      setProblem(failure.message);
    }
  };

  const grant = async (body: { principal: string; catalog: string; access: Access; revision?: number }, done: string) => {
    setProblem(null); setSaved(null); setBusy(true);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/engine/admin/catalog-access/grants`, {
        method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
      });
      if (!res.ok) { await afterFailure(await readFailure(res, "The grant was refused")); return; }
      setSaved(done);
      await load();
    } catch {
      setProblem("The API could not be reached; nothing was changed.");
    } finally {
      setBusy(false);
    }
  };

  const add = async () => {
    const who = principal.trim();
    if (!who) { setProblem("Enter the principal: the account exactly as it signs in, for example an email address."); return; }
    if (/\s/.test(who)) { setProblem("A principal has no spaces."); return; }
    if (!catalog) { setProblem("Choose a catalog."); return; }
    await grant({ principal: who, catalog, access }, `Granted ${who} ${access} on ${catalog}.`);
    setPrincipal("");
  };

  const change = async (existing: Grant) => {
    const next = edits[key(existing)];
    if (!next || next === existing.access) return;
    await grant({ principal: existing.principal, catalog: existing.catalog, access: next, revision: existing.revision },
      `Changed ${existing.principal} on ${existing.catalog} from ${existing.access} to ${next}.`);
  };

  const revoke = async (existing: Grant) => {
    if (!window.confirm(`Revoke ${existing.principal}'s ${existing.access} access to ${existing.catalog}? Their next request is refused at the Engine.`)) return;
    setProblem(null); setSaved(null); setBusy(true);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/engine/admin/catalog-access/grants`, {
        method: "DELETE", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ principal: existing.principal, catalog: existing.catalog, revision: existing.revision }),
      });
      if (!res.ok) { await afterFailure(await readFailure(res, "The revoke was refused")); return; }
      setSaved(`Revoked ${existing.principal} on ${existing.catalog}.`);
      await load();
    } catch {
      setProblem("The API could not be reached; nothing was changed.");
    } finally {
      setBusy(false);
    }
  };

  const grants = useMemo(() => [...(doc?.grants ?? [])].sort((a, b) => a.principal.localeCompare(b.principal) || a.catalog.localeCompare(b.catalog)), [doc]);
  const storeOff = doc ? !doc.store.enabled : false;

  return (
    <section className={s.card} aria-labelledby="ca-title">
      <div className={s.cardHead}>
        <div className={s.cardId}>
          <div className={s.mark}><i className="fas fa-key" /></div>
          <div style={{ minWidth: 0 }}>
            <h2 id="ca-title" className={s.cardTitle}>Catalog assignments</h2>
            <p className={s.cardSub}>Grant an account access to a catalog. Its sign-in role limits what it can do.</p>
          </div>
        </div>
        <div style={{ display: "flex", gap: 8 }}>
          <button type="button" className={s.ghost} onClick={() => void load()} disabled={busy}>Reload</button>
        </div>
      </div>

      {doc === undefined && !loadError && <p className={s.note} style={{ marginTop: 12 }}>Reading the coordinator…</p>}
      {loadError && <div className={g.problem} role="alert">{loadError}</div>}
      {doc && (
        <>
          {storeOff && (
            <div className={g.problem} role="alert">
              The KaveonDB transaction store is not configured on this coordinator, so no grant can be recorded. Until it is, catalogs are visible to administrators only.
            </div>
          )}
          <div className={g.tableWrap}>
            <table className={g.table}>
              <thead>
                <tr><th>Principal</th><th>Catalog</th><th>Access</th><th>Details</th><th>Actions</th></tr>
              </thead>
              <tbody>
                {grants.length === 0 && <tr><td colSpan={5} className={g.empty}>No catalog assignments yet.</td></tr>}
                {grants.map(grant => {
                  const id = key(grant);
                  const chosen = edits[id] ?? grant.access;
                  return (
                    <tr key={id}>
                      <td className="mono">{grant.principal}</td>
                      <td>{grant.catalog}{!doc.catalogs.includes(grant.catalog) && <span className={g.detail}> · not registered</span>}</td>
                      <td>
                        <select className={g.select} value={chosen} onChange={e => setEdits(prev => ({ ...prev, [id]: e.target.value as Access }))} aria-label={`Access for ${grant.principal} on ${grant.catalog}`} disabled={busy || storeOff}>
                          {ACCESS_LEVELS.map(level => <option key={level} value={level}>{level}</option>)}
                        </select>
                      </td>
                      <td>
                        <details>
                          <summary>Details</summary>
                          <div>Granted by {grant.granted_by} · {when(grant.granted_at_ms)}</div>
                          <div>Viewer: {ceiling(grant.access, "reader", doc.roles)}</div>
                          <div>Analyst / Editor: {ceiling(grant.access, "analyst", doc.roles)}</div>
                          <div>Revision {grant.revision}</div>
                        </details>
                      </td>
                      <td>
                        <div className={g.rowActions}>
                          <button type="button" className={s.ghost} style={{ height: 26 }} onClick={() => void change(grant)} disabled={busy || storeOff || chosen === grant.access}>Apply</button>
                          <button type="button" className={g.iconBtn} title="Revoke" aria-label={`Revoke ${grant.principal} on ${grant.catalog}`} onClick={() => void revoke(grant)} disabled={busy || storeOff}><i className="fas fa-xmark" /></button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>

          <div className={g.filters}>
            <label className={g.field}><span className={g.fieldLabel}>Principal</span>
              <input className={`${g.input} ${g.wide}`} value={principal} onChange={e => setPrincipal(e.target.value)} placeholder="name@example.com" aria-label="Principal to grant" disabled={busy || storeOff} />
            </label>
            <label className={g.field}><span className={g.fieldLabel}>Catalog</span>
              <select className={g.select} value={catalog} onChange={e => setCatalog(e.target.value)} aria-label="Catalog to grant" disabled={busy || storeOff || doc.catalogs.length === 0}>
                {doc.catalogs.length === 0 && <option value="">No catalogs registered</option>}
                {doc.catalogs.map(name => <option key={name} value={name}>{name}</option>)}
              </select>
            </label>
            <label className={g.field}><span className={g.fieldLabel}>Access</span>
              <select className={g.select} value={access} onChange={e => setAccess(e.target.value as Access)} aria-label="Access level to grant" disabled={busy || storeOff}>
                {ACCESS_LEVELS.map(level => <option key={level} value={level}>{level}</option>)}
              </select>
            </label>
            <button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => void add()} disabled={busy || storeOff || !doc.catalogs.length}><i className="fas fa-plus" aria-hidden="true" />Grant</button>
          </div>

          {problem && <div className={g.problem} role="alert">{problem}</div>}
          {saved && <div className={g.saved} role="status">{saved}</div>}
          <details className={s.previewOptions}>
            <summary>About access levels</summary>
            <p className={s.note}><b>Browse</b> lists tables and columns. <b>Query</b> also reads rows. <b>Manage</b> also permits catalog changes within the assigned role. Registering schemas and tables requires Editor or Admin.</p>
            <p className={s.note}>Viewers can browse. Administrators have access by role. Review grant changes in <Link href="/settings/audit" className={g.link}>Audit</Link>.</p>
            {doc.reserved.map(item => <p key={item.name} className={s.note}><b>{item.name}</b> is reserved. {item.visible_to === "none" ? "Hidden for all roles." : "Only administrators can view it."}</p>)}
          </details>
        </>
      )}
    </section>
  );
}

// ── Effective access ─────────────────────────────────────────────────────────

export function CheckAccess() {
  const [principal, setPrincipal] = useState("");
  const [doc, setDoc] = useState<EffectiveDocument | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const lookup = useCallback(async (who: string) => {
    const name = who.trim();
    if (!name) return;
    setBusy(true); setError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/engine/admin/catalog-access/effective/${encodeURIComponent(name)}`);
      if (!res.ok) { setDoc(null); setError((await readFailure(res, "Effective access could not be read")).message); return; }
      setDoc(await res.json());
    } catch {
      setDoc(null); setError("The API could not be reached.");
    } finally {
      setBusy(false);
    }
  }, []);
  return (
    <section className={s.card} aria-labelledby="ea-title">
      <div className={s.cardHead}>
        <div className={s.cardId}>
          <div className={s.mark}><i className="fas fa-user-shield" /></div>
          <div style={{ minWidth: 0 }}>
            <h2 id="ea-title" className={s.cardTitle}>Check access</h2>
            <p className={s.cardSub}>Enter an account to see its catalog grants and the access each sign-in role allows.</p>
          </div>
        </div>
      </div>
      <div className={g.filters}>
        <label className={g.field}><span className={g.fieldLabel}>Principal</span>
          <input className={`${g.input} ${g.wide}`} value={principal} onChange={e => setPrincipal(e.target.value)} onKeyDown={e => { if (e.key === "Enter") void lookup(principal); }} placeholder="name@example.com" aria-label="Principal to look up" />
        </label>
        <button type="button" className={s.ghost} onClick={() => void lookup(principal)} disabled={busy || !principal.trim()}>Look up</button>
      </div>
      {error && <div className={g.problem} role="alert">{error}</div>}
      {doc && (
        <>
          <div className={g.tableWrap}>
            <table className={g.table}>
              <thead><tr><th>Catalog</th><th>Granted</th><th>As {ROLE_LABEL.reader}</th><th>As {ROLE_LABEL.analyst}</th><th className="num">Revision</th></tr></thead>
              <tbody>
                {/* With the store off no grant can exist, so an empty list is a
                    fact about the coordinator, not about this person. Stated as
                    a finding about them it reads as a decision somebody made.
                    `store_enabled` is what the server sends to tell the two
                    apart, and it was being ignored. */}
                {doc.grants.length === 0 && (
                  <tr>
                    <td colSpan={5} className={g.empty}>
                      {doc.store_enabled
                        ? `${doc.principal} has no grant: as a non-administrator they see no catalog.`
                        : "The KaveonDB transaction store is not configured on this coordinator, so no grant can be recorded or read for anybody. This is not a finding about this principal."}
                    </td>
                  </tr>
                )}
                {doc.grants.map(grant => (
                  <tr key={grant.catalog}>
                    <td>{grant.catalog}{!grant.registered && <span className={g.detail}> · not registered</span>}</td>
                    <td>{grant.access}</td>
                    <td><span className={g.counter}>{grant.effective.reader}</span></td>
                    <td><span className={g.counter}>{grant.effective.analyst}</span></td>
                    <td className="num">{grant.revision}</td>
                  </tr>
                ))}
                {doc.ungranted.map(name => (
                  <tr key={`none-${name}`}>
                    <td>{name}</td><td><span className={g.detail}>none</span></td><td><span className={g.detail}>hidden</span></td><td><span className={g.detail}>hidden</span></td><td className="num" />
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </section>
  );
}

// ── Reconciliation of an open deployment ─────────────────────────────────────

export function ImportGrants() {
  const [proposal, setProposal] = useState<ImportDocument | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (apply: boolean) => {
    if (apply && !window.confirm(`Record ${proposal?.proposed.length ?? 0} grants from the open policy? Each is written to the audit ledger under your account.`)) return;
    setBusy(true); setError(null); setDone(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/engine/admin/catalog-access/import`, {
        method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ source: "open", apply }),
      });
      if (!res.ok) { setError((await readFailure(res, "The import was refused")).message); return; }
      const data: ImportDocument = await res.json();
      if (apply) {
        setDone(`Recorded ${data.recorded.length} grant${data.recorded.length === 1 ? "" : "s"}.`);
        setProposal(null);
      } else {
        setProposal(data);
      }
    } catch {
      setError("The API could not be reached; nothing was changed.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className={s.card} aria-labelledby="im-title">
      <div className={s.cardHead}>
        <div className={s.cardId}>
          <div className={s.mark}><i className="fas fa-clock-rotate-left" /></div>
          <div style={{ minWidth: 0 }}>
            <h2 id="im-title" className={s.cardTitle}>Import grants</h2>
            <p className={s.cardSub}>For deployments that previously allowed open access. Preview proposed grants for accounts in the audit log, then choose whether to apply the full proposal.</p>
          </div>
        </div>
        <div style={{ display: "flex", gap: 8 }}>
          <button type="button" className={s.ghost} onClick={() => void run(false)} disabled={busy}>Preview proposal</button>
          <button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => void run(true)} disabled={busy || !proposal || proposal.proposed.length === 0}>Record these grants</button>
        </div>
      </div>
      {error && <div className={g.problem} role="alert">{error}</div>}
      {done && <div className={g.saved} role="status">{done}</div>}
      {proposal && (
        <>
          <div className={s.facts}>
            <div className={s.fact}><div className={s.factLabel}>Principals in the ledger</div><div className={s.factValue}>{proposal.principals_seen}</div></div>
            <div className={s.fact}><div className={s.factLabel}>Catalogs</div><div className={s.factValue}>{proposal.catalogs.join(", ") || "none"}</div></div>
            <div className={s.fact}><div className={s.factLabel}>Proposed</div><div className={s.factValue}>{proposal.proposed.length}</div></div>
          </div>
          {!proposal.ledger_enabled && <p className={s.note} style={{ marginTop: 12 }}>The audit ledger is not enabled on this coordinator, so no principal can be proposed from it.</p>}
          <div className={g.tableWrap}>
            <table className={g.table}>
              <thead><tr><th>Principal</th><th>Role seen</th><th>Catalog</th><th>Access</th></tr></thead>
              <tbody>
                {/* With the ledger off there is nothing to propose from, so the
                    row must not claim every principal is already granted — the
                    note above says the ledger is disabled and this row
                    contradicted it. */}
                {proposal.proposed.length === 0 && (
                  <tr>
                    <td colSpan={4} className={g.empty}>
                      {proposal.ledger_enabled
                        ? "Nothing to record: every principal the ledger has seen already holds a grant on every catalog, or the ledger is empty."
                        : "Nothing can be proposed while the audit ledger is disabled on this coordinator."}
                    </td>
                  </tr>
                )}
                {proposal.proposed.map(row => (
                  <tr key={`${row.catalog}/${row.principal}`}><td className="mono">{row.principal}</td><td>{row.role_seen}</td><td>{row.catalog}</td><td>{row.access}</td></tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </section>
  );
}
