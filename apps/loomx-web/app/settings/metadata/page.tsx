"use client";

import React, { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import { useTheme } from "../../../contexts/ThemeContext";
import { Button } from "../../../components/Button";
import { ListPageShell } from "../../../components/ListPageShell";
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

const DB_TYPES: Array<{ key: DbType; label: string; icon: React.ReactNode }> = [
  { key: "fabric_sql",  label: "Microsoft Fabric SQL", icon: <FabricIcon /> },
  { key: "azure_sql",   label: "Azure SQL Database",   icon: <AzureSqlIcon /> },
  { key: "postgresql",  label: "PostgreSQL",            icon: <PostgreSQLIcon /> },
  { key: "mysql",       label: "MySQL / MariaDB",       icon: <MySQLIcon /> },
];

function usesFabricConnStr(t: DbType) { return t === "fabric_sql"; }
function usesEndpoint(t: DbType)      { return t === "azure_sql"; }
function usesHostPort(t: DbType)      { return t === "postgresql" || t === "mysql"; }

export default function MetadataSettingsPage() {
  const router = useRouter();
  const { isAdmin, loading: roleLoading } = useRole();
  const { primaryColor } = useTheme();

  const [config, setConfig] = useState<MetadataConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showModal, setShowModal] = useState(false);
  const [banner, setBanner] = useState<{ ok: boolean; message: string } | null>(null);

  useEffect(() => {
    if (!roleLoading && !isAdmin) router.replace("/");
  }, [roleLoading, isAdmin, router]);

  const loadConfig = () => {
    setLoading(true);
    msalFetch(`${API_BASE}/api/v1/admin/metadata`)
      .then(r => r.json())
      .then((d: MetadataConfig) => { setConfig(d); setLoading(false); })
      .catch(e => { setError(String(e)); setLoading(false); });
  };

  useEffect(() => {
    if (!roleLoading && isAdmin) loadConfig();
  }, [roleLoading, isAdmin]);

  if (!isAdmin && !roleLoading) return null;

  const meta = SETUP_DB_ICONS[config?.db_type ?? "fabric_sql"];
  const hostDisplay = config
    ? (config.db_type === "fabric_sql" || config.db_type === "azure_sql")
      ? config.endpoint
      : config.host
        ? `${config.host}${config.port ? `:${config.port}` : ""}`
        : "—"
    : "—";

  return (
    <ListPageShell
      icon="fa-database"
      title="Metadata Server"
      subtitle="The database LooMX uses to store its own metadata — datasets, charts, dashboards, and users."
      loading={loading || roleLoading}
      loadingMessage="Loading metadata server config"
      error={error}
      action={
        <Button onClick={() => { setBanner(null); setShowModal(true); }}>
          <i className="fas fa-edit" /> Edit Configuration
        </Button>
      }
    >
      {/* Banner after save */}
      {banner && (
        <div style={{
          display: "flex", alignItems: "center", gap: 10,
          padding: "0.75rem 1rem", borderRadius: 10, marginBottom: "1.25rem",
          background: banner.ok ? "#f0fdf4" : "#fef2f2",
          border: `1px solid ${banner.ok ? "#bbf7d0" : "#fecaca"}`,
          fontSize: 13.5, color: banner.ok ? "#166534" : "#991b1b",
        }}>
          <i className={`fas ${banner.ok ? "fa-check-circle" : "fa-exclamation-circle"}`} />
          {banner.message}
        </div>
      )}

      {/* Config card */}
      {config && (
        <div className="card" style={{ padding: "1.5rem" }}>
          {/* Icon + label row */}
          <div style={{ display: "flex", alignItems: "center", gap: 14, marginBottom: "1.5rem" }}>
            <div style={{
              width: 52, height: 52, borderRadius: 13, flexShrink: 0,
              background: meta?.bg ?? "#f0f9ff",
              border: `1px solid ${meta?.border ?? "#bae6fd"}`,
              display: "flex", alignItems: "center", justifyContent: "center",
            }}>
              {meta?.icon}
            </div>
            <div>
              <div style={{ fontSize: 16, fontWeight: 700, color: "#0f172a" }}>{config.label}</div>
              <div style={{ fontSize: 12.5, color: "#64748b", marginTop: 1 }}>{hostDisplay}</div>
            </div>
          </div>

          {/* Field grid */}
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(200px, 1fr))", gap: "1.25rem" }}>
            {[
              { label: "Type",     value: config.label,    mono: false },
              { label: "Database", value: config.database || "—", mono: true  },
              ...(config.endpoint ? [{ label: "Endpoint", value: config.endpoint, mono: true }] : []),
              ...(config.host     ? [{ label: "Host",     value: config.host,     mono: true }] : []),
              ...(config.port     ? [{ label: "Port",     value: config.port,     mono: true }] : []),
            ].map(({ label, value, mono }) => (
              <div key={label}>
                <div style={{ fontSize: 11, fontWeight: 700, color: "#94a3b8", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 4 }}>
                  {label}
                </div>
                <div style={{ fontSize: 13.5, color: "#334155", fontFamily: mono ? "monospace" : undefined, wordBreak: "break-all" }}>
                  {value}
                </div>
              </div>
            ))}
          </div>

          {/* Divider + info note */}
          <div style={{ marginTop: "1.5rem", paddingTop: "1.25rem", borderTop: "1px solid #f1f5f9", fontSize: 12.5, color: "#64748b", display: "flex", gap: 8 }}>
            <i className="fas fa-info-circle" style={{ color: primaryColor, marginTop: 1, flexShrink: 0 }} />
            Changing the metadata server will re-run the schema initialisation and restart the LooMX API. Existing data will be preserved if the database already contains the schema.
          </div>
        </div>
      )}

      {showModal && (
        <EditMetadataModal
          current={config}
          onClose={() => setShowModal(false)}
          onSuccess={(msg) => {
            setShowModal(false);
            setBanner({ ok: true, message: msg });
            setTimeout(() => loadConfig(), 2500);
          }}
        />
      )}
    </ListPageShell>
  );
}

