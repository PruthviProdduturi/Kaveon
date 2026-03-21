"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import { useTheme } from "../../../contexts/ThemeContext";
import { LoomXLoading } from "../../../components/LoomXLoading";
import {
  SETUP_DB_ICONS,
  FabricIcon, AzureSqlIcon, PostgreSQLIcon, MySQLIcon,
} from "../../../components/DataSourceIcons";

type DbType = "fabric_sql" | "azure_sql" | "postgresql" | "mysql";

interface MetadataConfig {
  db_type: DbType;
  label: string;
  endpoint: string;
  host: string;
  port: string;
  database: string;
}

interface FormState {
  db_type: DbType;
  connection_string: string;
  endpoint: string;
  host: string;
  port: string;
  database: string;
}

const DB_TYPES: Array<{ key: DbType; label: string }> = [
  { key: "fabric_sql",  label: "Microsoft Fabric SQL" },
  { key: "azure_sql",   label: "Azure SQL Database" },
  { key: "postgresql",  label: "PostgreSQL" },
  { key: "mysql",       label: "MySQL / MariaDB" },
];

const DB_ICONS: Record<DbType, React.ReactNode> = {
  fabric_sql:  <FabricIcon />,
  azure_sql:   <AzureSqlIcon />,
  postgresql:  <PostgreSQLIcon />,
  mysql:       <MySQLIcon />,
};

function usesEndpoint(t: DbType) { return t === "azure_sql"; }
function usesFabricConnStr(t: DbType) { return t === "fabric_sql"; }
function usesHostPort(t: DbType) { return t === "postgresql" || t === "mysql"; }

