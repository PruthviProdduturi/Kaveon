"use client";

/**
 * Setup Modal
 *
 * Guides the user through connecting a metadata database to LoomX.
 * Supports: Microsoft Fabric SQL, Azure SQL, PostgreSQL, MySQL.
 */

import React, { useState, useEffect } from "react";
import { msalFetch } from "../utils/msalFetch";
import { API_BASE } from "../config";
import { LoomXLogo } from "./LoomXLogo";

// ─── Types ────────────────────────────────────────────────────────────────────

type DbType = "fabric_sql" | "azure_sql" | "postgresql" | "mysql";

type SetupPhase =
  | "not_configured"
  | "connection_failed"
  | "access_denied"
  | "db_not_found"
  | "schema_missing"
  | "enter_connection"
  | "testing"
  | "initializing"
  | "success";

interface SetupError {
  message: string;
  error_type: string;
  level?: "error" | "warning" | "info";
  extra?: { issue_codes: Array<{ code: number; message: string }> };
}

export interface SetupData {
  status: string;
  endpoint?: string;
  database?: string;
  errors?: SetupError[];
}

interface SetupModalProps {
  data: SetupData;
  onComplete: () => void;
}

// ─── DB type definitions ──────────────────────────────────────────────────────

interface DbTypeConfig {
  label: string;
  icon: string;
  iconText?: string;
  defaultPort?: number;
  usesEndpoint: boolean;
  usesConnectionString?: boolean;
  beta?: boolean;
  endpointLabel: string;
  endpointPlaceholder: string;
  endpointHint: string;
  dbHint: string;
}

const DB_TYPES: Record<DbType, DbTypeConfig> = {
  fabric_sql: {
    label: "Microsoft Fabric SQL",
    icon: "",
    iconText: "F",
    usesEndpoint: false,
    usesConnectionString: true,
    endpointLabel: "",
    endpointPlaceholder: "",
    endpointHint: "",
    dbHint: "",
  },
  azure_sql: {
    label: "Azure SQL Database",
    icon: "fa-cloud",
    usesEndpoint: true,
    endpointLabel: "Server Name",
    endpointPlaceholder: "my-server.database.windows.net",
    endpointHint: "Found in Azure Portal → SQL Server → Server name.",
    dbHint: "The Azure SQL database that will store LoomX metadata.",
  },
  postgresql: {
    label: "PostgreSQL",
    icon: "fa-database",
    defaultPort: 5432,
    usesEndpoint: false,
    beta: true,
    endpointLabel: "Host",
    endpointPlaceholder: "localhost or db.example.com",
    endpointHint: "Hostname or IP address of your PostgreSQL server.",
    dbHint: "The PostgreSQL database name. It must already exist.",
  },
  mysql: {
    label: "MySQL / MariaDB",
    icon: "fa-database",
    defaultPort: 3306,
    usesEndpoint: false,
    beta: true,
    endpointLabel: "Host",
    endpointPlaceholder: "localhost or db.example.com",
    endpointHint: "Hostname or IP address of your MySQL or MariaDB server.",
    dbHint: "The MySQL database name. It must already exist.",
  },
};

// ─── Error hints ──────────────────────────────────────────────────────────────

const ERROR_HINTS: Record<string, { title: string; hint: string }> = {
  connection_failed: {
    title: "Cannot reach this host",
    hint: "Check the hostname for typos and confirm the required port is reachable from this machine.",
  },
  timeout: {
    title: "Connection timed out",
    hint: "The host port may be blocked by a corporate firewall or VPN. Contact your network team if needed.",
  },
  access_denied: {
    title: "Access denied",
    hint: "Authentication failed. Check your credentials, or verify Azure AD role assignments for MSSQL-based databases.",
  },
  db_not_found: {
    title: "Database not found",
    hint: "The database name was not found at this host. Names are case-sensitive — copy the exact name.",
  },
};

// ─── Styles ───────────────────────────────────────────────────────────────────