// ── Edit modal ────────────────────────────────────────────────────────────────

interface EditModalProps {
  current: MetadataConfig | null;
  onClose: () => void;
  onSuccess: (msg: string) => void;
}

function EditMetadataModal({ current, onClose, onSuccess }: EditModalProps) {
  const { gradientColors } = useTheme();

  const [form, setForm] = useState<FormState>({
    db_type:           current?.db_type ?? "fabric_sql",
    connection_string: "",
    endpoint:          current?.endpoint ?? "",
    host:              current?.host ?? "",
    port:              current?.port ?? "",
    database:          current?.database ?? "",
  });
  const [errors, setErrors]         = useState<Record<string, string>>({});
  const [testing, setTesting]       = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [saving, setSaving]         = useState(false);
  const [saveError, setSaveError]   = useState<string | null>(null);

  const setField = (k: keyof FormState, v: string) => {
    setForm(f => ({ ...f, [k]: v }));
    setErrors(e => { const n = { ...e }; delete n[k]; return n; });
    setTestResult(null);
  };

  const switchType = (t: DbType) => {
    setForm({ db_type: t, connection_string: "", endpoint: "", host: "", port: "", database: "" });
    setErrors({});
    setTestResult(null);
  };

  const validate = () => {
    const e: Record<string, string> = {};
    if (usesFabricConnStr(form.db_type) && !form.connection_string.trim()) e.connection_string = "Connection string is required.";
    if (usesEndpoint(form.db_type)      && !form.endpoint.trim())          e.endpoint = "Server name is required.";
    if (usesHostPort(form.db_type)      && !form.host.trim())              e.host = "Host is required.";
    if (!form.database.trim())                                              e.database = "Database name is required.";
    setErrors(e);
    return Object.keys(e).length === 0;
  };

  const buildPayload = () => {
    const t = form.db_type;
    const p: Record<string, unknown> = { db_type: t, database: form.database };
    if (usesFabricConnStr(t)) p.connection_string = form.connection_string;
    else if (usesEndpoint(t)) p.endpoint = form.endpoint;
    else if (usesHostPort(t)) { p.host = form.host; p.port = form.port ? parseInt(form.port) : (t === "postgresql" ? 5432 : 3306); }
    return p;
  };

  const handleTest = async () => {
    if (!validate()) return;
    setTesting(true);
    setTestResult(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/admin/metadata/test`, {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify(buildPayload()),
      });
      if (res.ok) {
        setTestResult({ ok: true, message: "Connection successful." });
      } else {
        const body = await res.json().catch(() => ({}));
        const msg: string = body?.detail?.errors?.[0]?.message ?? body?.detail ?? "Connection test failed.";
        setTestResult({ ok: false, message: msg });
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
    setSaveError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/admin/metadata/update`, {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify(buildPayload()),
      });
      const body = await res.json().catch(() => ({}));
      if (res.ok) {
        onSuccess(body?.message ?? "Metadata server updated. LooMX API is restarting…");
      } else {
        const msg: string = body?.detail?.errors?.[0]?.message ?? body?.detail ?? "Update failed.";
        setSaveError(msg);
      }
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const busy = testing || saving;

  return (
    <>
      {/* Backdrop */}
      <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.45)", zIndex: 1000 }} onClick={onClose} />

      {/* Dialog */}
      <div style={{
        position: "fixed", top: "50%", left: "50%", transform: "translate(-50%,-50%)",
        background: "white", borderRadius: 14, boxShadow: "0 24px 64px rgba(0,0,0,0.22)",
        zIndex: 1001, width: "90%", maxWidth: 540, maxHeight: "90vh",
        display: "flex", flexDirection: "column", overflow: "hidden",
      }}>
        {/* Gradient accent bar */}
        <div style={{ height: 4, background: `linear-gradient(90deg, ${gradientColors.light}, ${gradientColors.dark})`, flexShrink: 0 }} />

        {/* Header */}
        <div style={{
          display: "flex", justifyContent: "space-between", alignItems: "center",
          padding: "1.25rem 1.5rem", borderBottom: "1px solid #f1f5f9", flexShrink: 0,
        }}>
          <div>
            <h2 style={{ margin: 0, fontSize: "1.1rem", fontWeight: 700, color: "#0f172a" }}>Edit Metadata Server</h2>
            <p style={{ margin: "2px 0 0", fontSize: 12, color: "#64748b" }}>Test the connection before saving.</p>
          </div>
          <button onClick={onClose} style={{ background: "none", border: "none", fontSize: "1.1rem", cursor: "pointer", color: "#94a3b8", width: 30, height: 30, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6 }}>
            <i className="fas fa-times" />
          </button>
        </div>

        {/* Body — scrollable */}
        <div style={{ overflowY: "auto", flex: 1, padding: "1.25rem 1.5rem", display: "flex", flexDirection: "column", gap: "1rem" }}>

          {/* Save error */}
          {saveError && (
            <div style={{ padding: "0.75rem 1rem", borderRadius: 8, background: "#fef2f2", color: "#991b1b", border: "1px solid #fecaca", fontSize: 13, display: "flex", gap: "0.6rem", alignItems: "center" }}>
              <i className="fas fa-exclamation-circle" />{saveError}
            </div>
          )}

          {/* DB type picker */}
          <div className="chart-builder-field">
            <label className="chart-builder-label"><span>Database Type</span></label>
            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, marginTop: 4 }}>
              {DB_TYPES.map(({ key, label, icon }) => {
                const active = form.db_type === key;
                const m = SETUP_DB_ICONS[key];
                return (
                  <button
                    key={key}
                    type="button"
                    onClick={() => switchType(key)}
                    style={{
                      display: "flex", alignItems: "center", gap: 10, padding: "0.65rem 0.85rem",
                      borderRadius: 8, cursor: "pointer", textAlign: "left",
                      border: `2px solid ${active ? m?.color ?? "#6366f1" : "#e2e8f0"}`,
                      background: active ? (m?.bg ?? "#f0f9ff") : "white",
                      transition: "all 0.15s",
                    }}
                  >
                    <span style={{ flexShrink: 0, opacity: active ? 1 : 0.45, transition: "opacity 0.15s" }}>{icon}</span>
                    <span style={{ fontSize: 12.5, fontWeight: active ? 700 : 500, color: active ? (m?.color ?? "#0f172a") : "#64748b", lineHeight: 1.3 }}>
                      {label}
                    </span>
                    {active && <i className="fas fa-check-circle" style={{ marginLeft: "auto", color: m?.color ?? "#6366f1", fontSize: 13 }} />}
                  </button>
                );
              })}
            </div>
          </div>

          {/* Fabric: ODBC connection string */}
          {usesFabricConnStr(form.db_type) && (
            <>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>ODBC Connection String *</span></label>
                <textarea
                  rows={3}
                  value={form.connection_string}
                  onChange={e => setField("connection_string", e.target.value)}
                  placeholder="Driver={ODBC Driver 18 for SQL Server};Server=tcp:…;Database=…;Authentication=ActiveDirectoryInteractive"
                  style={{
                    width: "100%", boxSizing: "border-box", resize: "vertical",
                    border: `1px solid ${errors.connection_string ? "#ef4444" : "#e2e8f0"}`,
                    borderRadius: 8, padding: "0.6rem 0.75rem", fontSize: 12.5,
                    fontFamily: "monospace", outline: "none", color: "#0f172a", background: "white",
                  }}
                />
                {errors.connection_string && <p style={{ margin: "2px 0 0", fontSize: 11.5, color: "#dc2626" }}>{errors.connection_string}</p>}
                <p style={{ margin: "4px 0 0", fontSize: 11.5, color: "#94a3b8" }}>
                  Find this in your Fabric workspace → SQL Analytics Endpoint or Data Warehouse → Settings.
                </p>
              </div>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Database Name *</span></label>
                <input className="chart-builder-input" type="text" value={form.database}
                  onChange={e => setField("database", e.target.value)} placeholder="LooMX"
                  style={{ borderColor: errors.database ? "#ef4444" : undefined }} />
                {errors.database && <p style={{ margin: "2px 0 0", fontSize: 11.5, color: "#dc2626" }}>{errors.database}</p>}
              </div>
            </>
          )}

          {/* Azure SQL: server endpoint + database */}
          {usesEndpoint(form.db_type) && (
            <>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Server Name *</span></label>
                <input className="chart-builder-input" type="text" value={form.endpoint}
                  onChange={e => setField("endpoint", e.target.value)} placeholder="my-server.database.windows.net"
                  style={{ borderColor: errors.endpoint ? "#ef4444" : undefined }} />
                {errors.endpoint && <p style={{ margin: "2px 0 0", fontSize: 11.5, color: "#dc2626" }}>{errors.endpoint}</p>}
                <p style={{ margin: "4px 0 0", fontSize: 11.5, color: "#94a3b8" }}>Found in Azure Portal → SQL Server → Server name.</p>
              </div>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Database Name *</span></label>
                <input className="chart-builder-input" type="text" value={form.database}
                  onChange={e => setField("database", e.target.value)} placeholder="loomx-metadata"
                  style={{ borderColor: errors.database ? "#ef4444" : undefined }} />
                {errors.database && <p style={{ margin: "2px 0 0", fontSize: 11.5, color: "#dc2626" }}>{errors.database}</p>}
              </div>
            </>
          )}

          {/* PostgreSQL / MySQL: host + port + database */}
          {usesHostPort(form.db_type) && (
            <>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Host *</span></label>
                <div style={{ display: "flex", gap: 8 }}>
                  <input className="chart-builder-input" type="text" value={form.host}
                    onChange={e => setField("host", e.target.value)} placeholder="localhost or db.example.com"
                    style={{ flex: 1, borderColor: errors.host ? "#ef4444" : undefined }} />
                  <input className="chart-builder-input" type="number" value={form.port}
                    onChange={e => setField("port", e.target.value)}
                    placeholder={form.db_type === "postgresql" ? "5432" : "3306"}
                    style={{ width: 90, borderColor: undefined }} />
                </div>
                {errors.host && <p style={{ margin: "2px 0 0", fontSize: 11.5, color: "#dc2626" }}>{errors.host}</p>}
                <p style={{ margin: "4px 0 0", fontSize: 11.5, color: "#94a3b8" }}>
                  Hostname or IP address of your {form.db_type === "postgresql" ? "PostgreSQL" : "MySQL"} server.
                </p>
              </div>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Database Name *</span></label>
                <input className="chart-builder-input" type="text" value={form.database}
                  onChange={e => setField("database", e.target.value)} placeholder="loomx"
                  style={{ borderColor: errors.database ? "#ef4444" : undefined }} />
                {errors.database && <p style={{ margin: "2px 0 0", fontSize: 11.5, color: "#dc2626" }}>{errors.database}</p>}
              </div>
            </>
          )}

          {/* Test result */}
          {testResult && (
            <div style={{
              padding: "0.75rem 1rem", borderRadius: 8, fontSize: 13,
              background: testResult.ok ? "#f0fdf4" : "#fef2f2",
              border: `1px solid ${testResult.ok ? "#bbf7d0" : "#fecaca"}`,
              color: testResult.ok ? "#166534" : "#991b1b",
              display: "flex", alignItems: "center", gap: 8,
            }}>
              <i className={`fas ${testResult.ok ? "fa-check-circle" : "fa-exclamation-circle"}`} />
              {testResult.message}
            </div>
          )}

          {/* Restart warning */}
          <div style={{
            padding: "0.75rem 1rem", borderRadius: 8, fontSize: 12.5,
            background: "#fffbeb", border: "1px solid #fde68a",
            color: "#92400e", display: "flex", gap: 8, alignItems: "flex-start",
          }}>
            <i className="fas fa-triangle-exclamation" style={{ marginTop: 1, flexShrink: 0 }} />
            <span>Saving will <strong>restart the LooMX API</strong>. Connected users will experience a brief interruption.</span>
          </div>
        </div>

        {/* Footer */}
        <div style={{
          display: "flex", justifyContent: "flex-end", alignItems: "center", gap: "0.75rem",
          padding: "1rem 1.5rem", borderTop: "1px solid #f1f5f9",
          background: "#f9fafb", flexShrink: 0,
        }}>
          <Button type="button" variant="secondary" onClick={handleTest} disabled={busy}>
            {testing
              ? <><i className="fas fa-spinner fa-spin" /> Testing…</>
              : <><i className="fas fa-plug" /> Test Connection</>
            }
          </Button>
          <Button type="button" onClick={handleSave} disabled={busy || !testResult?.ok}>
            {saving
              ? <><i className="fas fa-spinner fa-spin" /> Saving…</>
              : <><i className="fas fa-save" /> Save &amp; Restart</>
            }
          </Button>
          <Button type="button" variant="secondary" onClick={onClose} disabled={busy}>Cancel</Button>
        </div>
      </div>
    </>
  );
}