export default function MetadataSettingsPage() {
  const router = useRouter();
  const { isAdmin, loading: roleLoading } = useRole();
  const { primaryColor } = useTheme();

  const [config, setConfig] = useState<MetadataConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [editing, setEditing] = useState(false);
  const [form, setForm] = useState<FormState>({
    db_type: "fabric_sql",
    connection_string: "",
    endpoint: "",
    host: "",
    port: "",
    database: "",
  });

  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveResult, setSaveResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [formErrors, setFormErrors] = useState<Record<string, string>>({});

  // Redirect non-admins
  useEffect(() => {
    if (!roleLoading && !isAdmin) router.replace("/");
  }, [roleLoading, isAdmin, router]);

  useEffect(() => {
    if (roleLoading || !isAdmin) return;
    msalFetch(`${API_BASE}/api/v1/admin/metadata`)
      .then(r => r.json())
      .then((d: MetadataConfig) => { setConfig(d); setLoading(false); })
      .catch(e => { setError(String(e)); setLoading(false); });
  }, [roleLoading, isAdmin]);

  const startEdit = () => {
    if (!config) return;
    setForm({
      db_type: config.db_type,
      connection_string: "",
      endpoint: config.endpoint,
      host: config.host,
      port: config.port,
      database: config.database,
    });
    setTestResult(null);
    setSaveResult(null);
    setFormErrors({});
    setEditing(true);
  };

  const cancelEdit = () => {
    setEditing(false);
    setTestResult(null);
    setSaveResult(null);
    setFormErrors({});
  };

  const validate = (): boolean => {
    const errs: Record<string, string> = {};
    const t = form.db_type;
    if (usesFabricConnStr(t) && !form.connection_string.trim()) {
      errs.connection_string = "Connection string is required.";
    }
    if (usesEndpoint(t) && !form.endpoint.trim()) {
      errs.endpoint = "Server endpoint is required.";
    }
    if (usesHostPort(t) && !form.host.trim()) {
      errs.host = "Host is required.";
    }
    if (!form.database.trim()) {
      errs.database = "Database name is required.";
    }
    setFormErrors(errs);
    return Object.keys(errs).length === 0;
  };

  const buildPayload = () => {
    const t = form.db_type;
    const base: Record<string, unknown> = { db_type: t, database: form.database };
    if (usesFabricConnStr(t)) {
      base.connection_string = form.connection_string;
    } else if (usesEndpoint(t)) {
      base.endpoint = form.endpoint;
    } else if (usesHostPort(t)) {
      base.host = form.host;
      base.port = form.port ? parseInt(form.port) : (t === "postgresql" ? 5432 : 3306);
    }
    return base;
  };

  const handleTest = async () => {
    if (!validate()) return;
    setTesting(true);
    setTestResult(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/admin/metadata/test`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(buildPayload()),
      });
      if (res.ok) {
        setTestResult({ ok: true, message: "Connection successful." });
      } else {
        const body = await res.json().catch(() => ({}));
        const msgs: string[] = body?.detail?.errors?.map((e: { message: string }) => e.message) ?? [];
        setTestResult({ ok: false, message: msgs[0] ?? body?.detail ?? "Connection test failed." });
      }
    } catch (e) {
      setTestResult({ ok: false, message: String(e) });
    } finally {
      setTesting(false);
    }
  };

  const handleSave = async () => {
    if (!validate()) return;
    setSaving(true);
    setSaveResult(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/admin/metadata/update`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(buildPayload()),
      });
      const body = await res.json().catch(() => ({}));
      if (res.ok) {
        setSaveResult({ ok: true, message: body?.message ?? "Metadata server updated. API is restarting…" });
        setEditing(false);
        // Reload config after a moment
        setTimeout(() => {
          setLoading(true);
          msalFetch(`${API_BASE}/api/v1/admin/metadata`)
            .then(r => r.json())
            .then((d: MetadataConfig) => { setConfig(d); setLoading(false); })
            .catch(() => setLoading(false));
        }, 2500);
      } else {
        const msgs: string[] = body?.detail?.errors?.map((e: { message: string }) => e.message) ?? [];
        setSaveResult({ ok: false, message: msgs[0] ?? body?.detail ?? "Update failed." });
      }
    } catch (e) {
      setSaveResult({ ok: false, message: String(e) });
    } finally {
      setSaving(false);
    }
  };

  const setField = (k: keyof FormState, v: string) => {
    setForm(f => ({ ...f, [k]: v }));
    setFormErrors(e => { const next = { ...e }; delete next[k]; return next; });
    setTestResult(null);
  };

  if (roleLoading || loading) return <LoomXLoading />;
  if (error) return (
    <div style={{ padding: "40px 24px", color: "#ef4444", fontSize: 14 }}>
      Failed to load metadata config: {error}
    </div>
  );
  if (!isAdmin) return null;

  const meta = SETUP_DB_ICONS[config?.db_type ?? "fabric_sql"];

  return (
    <div style={{ maxWidth: 680, margin: "0 auto", padding: "32px 24px" }}>
      {/* Header */}
      <div style={{ marginBottom: 28 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 6 }}>
          <i className="fas fa-database" style={{ fontSize: 18, color: primaryColor }} />
          <h1 style={{ fontSize: 22, fontWeight: 700, color: "#0f172a", margin: 0 }}>
            Metadata Server
          </h1>
        </div>
        <p style={{ fontSize: 13.5, color: "#64748b", margin: 0 }}>
          The database that LoomX uses to store its own metadata — datasets, charts, dashboards, users.
        </p>
      </div>

      {/* Save result banner */}
      {saveResult && (
        <div style={{
          background: saveResult.ok ? "#f0fdf4" : "#fef2f2",
          border: `1px solid ${saveResult.ok ? "#bbf7d0" : "#fecaca"}`,
          borderRadius: 10, padding: "12px 16px", marginBottom: 20,
          display: "flex", alignItems: "center", gap: 10,
          fontSize: 13.5, color: saveResult.ok ? "#166534" : "#991b1b",
        }}>
          <i className={`fas ${saveResult.ok ? "fa-check-circle" : "fa-exclamation-circle"}`} />
          {saveResult.message}
        </div>
      )}

      {/* Current config card */}
      {!editing && config && (
        <div style={{
          background: "#fff", border: "1px solid #e2e8f0", borderRadius: 14,
          padding: "24px 28px", marginBottom: 20,
          boxShadow: "0 1px 4px rgba(0,0,0,0.05)",
        }}>
          <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 16 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
              <div style={{
                width: 48, height: 48, borderRadius: 12, flexShrink: 0,
                background: meta?.bg ?? "#f0f9ff",
                border: `1px solid ${meta?.border ?? "#bae6fd"}`,
                display: "flex", alignItems: "center", justifyContent: "center",
              }}>
                {meta?.icon ?? <i className="fas fa-database" style={{ color: meta?.color, fontSize: 18 }} />}
              </div>
              <div>
                <div style={{ fontWeight: 700, fontSize: 16, color: "#0f172a", marginBottom: 2 }}>
                  {config.label}
                </div>
                <div style={{ fontSize: 12.5, color: "#64748b" }}>
                  {config.db_type === "fabric_sql" || config.db_type === "azure_sql"
                    ? config.endpoint
                    : config.host
                      ? `${config.host}${config.port ? `:${config.port}` : ""}`
                      : "—"
                  }
                </div>
              </div>
            </div>
            <button
              type="button"
              onClick={startEdit}
              style={{
                flexShrink: 0, background: "none", border: `1px solid #e2e8f0`,
                borderRadius: 8, padding: "7px 16px", fontSize: 13, fontWeight: 500,
                color: "#475569", cursor: "pointer", display: "flex", alignItems: "center", gap: 6,
              }}
            >
              <i className="fas fa-edit" style={{ fontSize: 12 }} /> Edit
            </button>
          </div>

          <div style={{ marginTop: 18, display: "grid", gridTemplateColumns: "1fr 1fr", gap: "10px 24px" }}>
            {[
              { label: "Type", value: config.label },
              { label: "Database", value: config.database || "—" },
              ...(config.endpoint ? [{ label: "Endpoint", value: config.endpoint }] : []),
              ...(config.host ? [{ label: "Host", value: config.host }] : []),
              ...(config.port ? [{ label: "Port", value: config.port }] : []),
            ].map(({ label, value }) => (
              <div key={label}>
                <div style={{ fontSize: 11, fontWeight: 700, color: "#94a3b8", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 2 }}>
                  {label}
                </div>
                <div style={{ fontSize: 13, color: "#334155", fontFamily: label === "Endpoint" || label === "Host" ? "monospace" : undefined, wordBreak: "break-all" }}>
                  {value}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Edit form */}
      {editing && (
        <div style={{
          background: "#fff", border: "1px solid #e2e8f0", borderRadius: 14,
          padding: "24px 28px", boxShadow: "0 1px 4px rgba(0,0,0,0.05)",
        }}>
          <h2 style={{ fontSize: 16, fontWeight: 700, color: "#0f172a", marginTop: 0, marginBottom: 20 }}>
            Reconfigure Metadata Server
          </h2>

          {/* DB type picker */}
          <label style={{ display: "block", fontSize: 11.5, fontWeight: 700, color: "#64748b", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 8 }}>
            Database Type
          </label>
          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, marginBottom: 20 }}>
            {DB_TYPES.map(({ key, label }) => {
              const active = form.db_type === key;
              const dbMeta = SETUP_DB_ICONS[key];
              return (
                <button
                  key={key}
                  type="button"
                  onClick={() => { setField("db_type", key); setField("connection_string", ""); setField("endpoint", ""); setField("host", ""); setField("port", ""); setField("database", ""); }}
                  style={{
                    background: active ? `${dbMeta?.color}12` : "#f8fafc",
                    border: `1.5px solid ${active ? dbMeta?.color ?? primaryColor : "#e2e8f0"}`,
                    borderRadius: 10, padding: "10px 14px", cursor: "pointer",
                    display: "flex", alignItems: "center", gap: 10, textAlign: "left",
                    transition: "all 0.12s",
                  }}
                >
                  <span style={{ flexShrink: 0, opacity: active ? 1 : 0.55 }}>{DB_ICONS[key]}</span>
                  <span style={{ fontSize: 12.5, fontWeight: active ? 700 : 500, color: active ? dbMeta?.color ?? primaryColor : "#64748b", lineHeight: 1.3 }}>
                    {label}
                  </span>
                </button>
              );
            })}
          </div>

          {/* Fabric connection string */}
          {usesFabricConnStr(form.db_type) && (
            <>
              <label style={{ display: "block", fontSize: 11.5, fontWeight: 700, color: "#64748b", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}>
                ODBC Connection String *
              </label>
              <textarea
                rows={3}
                value={form.connection_string}
                onChange={e => setField("connection_string", e.target.value)}
                placeholder="Driver={ODBC Driver 18 for SQL Server};Server=tcp:…;Database=…;Authentication=ActiveDirectoryInteractive"
                style={{
                  width: "100%", background: "#f8fafc",
                  border: `1px solid ${formErrors.connection_string ? "#ef4444" : "#e2e8f0"}`,
                  borderRadius: 8, padding: "10px 14px", fontSize: 12.5, color: "#0f172a",
                  fontFamily: "monospace", resize: "vertical", outline: "none",
                  boxSizing: "border-box", marginBottom: 4,
                }}
              />
              {formErrors.connection_string && <div style={{ fontSize: 11.5, color: "#ef4444", marginBottom: 10 }}>{formErrors.connection_string}</div>}
              <div style={{ fontSize: 11.5, color: "#94a3b8", marginBottom: 16 }}>
                Find this in your Fabric SQL Analytics Endpoint or Data Warehouse settings.
              </div>
              {/* Database extracted from conn string — still let user specify a fallback */}
              <label style={{ display: "block", fontSize: 11.5, fontWeight: 700, color: "#64748b", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}>
                Database Name *
              </label>
              <input
                type="text"
                value={form.database}
                onChange={e => setField("database", e.target.value)}
                placeholder="LoomX"
                style={{
                  width: "100%", background: "#f8fafc",
                  border: `1px solid ${formErrors.database ? "#ef4444" : "#e2e8f0"}`,
                  borderRadius: 8, padding: "10px 14px", fontSize: 13, color: "#0f172a",
                  outline: "none", boxSizing: "border-box", marginBottom: 4,
                }}
              />
              {formErrors.database && <div style={{ fontSize: 11.5, color: "#ef4444", marginBottom: 10 }}>{formErrors.database}</div>}
              <div style={{ fontSize: 11.5, color: "#94a3b8", marginBottom: 16 }}>
                The Fabric SQL database that stores LoomX metadata.
              </div>
            </>
          )}

          {/* Azure SQL endpoint + database */}
          {usesEndpoint(form.db_type) && (
            <>
              <label style={{ display: "block", fontSize: 11.5, fontWeight: 700, color: "#64748b", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}>
                Server Name *
              </label>
              <input
                type="text"
                value={form.endpoint}
                onChange={e => setField("endpoint", e.target.value)}
                placeholder="my-server.database.windows.net"
                style={{
                  width: "100%", background: "#f8fafc",
                  border: `1px solid ${formErrors.endpoint ? "#ef4444" : "#e2e8f0"}`,
                  borderRadius: 8, padding: "10px 14px", fontSize: 13, color: "#0f172a",
                  outline: "none", boxSizing: "border-box", marginBottom: 4,
                }}
              />
              {formErrors.endpoint && <div style={{ fontSize: 11.5, color: "#ef4444", marginBottom: 10 }}>{formErrors.endpoint}</div>}
              <div style={{ fontSize: 11.5, color: "#94a3b8", marginBottom: 16 }}>
                Found in Azure Portal → SQL Server → Server name.
              </div>
              <label style={{ display: "block", fontSize: 11.5, fontWeight: 700, color: "#64748b", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}>
                Database Name *
              </label>
              <input
                type="text"
                value={form.database}
                onChange={e => setField("database", e.target.value)}
                placeholder="loomx-metadata"
                style={{
                  width: "100%", background: "#f8fafc",
                  border: `1px solid ${formErrors.database ? "#ef4444" : "#e2e8f0"}`,
                  borderRadius: 8, padding: "10px 14px", fontSize: 13, color: "#0f172a",
                  outline: "none", boxSizing: "border-box", marginBottom: 4,
                }}
              />
              {formErrors.database && <div style={{ fontSize: 11.5, color: "#ef4444", marginBottom: 10 }}>{formErrors.database}</div>}
              <div style={{ fontSize: 11.5, color: "#94a3b8", marginBottom: 16 }}>
                The Azure SQL database that will store LoomX metadata.
              </div>
            </>
          )}

          {/* PostgreSQL / MySQL host + port + database */}
          {usesHostPort(form.db_type) && (
            <>
              <div style={{ display: "flex", gap: 10, marginBottom: 0 }}>
                <div style={{ flex: 1 }}>
                  <label style={{ display: "block", fontSize: 11.5, fontWeight: 700, color: "#64748b", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}>
                    Host *
                  </label>
                  <input
                    type="text"
                    value={form.host}
                    onChange={e => setField("host", e.target.value)}
                    placeholder="localhost or db.example.com"
                    style={{
                      width: "100%", background: "#f8fafc",
                      border: `1px solid ${formErrors.host ? "#ef4444" : "#e2e8f0"}`,
                      borderRadius: 8, padding: "10px 14px", fontSize: 13, color: "#0f172a",
                      outline: "none", boxSizing: "border-box",
                    }}
                  />
                  {formErrors.host && <div style={{ fontSize: 11.5, color: "#ef4444", marginTop: 4 }}>{formErrors.host}</div>}
                </div>
                <div style={{ width: 100 }}>
                  <label style={{ display: "block", fontSize: 11.5, fontWeight: 700, color: "#64748b", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}>
                    Port
                  </label>
                  <input
                    type="number"
                    value={form.port}
                    onChange={e => setField("port", e.target.value)}
                    placeholder={form.db_type === "postgresql" ? "5432" : "3306"}
                    style={{
                      width: "100%", background: "#f8fafc", border: "1px solid #e2e8f0",
                      borderRadius: 8, padding: "10px 14px", fontSize: 13, color: "#0f172a",
                      outline: "none", boxSizing: "border-box",
                    }}
                  />
                </div>
              </div>
              <div style={{ fontSize: 11.5, color: "#94a3b8", marginBottom: 16, marginTop: 6 }}>
                Hostname or IP address of your {form.db_type === "postgresql" ? "PostgreSQL" : "MySQL"} server.
              </div>
              <label style={{ display: "block", fontSize: 11.5, fontWeight: 700, color: "#64748b", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}>
                Database Name *
              </label>
              <input
                type="text"
                value={form.database}
                onChange={e => setField("database", e.target.value)}
                placeholder="loomx"
                style={{
                  width: "100%", background: "#f8fafc",
                  border: `1px solid ${formErrors.database ? "#ef4444" : "#e2e8f0"}`,
                  borderRadius: 8, padding: "10px 14px", fontSize: 13, color: "#0f172a",
                  outline: "none", boxSizing: "border-box", marginBottom: 4,
                }}
              />
              {formErrors.database && <div style={{ fontSize: 11.5, color: "#ef4444", marginBottom: 10 }}>{formErrors.database}</div>}
              <div style={{ fontSize: 11.5, color: "#94a3b8", marginBottom: 16 }}>
                The {form.db_type === "postgresql" ? "PostgreSQL" : "MySQL"} database name. It must already exist.
              </div>
            </>
          )}

          {/* Test result */}
          {testResult && (
            <div style={{
              background: testResult.ok ? "#f0fdf4" : "#fef2f2",
              border: `1px solid ${testResult.ok ? "#bbf7d0" : "#fecaca"}`,
              borderRadius: 8, padding: "10px 14px", marginBottom: 16,
              display: "flex", alignItems: "center", gap: 8,
              fontSize: 13, color: testResult.ok ? "#166534" : "#991b1b",
            }}>
              <i className={`fas ${testResult.ok ? "fa-check-circle" : "fa-exclamation-circle"}`} />
              {testResult.message}
            </div>
          )}

          {/* Warning */}
          <div style={{
            background: "#fffbeb", border: "1px solid #fde68a",
            borderRadius: 8, padding: "10px 14px", marginBottom: 20,
            display: "flex", gap: 8, fontSize: 12.5, color: "#92400e",
          }}>
            <i className="fas fa-triangle-exclamation" style={{ marginTop: 1, flexShrink: 0 }} />
            <span>Updating the metadata server will <strong>restart the LoomX API</strong>. Connected users will experience a brief interruption.</span>
          </div>

          {/* Buttons */}
          <div style={{ display: "flex", gap: 10, flexWrap: "wrap" }}>
            <button
              type="button"
              onClick={handleTest}
              disabled={testing || saving}
              style={{
                background: "none", border: "1.5px solid #6366f1", borderRadius: 8,
                padding: "9px 18px", fontSize: 13.5, fontWeight: 600, color: "#6366f1",
                cursor: testing || saving ? "not-allowed" : "pointer",
                display: "flex", alignItems: "center", gap: 6, opacity: testing || saving ? 0.6 : 1,
              }}
            >
              {testing
                ? <><i className="fas fa-spinner fa-spin" /> Testing…</>
                : <><i className="fas fa-plug" /> Test Connection</>
              }
            </button>
            <button
              type="button"
              onClick={handleSave}
              disabled={saving || testing || !testResult?.ok}
              style={{
                background: saving || testing || !testResult?.ok
                  ? "#e2e8f0"
                  : "linear-gradient(135deg, #3b82f6 0%, #6366f1 100%)",
                border: "none", borderRadius: 8,
                padding: "9px 18px", fontSize: 13.5, fontWeight: 600,
                color: saving || testing || !testResult?.ok ? "#94a3b8" : "#fff",
                cursor: saving || testing || !testResult?.ok ? "not-allowed" : "pointer",
                display: "flex", alignItems: "center", gap: 6,
              }}
            >
              {saving
                ? <><i className="fas fa-spinner fa-spin" /> Saving…</>
                : <><i className="fas fa-save" /> Save & Restart</>
              }
            </button>
            <button
              type="button"
              onClick={cancelEdit}
              disabled={saving || testing}
              style={{
                background: "none", border: "1px solid #e2e8f0", borderRadius: 8,
                padding: "9px 18px", fontSize: 13.5, fontWeight: 500, color: "#64748b",
                cursor: "pointer", marginLeft: "auto",
              }}
            >
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