const S = {
  overlay: {
    position: "fixed", inset: 0, background: "rgba(10, 16, 30, 0.93)",
    display: "flex", alignItems: "center", justifyContent: "center",
    zIndex: 9999, padding: "20px",
  } as React.CSSProperties,

  card: {
    background: "#1e293b", border: "1px solid #2d3f5c", borderRadius: 18,
    padding: "36px 40px 40px", width: "100%", maxWidth: 520,
    boxShadow: "0 32px 72px rgba(0,0,0,0.6)", overflowY: "auto" as const,
    maxHeight: "calc(100vh - 40px)",
  } as React.CSSProperties,

  logoRow: { display: "flex", justifyContent: "center", marginBottom: 4 } as React.CSSProperties,
  heading: { fontSize: 21, fontWeight: 700, color: "#f1f5f9", margin: "16px 0 6px" } as React.CSSProperties,
  sub: { fontSize: 13.5, color: "#94a3b8", lineHeight: 1.65, marginBottom: 20 } as React.CSSProperties,

  label: {
    display: "block", fontSize: 12, fontWeight: 700, color: "#94a3b8",
    textTransform: "uppercase" as const, letterSpacing: "0.06em", marginBottom: 5,
  } as React.CSSProperties,

  input: (hasError: boolean): React.CSSProperties => ({
    width: "100%", background: "#0f172a",
    border: `1px solid ${hasError ? "#ef4444" : "#334155"}`,
    borderRadius: 8, padding: "10px 14px", fontSize: 13, color: "#f1f5f9",
    outline: "none", boxSizing: "border-box", marginBottom: 4, transition: "border-color 0.15s",
  }),

  inputRow: { display: "flex", gap: 10 } as React.CSSProperties,

  hint: { fontSize: 11.5, color: "#475569", marginBottom: 14, lineHeight: 1.5 } as React.CSSProperties,

  btnPrimary: (disabled = false): React.CSSProperties => ({
    width: "100%",
    background: disabled ? "#1e3a5f" : "linear-gradient(135deg, #3b82f6 0%, #6366f1 100%)",
    border: "none", borderRadius: 10, padding: "12px 20px", fontSize: 14,
    fontWeight: 600, color: disabled ? "#475569" : "#fff",
    cursor: disabled ? "not-allowed" : "pointer", marginBottom: 10, transition: "opacity 0.15s",
  }),

  btnSecondary: {
    width: "100%", background: "transparent", border: "1px solid #334155",
    borderRadius: 10, padding: "11px 20px", fontSize: 14, fontWeight: 500,
    color: "#64748b", cursor: "pointer", marginBottom: 10,
  } as React.CSSProperties,

  dbTypeGrid: { display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, marginBottom: 20 } as React.CSSProperties,

  betaBadge: {
    fontSize: 9, fontWeight: 700, color: "#f59e0b", background: "rgba(245,158,11,0.12)",
    border: "1px solid rgba(245,158,11,0.3)", borderRadius: 4, padding: "1px 5px",
    letterSpacing: "0.05em", textTransform: "uppercase" as const, flexShrink: 0,
  } as React.CSSProperties,

  dbTypeCard: (active: boolean, disabled: boolean): React.CSSProperties => ({
    background: active ? "rgba(99,102,241,0.15)" : "#0f172a",
    border: `1px solid ${active ? "#6366f1" : "#1e293b"}`,
    borderRadius: 10, padding: "10px 12px", cursor: disabled ? "not-allowed" : "pointer",
    display: "flex", alignItems: "center", gap: 8, transition: "all 0.15s",
    opacity: disabled ? 0.5 : 1,
  }),

  dbTypeLabel: (active: boolean): React.CSSProperties => ({
    fontSize: 12, fontWeight: active ? 700 : 500,
    color: active ? "#a5b4fc" : "#64748b", lineHeight: 1.3,
  }),

  errorBox: {
    background: "rgba(239,68,68,0.08)", border: "1px solid rgba(239,68,68,0.3)",
    borderRadius: 10, padding: "12px 16px", marginBottom: 18,
  } as React.CSSProperties,

  errorTitle: { fontSize: 13, fontWeight: 700, color: "#f87171", marginBottom: 4 } as React.CSSProperties,
  errorHint: { fontSize: 12, color: "#fca5a5", lineHeight: 1.6, marginBottom: 6 } as React.CSSProperties,
  errorRaw: { fontSize: 11, color: "#6b7280", fontFamily: "monospace", wordBreak: "break-all" as const, marginTop: 6 } as React.CSSProperties,

  successBox: {
    background: "rgba(34,197,94,0.08)", border: "1px solid rgba(34,197,94,0.3)",
    borderRadius: 10, padding: "12px 16px", marginBottom: 18, fontSize: 13, color: "#86efac",
    display: "flex", alignItems: "center", gap: 8,
  } as React.CSSProperties,

  infoBox: {
    background: "rgba(59,130,246,0.07)", border: "1px solid rgba(59,130,246,0.25)",
    borderRadius: 10, padding: "12px 16px", marginBottom: 20, fontSize: 12,
    color: "#93c5fd", lineHeight: 1.8, wordBreak: "break-all" as const,
  } as React.CSSProperties,
} as const;

