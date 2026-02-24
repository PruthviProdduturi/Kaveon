"use client";

/**
 * Setup Modal
 *
 * Shown after login when the metadata database is not configured, unreachable,
 * or has not been initialised.  Guides the user through providing a Fabric SQL
 * connection and running the LoomX schema automatically.
 *
 * Modelled on Apache Superset's database connection wizard:
 *   - Step progress indicator (Enter → Test → Initialize)
 *   - SIP-40-style issue codes with actionable fix hints per error type
 *   - Errors auto-clear when the user edits the form (no stale messages)
 *   - 200 / 400 HTTP status from the API drives success vs. error branch
 *   - Endpoint format example shown inline (like Superset's URI help text)
 */

import React, { useState, useEffect } from "react";
import { msalFetch } from "../utils/msalFetch";
import { API_BASE } from "../config";
import { LoomXLogo } from "./LoomXLogo";

// ─── Types ────────────────────────────────────────────────────────────────────

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

/** Mirrors the SIP-40 error shape returned by the API. */
interface SetupError {
  message: string;
  error_type: string;
  level?: "error" | "warning" | "info";
  extra?: {
    issue_codes: Array<{ code: number; message: string }>;
  };
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

// ─── Issue-code hint messages (per error_type) ────────────────────────────────
// Mirrors Superset's per-code actionable guidance shown under the error box.

const ERROR_HINTS: Record<string, { title: string; hint: string }> = {
  connection_failed: {
    title: "Cannot reach this endpoint",
    hint:
      "Check for typos in the endpoint hostname and confirm port 1433 is " +
      "reachable from this machine. Fabric SQL endpoints look like: " +
      "xyz123-abc.datawarehouse.fabric.microsoft.com",
  },
  timeout: {
    title: "Connection timed out",
    hint:
      "Port 1433 (required by Fabric SQL / ODBC Driver 18) may be blocked " +
      "by a corporate firewall or VPN. Contact your network team if needed.",
  },
  access_denied: {
    title: "Access denied",
    hint:
      "Your Azure AD identity must have the Contributor or Member role on " +
      "the Fabric workspace. Ask your workspace admin to grant access, then retry.",
  },
  db_not_found: {
    title: "Database not found",
    hint:
      "The database name was not found at this endpoint. Database names are " +
      "case-sensitive in Fabric SQL — copy the exact name from your Fabric workspace.",
  },
};

// ─── Styles ───────────────────────────────────────────────────────────────────

const S = {
  overlay: {
    position: "fixed",
    inset: 0,
    background: "rgba(10, 16, 30, 0.93)",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    zIndex: 9999,
    padding: "20px",
  } as React.CSSProperties,

  card: {
    background: "#1e293b",
    border: "1px solid #2d3f5c",
    borderRadius: 18,
    padding: "36px 40px 40px",
    width: "100%",
    maxWidth: 500,
    boxShadow: "0 32px 72px rgba(0,0,0,0.6)",
    overflowY: "auto" as const,
    maxHeight: "calc(100vh - 40px)",
  } as React.CSSProperties,

  logoRow: {
    display: "flex",
    justifyContent: "center",
    marginBottom: 4,
  } as React.CSSProperties,

  heading: {
    fontSize: 21,
    fontWeight: 700,
    color: "#f1f5f9",
    margin: "16px 0 6px",
  } as React.CSSProperties,

  sub: {
    fontSize: 13.5,
    color: "#94a3b8",
    lineHeight: 1.65,
    marginBottom: 24,
  } as React.CSSProperties,

  label: {
    display: "block",
    fontSize: 12,
    fontWeight: 700,
    color: "#94a3b8",
    textTransform: "uppercase" as const,
    letterSpacing: "0.06em",
    marginBottom: 5,
  } as React.CSSProperties,

  input: (hasError: boolean): React.CSSProperties => ({
    width: "100%",
    background: "#0f172a",
    border: `1px solid ${hasError ? "#ef4444" : "#334155"}`,
    borderRadius: 8,
    padding: "10px 14px",
    fontSize: 13,
    color: "#f1f5f9",
    outline: "none",
    boxSizing: "border-box",
    marginBottom: 4,
    transition: "border-color 0.15s",
  }),

  hint: {
    fontSize: 11.5,
    color: "#475569",
    marginBottom: 14,
    lineHeight: 1.5,
  } as React.CSSProperties,

  btnPrimary: (disabled = false): React.CSSProperties => ({
    width: "100%",
    background: disabled
      ? "#1e3a5f"
      : "linear-gradient(135deg, #3b82f6 0%, #6366f1 100%)",
    border: "none",
    borderRadius: 10,
    padding: "12px 20px",
    fontSize: 14,
    fontWeight: 600,
    color: disabled ? "#475569" : "#fff",
    cursor: disabled ? "not-allowed" : "pointer",
    marginBottom: 10,
    transition: "opacity 0.15s",
  }),

  btnSecondary: {
    width: "100%",
    background: "transparent",
    border: "1px solid #334155",
    borderRadius: 10,
    padding: "11px 20px",
    fontSize: 14,
    fontWeight: 500,
    color: "#64748b",
    cursor: "pointer",
    marginBottom: 10,
  } as React.CSSProperties,

  errorBox: {
    background: "rgba(239,68,68,0.08)",
    border: "1px solid rgba(239,68,68,0.3)",
    borderRadius: 10,
    padding: "12px 16px",
    marginBottom: 18,
  } as React.CSSProperties,

  errorTitle: {
    fontSize: 13,
    fontWeight: 700,
    color: "#f87171",
    marginBottom: 4,
  } as React.CSSProperties,

  errorHint: {
    fontSize: 12,
    color: "#fca5a5",
    lineHeight: 1.6,
    marginBottom: 6,
  } as React.CSSProperties,

  errorRaw: {
    fontSize: 11,
    color: "#6b7280",
    fontFamily: "monospace",
    wordBreak: "break-all" as const,
    marginTop: 6,
  } as React.CSSProperties,

  successBox: {
    background: "rgba(34,197,94,0.08)",
    border: "1px solid rgba(34,197,94,0.3)",
    borderRadius: 10,
    padding: "12px 16px",
    marginBottom: 18,
    fontSize: 13,
    color: "#86efac",
    display: "flex",
    alignItems: "center",
    gap: 8,
  } as React.CSSProperties,

  infoBox: {
    background: "rgba(59,130,246,0.07)",
    border: "1px solid rgba(59,130,246,0.25)",
    borderRadius: 10,
    padding: "12px 16px",
    marginBottom: 20,
    fontSize: 12,
    color: "#93c5fd",
    lineHeight: 1.8,
    wordBreak: "break-all" as const,
  } as React.CSSProperties,
} as const;

// ─── Step progress indicator ──────────────────────────────────────────────────

type StepState = "done" | "active" | "pending";

function StepDot({ state }: { state: StepState }) {
  const colors: Record<StepState, string> = {
    done: "#22c55e",
    active: "#6366f1",
    pending: "#334155",
  };
  return (
    <div
      style={{
        width: 10,
        height: 10,
        borderRadius: "50%",
        background: colors[state],
        transition: "background 0.2s",
      }}
    />
  );
}

function StepBar({
  step,
  testOk,
  phase,
}: {
  step: 1 | 2 | 3;
  testOk: boolean;
  phase: SetupPhase;
}) {
  const isInit = phase === "initializing" || phase === "success";

  const s1: StepState = "done"; // always on (they're on this screen)
  const s2: StepState = testOk || isInit ? "done" : step >= 2 ? "active" : "pending";
  const s3: StepState = phase === "success" ? "done" : isInit ? "active" : "pending";

  const connector = (done: boolean) => (
    <div
      style={{
        flex: 1,
        height: 1,
        background: done ? "#22c55e" : "#334155",
        margin: "0 6px",
        transition: "background 0.2s",
      }}
    />
  );

  return (
    <div style={{ marginBottom: 28 }}>
      {/* Dots + connectors */}
      <div style={{ display: "flex", alignItems: "center", marginBottom: 6 }}>
        <StepDot state={s1} />
        {connector(s2 === "done")}
        <StepDot state={s2} />
        {connector(s3 !== "pending")}
        <StepDot state={s3} />
      </div>
      {/* Labels */}
      <div style={{ display: "flex", justifyContent: "space-between", fontSize: 10.5, color: "#475569", letterSpacing: "0.04em", textTransform: "uppercase" as const }}>
        <span style={{ color: "#94a3b8" }}>Enter Details</span>
        <span style={{ color: s2 !== "pending" ? "#94a3b8" : undefined }}>Test</span>
        <span style={{ color: s3 !== "pending" ? "#94a3b8" : undefined }}>Initialize</span>
      </div>
    </div>
  );
}

// ─── Error display ─────────────────────────────────────────────────────────────

function ErrorDisplay({ errors, rawMessage }: { errors?: SetupError[]; rawMessage?: string }) {
  const primary = errors?.[0];
  const errType = primary?.error_type ?? "connection_failed";
  const info = ERROR_HINTS[errType];

  // Collect all issue-code messages from the API
  const issueMsgs = (primary?.extra?.issue_codes ?? []).map((ic) => ic.message);
  const raw = primary?.message ?? rawMessage;

  return (
    <div style={S.errorBox}>
      {info && <div style={S.errorTitle}>{info.title}</div>}
      {info && <div style={S.errorHint}>{info.hint}</div>}
      {issueMsgs.map((m, i) => (
        <div key={i} style={{ ...S.errorHint, color: "#fcd34d", fontSize: 11.5 }}>
          <i className="fas fa-lightbulb" style={{ marginRight: 5 }} />
          {m}
        </div>
      ))}
      {raw && <div style={S.errorRaw}>{raw}</div>}
    </div>
  );
}

// ─── Component ────────────────────────────────────────────────────────────────

export function SetupModal({ data, onComplete }: SetupModalProps) {
  const initialPhase =
    data.status === "schema_missing"
      ? "schema_missing"
      : (data.status as SetupPhase);

  const [phase, setPhase] = useState<SetupPhase>(initialPhase);
  const [endpoint, setEndpoint] = useState(data.endpoint ?? "");
  const [database, setDatabase] = useState(data.database ?? "");
  const [errors, setErrors] = useState<SetupError[] | null>(null);
  const [testOk, setTestOk] = useState(false);

  useEffect(() => {
    if (phase !== "testing" && phase !== "initializing" && phase !== "success") {
      setPhase(initialPhase);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data.status]);

  // Superset pattern: clear ALL validation state when the user edits the form.
  function handleEndpointChange(v: string) {
    setEndpoint(v);
    setErrors(null);
    setTestOk(false);
  }
  function handleDatabaseChange(v: string) {
    setDatabase(v);
    setErrors(null);
    setTestOk(false);
  }

  // ── Handlers ──────────────────────────────────────────────────────────────

  async function handleTest() {
    setPhase("testing");
    setErrors(null);
    setTestOk(false);

    try {
      const res = await msalFetch(`${API_BASE}/api/v1/setup/test`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ endpoint: endpoint.trim(), database: database.trim() }),
      });

      if (res.ok) {
        // HTTP 200 = success (mirrors Superset's test_connection 200 OK)
        setTestOk(true);
        setPhase("enter_connection");
      } else {
        // HTTP 400 = connection failure with structured errors
        const body = await res.json();
        setErrors(body.errors ?? null);
        setPhase("enter_connection");
      }
    } catch (err) {
      setErrors([{
        message: err instanceof Error ? err.message : "Network error",
        error_type: "connection_failed",
        level: "error",
      }]);
      setPhase("enter_connection");
    }
  }

  async function handleInitialize(ep = endpoint, db = database) {
    setPhase("initializing");
    setErrors(null);

    try {
      const res = await msalFetch(`${API_BASE}/api/v1/setup/initialize`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ endpoint: ep.trim(), database: db.trim() }),
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
      setErrors([{
        message: err instanceof Error ? err.message : "Network error",
        error_type: "connection_failed",
        level: "error",
      }]);
      setPhase("enter_connection");
    }
  }

  // ── Connection form (shared by not_configured + enter_connection) ──────────

  function ConnectionForm({ cancelTo }: { cancelTo?: SetupPhase }) {
    const canTest = endpoint.trim().length > 0 && database.trim().length > 0;
    const isWorking = phase === "testing";

    return (
      <>
        <StepBar step={testOk ? 2 : 1} testOk={testOk} phase={phase} />

        <h2 style={S.heading}>Connect Metadata Database</h2>
        <p style={S.sub}>
          LoomX stores datasets, charts, and dashboards in a Microsoft Fabric SQL database.
          Enter the SQL endpoint and database name below.
        </p>

        {errors && !testOk && <ErrorDisplay errors={errors} />}

        {testOk && (
          <div style={S.successBox}>
            <i className="fas fa-check-circle" />
            Connection successful — ready to initialise.
          </div>
        )}

        <label style={S.label} htmlFor="setup-ep">SQL Endpoint</label>
        <input
          id="setup-ep"
          style={S.input(!!errors && !testOk)}
          type="text"
          placeholder="xyz123-abc.datawarehouse.fabric.microsoft.com"
          value={endpoint}
          onChange={(e) => handleEndpointChange(e.target.value)}
          disabled={isWorking}
          autoComplete="off"
          spellCheck={false}
        />
        <p style={S.hint}>
          Found in Fabric workspace → SQL analytics endpoint → copy the server address.
        </p>

        <label style={S.label} htmlFor="setup-db">Database Name</label>
        <input
          id="setup-db"
          style={S.input(!!errors && !testOk)}
          type="text"
          placeholder="MyLoomXMetadata"
          value={database}
          onChange={(e) => handleDatabaseChange(e.target.value)}
          disabled={isWorking}
          autoComplete="off"
        />
        <p style={{ ...S.hint, marginBottom: 20 }}>
          The Fabric SQL database that will store LoomX metadata. Names are case-sensitive.
        </p>

        {testOk ? (
          <button style={S.btnPrimary()} onClick={() => handleInitialize()}>
            <i className="fas fa-magic" style={{ marginRight: 8 }} />
            Initialize Database
          </button>
        ) : (
          <button
            style={S.btnPrimary(!canTest || isWorking)}
            onClick={handleTest}
            disabled={!canTest || isWorking}
          >
            {isWorking
              ? <><i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />Testing…</>
              : <><i className="fas fa-plug" style={{ marginRight: 8 }} />Test Connection</>}
          </button>
        )}

        {cancelTo && (
          <button style={S.btnSecondary} onClick={() => setPhase(cancelTo)} disabled={isWorking}>
            Cancel
          </button>
        )}
      </>
    );
  }

  // ── Error state pages (connection_failed / access_denied / db_not_found) ───

  function ErrorState({
    title,
    description,
    errType,
  }: {
    title: string;
    description: string;
    errType: string;
  }) {
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
              <i className="fas fa-lightbulb" style={{ marginRight: 5 }} />
              {m}
            </div>
          ))}
          {data.errors?.[0]?.message && (
            <div style={S.errorRaw}>{data.errors[0].message}</div>
          )}
        </div>

        <button style={S.btnPrimary()} onClick={() => {
          if (data.endpoint) setEndpoint(data.endpoint);
          if (data.database) setDatabase(data.database);
          setErrors(null);
          setTestOk(false);
          setPhase("enter_connection");
        }}>
          <i className="fas fa-edit" style={{ marginRight: 8 }} />
          Edit Connection
        </button>
        <button style={S.btnSecondary} onClick={() => window.location.reload()}>
          <i className="fas fa-redo" style={{ marginRight: 8 }} />
          Retry
        </button>
      </>
    );
  }

  // ── Main render ──────────────────────────────────────────────────────────────

  return (
    <div style={S.overlay}>
      <div style={S.card}>
        <div style={S.logoRow}>
          <LoomXLogo size={48} animate="pulse" />
        </div>

        {/* ── Enter / not_configured ── */}
        {(phase === "not_configured" || phase === "enter_connection") && (
          <ConnectionForm
            cancelTo={
              phase === "enter_connection" && initialPhase !== "not_configured"
                ? initialPhase
                : undefined
            }
          />
        )}

        {/* ── connection_failed ── */}
        {phase === "connection_failed" && (
          <ErrorState
            errType="connection_failed"
            title="Cannot Connect"
            description="LoomX cannot reach the configured metadata database. Check your endpoint hostname and network, then retry or enter a different connection."
          />
        )}

        {/* ── access_denied ── */}
        {phase === "access_denied" && (
          <ErrorState
            errType="access_denied"
            title="Access Denied"
            description="LoomX reached the endpoint but your Azure AD account does not have permission to access this database. Grant access in the Fabric workspace, or connect a different database."
          />
        )}

        {/* ── db_not_found ── */}
        {phase === "db_not_found" && (
          <ErrorState
            errType="db_not_found"
            title="Database Not Found"
            description="The endpoint was reached but the database does not exist or is not accessible. Verify the exact database name (case-sensitive) in your Fabric workspace."
          />
        )}

        {/* ── schema_missing ── */}
        {phase === "schema_missing" && (
          <>
            <StepBar step={3} testOk phase="schema_missing" />
            <h2 style={S.heading}>Initialize Database</h2>
            <p style={S.sub}>
              LoomX connected successfully. The required tables have not been created yet —
              click below and LoomX will set them up in seconds.
            </p>

            <div style={S.infoBox}>
              <i className="fas fa-check-circle" style={{ marginRight: 6, color: "#4ade80" }} />
              <strong>Endpoint</strong>&nbsp; {data.endpoint ?? endpoint}<br />
              <i className="fas fa-database" style={{ marginRight: 6, marginTop: 4, marginLeft: 1 }} />
              <strong>Database</strong>&nbsp; {data.database ?? database}
            </div>

            {errors && <ErrorDisplay errors={errors} />}

            <button
              style={S.btnPrimary()}
              onClick={() => {
                const ep = data.endpoint ?? endpoint;
                const db = data.database ?? database;
                setEndpoint(ep);
                setDatabase(db);
                handleInitialize(ep, db);
              }}
            >
              <i className="fas fa-magic" style={{ marginRight: 8 }} />
              Initialize Database
            </button>
            <button style={S.btnSecondary} onClick={() => {
              if (data.endpoint) setEndpoint(data.endpoint);
              if (data.database) setDatabase(data.database);
              setErrors(null);
              setTestOk(true);
              setPhase("enter_connection");
            }}>
              <i className="fas fa-edit" style={{ marginRight: 8 }} />
              Use a Different Connection
            </button>
          </>
        )}

        {/* ── initializing ── */}
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

        {/* ── success ── */}
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
              <i className="fas fa-spinner fa-spin" style={{ marginRight: 6 }} />
              Reloading in a few seconds…
            </div>
          </>
        )}
      </div>
    </div>
  );
}
