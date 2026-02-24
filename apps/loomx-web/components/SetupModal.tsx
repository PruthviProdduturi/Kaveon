"use client";

/**
 * Setup Modal
 *
 * Shown after login when the metadata database is not configured, unreachable,
 * or has not been initialised.  Guides the user through providing a Fabric SQL
 * connection and running the LoomX schema automatically.
 *
 * Handles every scenario a first-time user can encounter:
 *   not_configured  – no .env vars; enter connection for the first time
 *   connection_failed – endpoint wrong or network issue
 *   access_denied   – connected but no permission → create new DB or get access
 *   db_not_found    – endpoint works but database name is wrong / doesn't exist
 *   schema_missing  – DB exists but LoomX tables are absent → one-click init
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

export interface SetupData {
  status: string;
  endpoint?: string;
  database?: string;
  message?: string;
}

interface SetupModalProps {
  data: SetupData;
  onComplete: () => void;
}

// ─── Styles ───────────────────────────────────────────────────────────────────

const overlay: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(15, 23, 42, 0.92)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  zIndex: 9999,
};

const card: React.CSSProperties = {
  background: "#1e293b",
  border: "1px solid #334155",
  borderRadius: 16,
  padding: "40px 44px",
  width: "100%",
  maxWidth: 480,
  boxShadow: "0 25px 60px rgba(0,0,0,0.5)",
};

const heading: React.CSSProperties = {
  fontSize: 22,
  fontWeight: 700,
  color: "#f1f5f9",
  margin: "20px 0 8px",
};

const subheading: React.CSSProperties = {
  fontSize: 14,
  color: "#94a3b8",
  lineHeight: 1.6,
  marginBottom: 28,
};

const label: React.CSSProperties = {
  display: "block",
  fontSize: 13,
  fontWeight: 600,
  color: "#cbd5e1",
  marginBottom: 6,
};

const inputStyle: React.CSSProperties = {
  width: "100%",
  background: "#0f172a",
  border: "1px solid #334155",
  borderRadius: 8,
  padding: "10px 14px",
  fontSize: 13,
  color: "#f1f5f9",
  outline: "none",
  boxSizing: "border-box",
  marginBottom: 16,
};

const btnPrimary: React.CSSProperties = {
  width: "100%",
  background: "linear-gradient(135deg, #3b82f6, #6366f1)",
  border: "none",
  borderRadius: 10,
  padding: "12px 20px",
  fontSize: 14,
  fontWeight: 600,
  color: "#fff",
  cursor: "pointer",
  marginBottom: 10,
};

const btnSecondary: React.CSSProperties = {
  width: "100%",
  background: "transparent",
  border: "1px solid #475569",
  borderRadius: 10,
  padding: "11px 20px",
  fontSize: 14,
  fontWeight: 500,
  color: "#94a3b8",
  cursor: "pointer",
  marginBottom: 10,
};

const errorBox: React.CSSProperties = {
  background: "rgba(220,38,38,0.12)",
  border: "1px solid rgba(220,38,38,0.35)",
  borderRadius: 8,
  padding: "10px 14px",
  fontSize: 12,
  color: "#fca5a5",
  marginBottom: 16,
  wordBreak: "break-all",
};

const successBox: React.CSSProperties = {
  background: "rgba(34,197,94,0.12)",
  border: "1px solid rgba(34,197,94,0.35)",
  borderRadius: 8,
  padding: "10px 14px",
  fontSize: 13,
  color: "#86efac",
  marginBottom: 16,
};

const infoBox: React.CSSProperties = {
  background: "rgba(59,130,246,0.1)",
  border: "1px solid rgba(59,130,246,0.3)",
  borderRadius: 8,
  padding: "10px 14px",
  fontSize: 12,
  color: "#93c5fd",
  marginBottom: 20,
  wordBreak: "break-all",
};

// ─── Component ────────────────────────────────────────────────────────────────

export function SetupModal({ data, onComplete }: SetupModalProps) {
  const initialPhase =
    data.status === "schema_missing" ? "schema_missing" : (data.status as SetupPhase);

  const [phase, setPhase] = useState<SetupPhase>(initialPhase);
  const [endpoint, setEndpoint] = useState(data.endpoint ?? "");
  const [database, setDatabase] = useState(data.database ?? "");
  const [probeError, setProbeError] = useState<string | null>(null);
  const [testOk, setTestOk] = useState(false);

  // When the parent passes new data after re-checking, sync phase.
  useEffect(() => {
    if (phase !== "testing" && phase !== "initializing" && phase !== "success") {
      setPhase(initialPhase);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data.status]);

  // ── Handlers ────────────────────────────────────────────────────────────────

  async function handleTest() {
    setPhase("testing");
    setProbeError(null);
    setTestOk(false);

    try {
      const res = await msalFetch(`${API_BASE}/api/v1/setup/test`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ endpoint: endpoint.trim(), database: database.trim() }),
      });
      const result = await res.json();

      if (result.success) {
        setTestOk(true);
        setPhase("enter_connection");
      } else {
        setProbeError(result.message || "Connection test failed");
        setPhase("enter_connection");
      }
    } catch (err) {
      setProbeError(err instanceof Error ? err.message : "Network error");
      setPhase("enter_connection");
    }
  }

  async function handleInitialize() {
    setPhase("initializing");
    setProbeError(null);

    try {
      const res = await msalFetch(`${API_BASE}/api/v1/setup/initialize`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ endpoint: endpoint.trim(), database: database.trim() }),
      });
      const result = await res.json();

      if (result.success) {
        setPhase("success");
        // API is restarting — reload after a brief pause.
        setTimeout(() => {
          window.location.reload();
        }, 3500);
      } else {
        setProbeError(result.message || "Initialisation failed");
        setPhase("enter_connection");
      }
    } catch (err) {
      setProbeError(err instanceof Error ? err.message : "Network error");
      setPhase("enter_connection");
    }
  }

  // ── Render helpers ───────────────────────────────────────────────────────────

  function renderConnectionForm(
    title: string,
    description: React.ReactNode,
    cancelAction?: () => void,
  ) {
    const canTest = endpoint.trim().length > 0 && database.trim().length > 0;
    const isWorking = phase === "testing";

    return (
      <>
        <h2 style={heading}>{title}</h2>
        <p style={subheading}>{description}</p>

        {probeError && !testOk && (
          <div style={errorBox}>
            <i className="fas fa-exclamation-circle" style={{ marginRight: 6 }} />
            {probeError}
          </div>
        )}

        {testOk && (
          <div style={successBox}>
            <i className="fas fa-check-circle" style={{ marginRight: 6 }} />
            Connection successful! Ready to initialise.
          </div>
        )}

        <label style={label} htmlFor="setup-endpoint">
          Fabric SQL Endpoint
        </label>
        <input
          id="setup-endpoint"
          style={inputStyle}
          type="text"
          placeholder="xxxxxxxx.datawarehouse.fabric.microsoft.com"
          value={endpoint}
          onChange={(e) => { setEndpoint(e.target.value); setTestOk(false); }}
          disabled={isWorking}
        />

        <label style={label} htmlFor="setup-database">
          Database Name
        </label>
        <input
          id="setup-database"
          style={inputStyle}
          type="text"
          placeholder="MyLoomXMetadataDB"
          value={database}
          onChange={(e) => { setDatabase(e.target.value); setTestOk(false); }}
          disabled={isWorking}
        />

        {testOk ? (
          <button style={btnPrimary} onClick={handleInitialize}>
            <i className="fas fa-rocket" style={{ marginRight: 8 }} />
            Initialize Database
          </button>
        ) : (
          <button style={{ ...btnPrimary, opacity: canTest ? 1 : 0.5 }} onClick={handleTest} disabled={!canTest || isWorking}>
            {isWorking ? (
              <>
                <i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />
                Testing…
              </>
            ) : (
              <>
                <i className="fas fa-plug" style={{ marginRight: 8 }} />
                Test Connection
              </>
            )}
          </button>
        )}

        {cancelAction && (
          <button style={btnSecondary} onClick={cancelAction} disabled={isWorking}>
            Cancel
          </button>
        )}
      </>
    );
  }

  // ── Main render ──────────────────────────────────────────────────────────────

  return (
    <div style={overlay}>
      <div style={card}>
        {/* Logo */}
        <div style={{ textAlign: "center" }}>
          <LoomXLogo size={52} animate="pulse" />
        </div>

        {/* ── Phase: not_configured ── */}
        {(phase === "not_configured" || phase === "enter_connection") &&
          renderConnectionForm(
            "Connect Metadata Database",
            "LoomX stores your datasets, charts, and dashboards in a Microsoft Fabric SQL database. Enter your endpoint and database name to get started.",
            phase === "enter_connection" && initialPhase !== "not_configured"
              ? () => setPhase(initialPhase)
              : undefined,
          )}

        {/* ── Phase: connection_failed ── */}
        {phase === "connection_failed" && (
          <>
            <h2 style={heading}>Cannot Connect</h2>
            <p style={subheading}>
              LoomX cannot reach the configured metadata database. Check your endpoint or network
              connection, then try again.
            </p>

            {data.endpoint && (
              <div style={infoBox}>
                <strong>Endpoint:</strong> {data.endpoint}
                <br />
                <strong>Database:</strong> {data.database}
              </div>
            )}

            {data.message && (
              <div style={errorBox}>
                <i className="fas fa-exclamation-circle" style={{ marginRight: 6 }} />
                {data.message}
              </div>
            )}

            <button style={btnPrimary} onClick={() => setPhase("enter_connection")}>
              <i className="fas fa-edit" style={{ marginRight: 8 }} />
              Use a Different Connection
            </button>
            <button
              style={btnSecondary}
              onClick={() => window.location.reload()}
            >
              <i className="fas fa-redo" style={{ marginRight: 8 }} />
              Retry
            </button>
          </>
        )}

        {/* ── Phase: access_denied ── */}
        {phase === "access_denied" && (
          <>
            <h2 style={heading}>Access Denied</h2>
            <p style={subheading}>
              LoomX connected to the endpoint but your account does not have permission to access
              the metadata database. You can connect to a different database you own, or contact
              your admin to get access.
            </p>

            {data.endpoint && (
              <div style={infoBox}>
                <strong>Endpoint:</strong> {data.endpoint}
                <br />
                <strong>Database:</strong> {data.database}
              </div>
            )}

            {data.message && (
              <div style={errorBox}>
                <i className="fas fa-lock" style={{ marginRight: 6 }} />
                {data.message}
              </div>
            )}

            <button style={btnPrimary} onClick={() => setPhase("enter_connection")}>
              <i className="fas fa-database" style={{ marginRight: 8 }} />
              Use a Different Database
            </button>
            <button
              style={btnSecondary}
              onClick={() => window.location.reload()}
            >
              <i className="fas fa-redo" style={{ marginRight: 8 }} />
              I Have Access Now — Retry
            </button>
          </>
        )}

        {/* ── Phase: db_not_found ── */}
        {phase === "db_not_found" && (
          <>
            <h2 style={heading}>Database Not Found</h2>
            <p style={subheading}>
              The endpoint was reached but the database does not exist or is not visible to your
              account. Create a new Fabric SQL database and come back with the new connection
              details.
            </p>

            {data.endpoint && (
              <div style={infoBox}>
                <strong>Endpoint:</strong> {data.endpoint}
                <br />
                <strong>Database:</strong> {data.database}
              </div>
            )}

            {data.message && (
              <div style={errorBox}>
                <i className="fas fa-exclamation-circle" style={{ marginRight: 6 }} />
                {data.message}
              </div>
            )}

            <button style={btnPrimary} onClick={() => setPhase("enter_connection")}>
              <i className="fas fa-edit" style={{ marginRight: 8 }} />
              Use a Different Connection
            </button>
            <button
              style={btnSecondary}
              onClick={() => window.location.reload()}
            >
              <i className="fas fa-redo" style={{ marginRight: 8 }} />
              Retry
            </button>
          </>
        )}

        {/* ── Phase: schema_missing ── */}
        {phase === "schema_missing" && (
          <>
            <h2 style={heading}>Initialize Database</h2>
            <p style={subheading}>
              LoomX connected successfully, but the required tables have not been created yet.
              Click below to set up the database — this only takes a few seconds.
            </p>

            <div style={infoBox}>
              <i className="fas fa-database" style={{ marginRight: 6 }} />
              <strong>Endpoint:</strong> {data.endpoint ?? endpoint}
              <br />
              <i className="fas fa-layer-group" style={{ marginRight: 6, marginTop: 4 }} />
              <strong>Database:</strong> {data.database ?? database}
            </div>

            <button
              style={btnPrimary}
              onClick={() => {
                // Use the confirmed endpoint/database from status response.
                if (data.endpoint) setEndpoint(data.endpoint);
                if (data.database) setDatabase(data.database);
                handleInitialize();
              }}
            >
              <i className="fas fa-magic" style={{ marginRight: 8 }} />
              Initialize Database
            </button>
            <button style={btnSecondary} onClick={() => setPhase("enter_connection")}>
              <i className="fas fa-edit" style={{ marginRight: 8 }} />
              Use a Different Connection
            </button>
          </>
        )}

        {/* ── Phase: initializing ── */}
        {phase === "initializing" && (
          <>
            <h2 style={heading}>Setting Up LoomX…</h2>
            <p style={subheading}>
              Creating tables in your metadata database. This should only take a moment.
            </p>
            <div style={{ textAlign: "center", padding: "24px 0", color: "#6366f1", fontSize: 36 }}>
              <i className="fas fa-spinner fa-spin" />
            </div>
          </>
        )}

        {/* ── Phase: success ── */}
        {phase === "success" && (
          <>
            <h2 style={{ ...heading, color: "#4ade80" }}>All Set!</h2>
            <p style={subheading}>
              Your metadata database has been initialised successfully. LoomX is restarting to
              apply the configuration — the page will reload automatically.
            </p>
            <div style={{ textAlign: "center", padding: "16px 0", color: "#4ade80", fontSize: 40 }}>
              <i className="fas fa-check-circle" />
            </div>
            <div style={{ textAlign: "center", color: "#64748b", fontSize: 13 }}>
              <i className="fas fa-spinner fa-spin" style={{ marginRight: 6 }} />
              Reloading in a few seconds…
            </div>
          </>
        )}
      </div>
    </div>
  );
}