// ─── Step bar ─────────────────────────────────────────────────────────────────

type StepState = "done" | "active" | "pending";

function StepDot({ state }: { state: StepState }) {
  const colors: Record<StepState, string> = { done: "#22c55e", active: "#6366f1", pending: "#334155" };
  return <div style={{ width: 10, height: 10, borderRadius: "50%", background: colors[state], transition: "background 0.2s" }} />;
}

function StepBar({ step, testOk, phase }: { step: 1 | 2 | 3; testOk: boolean; phase: SetupPhase }) {
  const isInit = phase === "initializing" || phase === "success";
  const s1: StepState = "done";
  const s2: StepState = testOk || isInit ? "done" : step >= 2 ? "active" : "pending";
  const s3: StepState = phase === "success" ? "done" : isInit ? "active" : "pending";
  const connector = (done: boolean) => (
    <div style={{ flex: 1, height: 1, background: done ? "#22c55e" : "#334155", margin: "0 6px", transition: "background 0.2s" }} />
  );
  return (
    <div style={{ marginBottom: 28 }}>
      <div style={{ display: "flex", alignItems: "center", marginBottom: 6 }}>
        <StepDot state={s1} />{connector(s2 === "done")}<StepDot state={s2} />{connector(s3 !== "pending")}<StepDot state={s3} />
      </div>
      <div style={{ display: "flex", justifyContent: "space-between", fontSize: 10.5, color: "#475569", letterSpacing: "0.04em", textTransform: "uppercase" as const }}>
        <span style={{ color: "#94a3b8" }}>Enter Details</span>
        <span style={{ color: s2 !== "pending" ? "#94a3b8" : undefined }}>Test</span>
        <span style={{ color: s3 !== "pending" ? "#94a3b8" : undefined }}>Initialize</span>
      </div>
    </div>
  );
}

// ─── Error display ────────────────────────────────────────────────────────────

function ErrorDisplay({ errors, rawMessage }: { errors?: SetupError[]; rawMessage?: string }) {
  const primary = errors?.[0];
  const errType = primary?.error_type ?? "connection_failed";
  const info = ERROR_HINTS[errType];
  const issueMsgs = (primary?.extra?.issue_codes ?? []).map((ic) => ic.message);
  const raw = primary?.message ?? rawMessage;
  return (
    <div style={S.errorBox}>
      {info && <div style={S.errorTitle}>{info.title}</div>}
      {info && <div style={S.errorHint}>{info.hint}</div>}
      {issueMsgs.map((m, i) => (
        <div key={i} style={{ ...S.errorHint, color: "#fcd34d", fontSize: 11.5 }}>
          <i className="fas fa-lightbulb" style={{ marginRight: 5 }} />{m}
        </div>
      ))}
      {raw && <div style={S.errorRaw}>{raw}</div>}
    </div>
  );
}

// ─── DB type picker ───────────────────────────────────────────────────────────

function DbTypePicker({ value, onChange, disabled }: { value: DbType; onChange: (t: DbType) => void; disabled: boolean }) {
  const types: DbType[] = ["fabric_sql", "azure_sql", "postgresql", "mysql"];
  return (
    <div>
      <label style={S.label}>Database Type</label>
      <div style={S.dbTypeGrid}>
        {types.map((t) => {
          const cfg = DB_TYPES[t];
          const active = value === t;
          return (
            <div key={t} style={S.dbTypeCard(active, disabled)} onClick={() => !disabled && onChange(t)}>
              {cfg.iconText ? (
                <span style={{
                  fontSize: 13, fontWeight: 800, fontStyle: "italic",
                  color: active ? "#6366f1" : "#475569",
                  flexShrink: 0, width: 14, textAlign: "center" as const,
                  fontFamily: "Georgia, serif",
                }}>{cfg.iconText}</span>
              ) : (
                <i className={`fas ${cfg.icon}`} style={{ fontSize: 14, color: active ? "#6366f1" : "#475569", flexShrink: 0 }} />
              )}
              <span style={{ flex: 1, ...S.dbTypeLabel(active) }}>{cfg.label}</span>
              {cfg.beta && <span style={S.betaBadge}>Beta</span>}
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ─── Main component ───────────────────────────────────────────────────────────

export function SetupModal({ data, onComplete }: SetupModalProps) {
  const initialPhase = data.status === "schema_missing" ? "schema_missing" : (data.status as SetupPhase);

  const [phase, setPhase] = useState<SetupPhase>(initialPhase);
  const [dbType, setDbType] = useState<DbType>("fabric_sql");
  const [connectionString, setConnectionString] = useState("");
  const [endpoint, setEndpoint] = useState(data.endpoint ?? "");
  const [database, setDatabase] = useState(data.database ?? "");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [errors, setErrors] = useState<SetupError[] | null>(null);
  const [testOk, setTestOk] = useState(false);

  const cfg = DB_TYPES[dbType];

  useEffect(() => {
    if (phase !== "testing" && phase !== "initializing" && phase !== "success") {
      setPhase(initialPhase);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data.status]);

  function handleDbTypeChange(t: DbType) {
    setDbType(t);
    setErrors(null);
    setTestOk(false);
    const newCfg = DB_TYPES[t];
    if (!newCfg.usesEndpoint && !port) {
      setPort(String(newCfg.defaultPort ?? ""));
    }
  }

  function clearValidation() { setErrors(null); setTestOk(false); }

  function buildPayload() {
    if (cfg.usesConnectionString) {
      return { db_type: dbType, connection_string: connectionString.trim() };
    }
    if (cfg.usesEndpoint) {
      return { db_type: dbType, endpoint: endpoint.trim(), database: database.trim() };
    }
    return {
      db_type: dbType,
      host: host.trim(),
      port: parseInt(port) || cfg.defaultPort,
      database: database.trim(),
    };
  }

  function canTest() {
    if (cfg.usesConnectionString) return !!connectionString.trim();
    if (!database.trim()) return false;
    if (cfg.usesEndpoint) return !!endpoint.trim();
    return !!host.trim();
  }

  async function handleTest() {
    setPhase("testing");
    setErrors(null);
    setTestOk(false);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/setup/test`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(buildPayload()),
      });
      if (res.ok) {
        setTestOk(true);
        setPhase("enter_connection");
      } else {
        const body = await res.json();
        setErrors(body.errors ?? null);
        setPhase("enter_connection");
      }
    } catch (err) {
      setErrors([{ message: err instanceof Error ? err.message : "Network error", error_type: "connection_failed", level: "error" }]);
      setPhase("enter_connection");
    }
  }

  async function handleInitialize(overridePayload?: object) {
    setPhase("initializing");
    setErrors(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/setup/initialize`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(overridePayload ?? buildPayload()),
      });
      if (res.ok) {
        setPhase("success");
        setTimeout(() => window.location.reload(), 3500);
      } else {
        const body = await res.json();
        setErrors(body.errors ?? null);
        setPhase("enter_connection");
      }
    } catch (err) {
      setErrors([{ message: err instanceof Error ? err.message : "Network error", error_type: "connection_failed", level: "error" }]);
      setPhase("enter_connection");
    }
  }

  // ── Connection form ──────────────────────────────────────────────────────────

  function ConnectionForm({ cancelTo }: { cancelTo?: SetupPhase }) {
    const isWorking = phase === "testing";
    const hasError = !!errors && !testOk;

    return (
      <>
        <StepBar step={testOk ? 2 : 1} testOk={testOk} phase={phase} />
        <h2 style={S.heading}>Connect Metadata Database</h2>
        <p style={S.sub}>
          LoomX stores datasets, charts, and dashboards in a database you control.
          Choose a type and enter your connection details.
        </p>

        <DbTypePicker value={dbType} onChange={handleDbTypeChange} disabled={isWorking} />

        {errors && !testOk && <ErrorDisplay errors={errors} />}
        {testOk && (
          <div style={S.successBox}>
            <i className="fas fa-check-circle" />
            Connection successful — ready to initialise.
          </div>
        )}

        {/* Fabric SQL — ODBC connection string */}
        {cfg.usesConnectionString && (
          <>
            <label style={S.label} htmlFor="setup-connstr">ODBC Connection String</label>
            <textarea
              id="setup-connstr"
              style={{
                ...S.input(hasError),
                resize: "vertical",
                minHeight: 90,
                fontFamily: "monospace",
                fontSize: 12,
              }}
              placeholder={
                "Server=tcp:xyz.database.fabric.microsoft.com,1433;" +
                "Initial Catalog=MyDatabase;" +
                "Authentication=ActiveDirectoryInteractive;Encrypt=True;"
              }
              value={connectionString}
              onChange={(e) => { clearValidation(); setConnectionString(e.target.value); }}
              disabled={isWorking}
              autoComplete="off"
              spellCheck={false}
            />
            <p style={S.hint}>
              Copy from Fabric workspace → SQL Database → Connection strings → ODBC.
              LoomX extracts the server and database automatically.
            </p>
          </>
        )}

        {/* Host / Endpoint (Azure SQL and non-MSSQL types) */}
        {!cfg.usesConnectionString && (
          <>
            <label style={S.label} htmlFor="setup-ep">{cfg.endpointLabel}</label>
            <input
              id="setup-ep"
              style={S.input(hasError)}
              type="text"
              placeholder={cfg.endpointPlaceholder}
              value={cfg.usesEndpoint ? endpoint : host}
              onChange={(e) => { clearValidation(); cfg.usesEndpoint ? setEndpoint(e.target.value) : setHost(e.target.value); }}
              disabled={isWorking}
              autoComplete="off"
              spellCheck={false}
            />
            <p style={S.hint}>{cfg.endpointHint}</p>
          </>
        )}

        {/* Non-MSSQL: database + port on one row, then username + password */}
        {!cfg.usesEndpoint && !cfg.usesConnectionString && (
          <>
            <div style={S.inputRow}>
              <div style={{ flex: 1 }}>
                <label style={S.label} htmlFor="setup-db">Database Name</label>
                <input
                  id="setup-db"
                  style={{ ...S.input(hasError), marginBottom: 0 }}
                  type="text"
                  placeholder="my_loomx_db"
                  value={database}
                  onChange={(e) => { clearValidation(); setDatabase(e.target.value); }}
                  disabled={isWorking}
                  autoComplete="off"
                />
              </div>
              <div style={{ width: 90 }}>
                <label style={S.label} htmlFor="setup-port">Port</label>
                <input
                  id="setup-port"
                  style={{ ...S.input(false), marginBottom: 0 }}
                  type="number"
                  placeholder={String(cfg.defaultPort)}
                  value={port}
                  onChange={(e) => { clearValidation(); setPort(e.target.value); }}
                  disabled={isWorking}
                />
              </div>
            </div>
            <p style={{ ...S.hint, marginBottom: 16 }}>{cfg.dbHint}</p>
            <p style={{ ...S.hint, marginBottom: 20, color: "#4ade80" }}>
              <i className="fas fa-shield-check" style={{ marginRight: 6 }} />
              Connects via Azure AD Managed Identity — no credentials required.
            </p>
          </>
        )}

        {/* Azure SQL: database name below endpoint */}
        {cfg.usesEndpoint && !cfg.usesConnectionString && (
          <>
            <label style={S.label} htmlFor="setup-db">Database Name</label>
            <input
              id="setup-db"
              style={S.input(hasError)}
              type="text"
              placeholder={dbType === "fabric_sql" ? "MyLoomXMetadata" : "loomx"}
              value={database}
              onChange={(e) => { clearValidation(); setDatabase(e.target.value); }}
              disabled={isWorking}
              autoComplete="off"
            />
            <p style={{ ...S.hint, marginBottom: 20 }}>{cfg.dbHint}</p>
          </>
        )}

        {testOk ? (
          <button style={S.btnPrimary()} onClick={() => handleInitialize()}>
            <i className="fas fa-magic" style={{ marginRight: 8 }} />Initialize Database
          </button>
        ) : (
          <button
            style={S.btnPrimary(!canTest() || isWorking)}
            onClick={handleTest}
            disabled={!canTest() || isWorking}
          >
            {isWorking
              ? <><i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />Testing…</>
              : <><i className="fas fa-plug" style={{ marginRight: 8 }} />Test Connection</>}
          </button>
        )}

        {cancelTo && (
          <button style={S.btnSecondary} onClick={() => setPhase(cancelTo)} disabled={isWorking}>Cancel</button>
        )}
      </>
    );
  }

  // ── Error state pages ────────────────────────────────────────────────────────

  function ErrorState({ title, description, errType }: { title: string; description: string; errType: string }) {
    const info = ERROR_HINTS[errType];
    const issueMsgs = (data.errors?.[0]?.extra?.issue_codes ?? []).map((ic) => ic.message);
    return (
      <>
        <h2 style={S.heading}>{title}</h2>
        <p style={S.sub}>{description}</p>
        {data.endpoint && (
          <div style={S.infoBox}>
            <strong>Endpoint</strong>&nbsp; {data.endpoint}<br />
            <strong>Database</strong>&nbsp; {data.database}
          </div>
        )}
        <div style={S.errorBox}>
          {info && <div style={S.errorTitle}>{info.title}</div>}
          {info && <div style={S.errorHint}>{info.hint}</div>}
          {issueMsgs.map((m, i) => (
            <div key={i} style={{ ...S.errorHint, color: "#fcd34d", fontSize: 11.5 }}>
              <i className="fas fa-lightbulb" style={{ marginRight: 5 }} />{m}
            </div>
          ))}
          {data.errors?.[0]?.message && <div style={S.errorRaw}>{data.errors[0].message}</div>}
        </div>
        <button style={S.btnPrimary()} onClick={() => {
          if (data.endpoint) setEndpoint(data.endpoint);
          if (data.database) setDatabase(data.database);
          setErrors(null); setTestOk(false); setPhase("enter_connection");
        }}>
          <i className="fas fa-edit" style={{ marginRight: 8 }} />Edit Connection
        </button>
        <button style={S.btnSecondary} onClick={() => window.location.reload()}>
          <i className="fas fa-redo" style={{ marginRight: 8 }} />Retry
        </button>
      </>
    );
  }

  // ── Render ───────────────────────────────────────────────────────────────────

  return (
    <div style={S.overlay}>
      <div style={S.card}>
        <div style={S.logoRow}><LoomXLogo size={48} animate="pulse" /></div>

        {(phase === "not_configured" || phase === "enter_connection") && (
          <ConnectionForm cancelTo={phase === "enter_connection" && initialPhase !== "not_configured" ? initialPhase : undefined} />
        )}

        {phase === "connection_failed" && (
          <ErrorState errType="connection_failed" title="Cannot Connect"
            description="LoomX cannot reach the configured metadata database. Check your connection details and retry." />
        )}

        {phase === "access_denied" && (
          <ErrorState errType="access_denied" title="Access Denied"
            description="LoomX reached the host but authentication failed. Verify credentials or Azure AD role assignments." />
        )}

        {phase === "db_not_found" && (
          <ErrorState errType="db_not_found" title="Database Not Found"
            description="The host was reached but the database does not exist. Verify the exact name (case-sensitive)." />
        )}

        {phase === "schema_missing" && (
          <>
            <StepBar step={3} testOk phase="schema_missing" />
            <h2 style={S.heading}>Initialize Database</h2>
            <p style={S.sub}>
              LoomX connected successfully. The required tables haven't been created yet —
              click below and LoomX will set them up in seconds.
            </p>
            <div style={S.infoBox}>
              <i className="fas fa-check-circle" style={{ marginRight: 6, color: "#4ade80" }} />
              <strong>Endpoint</strong>&nbsp; {data.endpoint ?? endpoint}<br />
              <i className="fas fa-database" style={{ marginRight: 6, marginTop: 4, marginLeft: 1 }} />
              <strong>Database</strong>&nbsp; {data.database ?? database}
            </div>
            {errors && <ErrorDisplay errors={errors} />}
            <button style={S.btnPrimary()} onClick={() => {
              const ep = data.endpoint ?? endpoint;
              const db = data.database ?? database;
              setEndpoint(ep); setDatabase(db);
              handleInitialize({ db_type: dbType, endpoint: ep, database: db });
            }}>
              <i className="fas fa-magic" style={{ marginRight: 8 }} />Initialize Database
            </button>
            <button style={S.btnSecondary} onClick={() => {
              if (data.endpoint) setEndpoint(data.endpoint);
              if (data.database) setDatabase(data.database);
              setErrors(null); setTestOk(true); setPhase("enter_connection");
            }}>
              <i className="fas fa-edit" style={{ marginRight: 8 }} />Use a Different Connection
            </button>
          </>
        )}

        {phase === "initializing" && (
          <>
            <StepBar step={3} testOk={false} phase="initializing" />
            <h2 style={S.heading}>Setting Up LoomX…</h2>
            <p style={S.sub}>Creating tables in your metadata database. This usually takes a few seconds.</p>
            <div style={{ textAlign: "center", padding: "28px 0", color: "#6366f1", fontSize: 38 }}>
              <i className="fas fa-spinner fa-spin" />
            </div>
          </>
        )}

        {phase === "success" && (
          <>
            <h2 style={{ ...S.heading, color: "#4ade80" }}>All Set!</h2>
            <p style={S.sub}>
              Metadata database initialised successfully. LoomX is restarting to apply the
              configuration — the page will reload automatically.
            </p>
            <div style={{ textAlign: "center", padding: "20px 0", color: "#4ade80", fontSize: 44 }}>
              <i className="fas fa-check-circle" />
            </div>
            <div style={{ textAlign: "center", color: "#475569", fontSize: 12.5, marginTop: 8 }}>
              <i className="fas fa-spinner fa-spin" style={{ marginRight: 6 }} />Reloading in a few seconds…
            </div>
          </>
        )}
      </div>
    </div>
  );
}
