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
import { type AuthProvider } from "../../../auth/useAuth";

// ── Types ──────────────────────────────────────────────────────────────────────

type DbType = "fabric_sql" | "azure_sql" | "postgresql" | "mysql";

interface MetadataConfig {
  db_type: DbType;
  label: string;
  endpoint: string;
  host: string;
  port: string;
  database: string;
  ui_configured: boolean;
}

interface MetadataFormState {
  db_type: DbType;
  connection_string: string;
  endpoint: string;
  host: string;
  port: string;
  database: string;
}

interface AuthConfig {
  provider: AuthProvider;
  ui_configured?: boolean;
  azure_client_id?: string;
  azure_tenant_id?: string;
  google_client_id?: string;
}

// ── Static ─────────────────────────────────────────────────────────────────────

const DB_TYPES: Array<{ key: DbType; label: string; icon: React.ReactNode }> = [
  { key: "fabric_sql",  label: "Microsoft Fabric SQL", icon: <FabricIcon /> },
  { key: "azure_sql",   label: "Azure SQL Database",   icon: <AzureSqlIcon /> },
  { key: "postgresql",  label: "PostgreSQL",            icon: <PostgreSQLIcon /> },
  { key: "mysql",       label: "MySQL / MariaDB",       icon: <MySQLIcon /> },
];

const AUTH_PROVIDERS: Array<{
  key: AuthProvider; label: string; description: string;
  color: string; bg: string; border: string; beta?: boolean;
}> = [
  { key: "local",    label: "Local Login",        description: "Username & password stored in the Weft database", color: "#0f172a", bg: "#f8fafc", border: "#e2e8f0" },
  { key: "azure_ad", label: "Microsoft Azure AD", description: "Single sign-on via Microsoft Entra ID",            color: "#0078d4", bg: "#eff6ff", border: "#bfdbfe" },
  { key: "google",   label: "Google OAuth2",       description: "Sign in with Google accounts",                    color: "#ea4335", bg: "#fff1f0", border: "#fecaca", beta: true },
];

function usesFabricConnStr(t: DbType) { return t === "fabric_sql"; }
function usesEndpoint(t: DbType)      { return t === "azure_sql"; }
function usesHostPort(t: DbType)      { return t === "postgresql" || t === "mysql"; }

function maskId(val: string | undefined) {
  if (!val || val.length < 8) return val ?? "—";
  return val.slice(0, 6) + "••••••••" + val.slice(-4);
}

// ── Page ───────────────────────────────────────────────────────────────────────

export default function SystemSettingsPage() {
  const router = useRouter();
  const { isAdmin, loading: roleLoading } = useRole();
  const { primaryColor, gradientColors }  = useTheme();

  const [metaConfig,    setMetaConfig]    = useState<MetadataConfig | null>(null);
  const [metaLoading,   setMetaLoading]   = useState(true);
  const [showMetaModal, setShowMetaModal] = useState(false);
  const [metaBanner,    setMetaBanner]    = useState<{ ok: boolean; message: string } | null>(null);

  const [authConfig,    setAuthConfig]    = useState<AuthConfig | null>(null);
  const [authLoading,   setAuthLoading]   = useState(true);
  const [showAuthModal, setShowAuthModal] = useState(false);
  const [authBanner,    setAuthBanner]    = useState<{ ok: boolean; message: string } | null>(null);

  const [showReset, setShowReset] = useState(false);

  useEffect(() => {
    if (!roleLoading && !isAdmin) router.replace("/");
  }, [roleLoading, isAdmin, router]);

  const loadMetadata = () => {
    setMetaLoading(true);
    msalFetch(`${API_BASE}/api/v1/admin/metadata`)
      .then(r => r.ok ? r.json() : Promise.reject(r.status))
      .then((d: MetadataConfig) => { setMetaConfig(d); setMetaLoading(false); })
      .catch(() => { setMetaConfig(null); setMetaLoading(false); });
  };

  const loadAuth = () => {
    setAuthLoading(true);
    msalFetch(`${API_BASE}/api/v1/admin/auth`)
      .then(r => r.json())
      .then((d: AuthConfig) => { setAuthConfig(d); setAuthLoading(false); })
      .catch(() => { setAuthConfig({ provider: "local" }); setAuthLoading(false); });
  };

  useEffect(() => {
    if (!roleLoading && isAdmin) { loadMetadata(); loadAuth(); }
  }, [roleLoading, isAdmin]);

  if (!isAdmin && !roleLoading) return null;

  const authMeta     = AUTH_PROVIDERS.find(p => p.key === (authConfig?.provider ?? "local"))!;
  const metaIconMeta = SETUP_DB_ICONS[metaConfig?.db_type ?? "fabric_sql"];
  const metaConfigured = !!metaConfig?.ui_configured;
  const authConfigured = !!authConfig?.ui_configured;
  const fabricWithoutAzureAuth = metaConfigured && metaConfig!.db_type === "fabric_sql" && authConfig?.provider !== "azure_ad";
  const hostDisplay  = metaConfigured
    ? (metaConfig!.db_type === "fabric_sql" || metaConfig!.db_type === "azure_sql")
      ? metaConfig!.endpoint
      : metaConfig!.host ? `${metaConfig!.host}${metaConfig!.port ? `:${metaConfig!.port}` : ""}` : "—"
    : "—";

  return (
    <ListPageShell
      icon="fa-sliders"
      title="System"
      subtitle="Core infrastructure — authentication and metadata database."
      loading={roleLoading || metaLoading}
      action={metaConfigured || authConfigured ? (
        <button
          onClick={() => setShowReset(true)}
          style={{
            display: "flex", alignItems: "center", gap: 7,
            padding: "0.45rem 1rem", borderRadius: 8, cursor: "pointer",
            border: "1.5px solid #fecaca", background: "#fff5f5",
            color: "#dc2626", fontSize: 13, fontWeight: 600, flexShrink: 0,
          }}
        >
          <i className="fas fa-rotate-left" style={{ fontSize: 12 }} />
          Reset
        </button>
      ) : undefined}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: "1.5rem" }}>

        {/* ── Authentication ───────────────────────────────────────────────── */}
        {authBanner && <InlineBanner banner={authBanner} onDismiss={() => setAuthBanner(null)} />}

        <SectionCard
          accentColor={primaryColor}
          status={authConfigured ? "active" : "warning"}
          header={
            <CardHeader
              icon="fa-lock"
              title="Authentication"
              subtitle="Controls how users sign in to Weft"
              status={authConfigured ? "active" : "warning"}
              primaryColor={primaryColor}
              action={
                <Button onClick={() => { setAuthBanner(null); setShowAuthModal(true); }}>
                  <i className="fas fa-edit" /> Edit
                </Button>
              }
            />
          }
        >
          {authLoading ? (
            <div style={{ color: "#64748b", fontSize: 13, display: "flex", alignItems: "center", gap: 8 }}>
              <i className="fas fa-spinner fa-spin" /> Loading…
            </div>
          ) : (
            <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
              <div style={{
                width: 48, height: 48, borderRadius: 12, flexShrink: 0,
                background: authMeta.bg, border: `1.5px solid ${authMeta.border}`,
                display: "flex", alignItems: "center", justifyContent: "center",
                boxShadow: `0 1px 4px ${authMeta.border}60`,
              }}>
                {authConfig?.provider === "azure_ad" ? <MicrosoftIcon size={22} />
                  : authConfig?.provider === "google" ? <GoogleIcon size={20} />
                  : <i className="fas fa-lock" style={{ fontSize: 18, color: "#475569" }} />}
              </div>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 5 }}>
                  <span style={{ fontSize: 14, fontWeight: 700, color: "#0f172a" }}>{authMeta.label}</span>
                  {authMeta.beta && <BetaBadge />}
                </div>
                {authConfig?.provider === "azure_ad" && (
                  <div style={{ display: "flex", gap: "1.5rem", flexWrap: "wrap" }}>
                    <FieldPair label="Tenant ID" value={maskId(authConfig.azure_tenant_id)} />
                    <FieldPair label="Client ID" value={maskId(authConfig.azure_client_id)} />
                  </div>
                )}
                {authConfig?.provider === "google" && (
                  <FieldPair label="Client ID" value={maskId(authConfig.google_client_id)} />
                )}
                {authConfig?.provider === "local" && (
                  <span style={{ fontSize: 12.5, color: "#64748b" }}>Users sign in with a username and password managed in Weft.</span>
                )}
              </div>
            </div>
          )}
        </SectionCard>

        {/* ── Metadata Database ────────────────────────────────────────────── */}
        {metaBanner && <InlineBanner banner={metaBanner} onDismiss={() => setMetaBanner(null)} />}

        <SectionCard
          accentColor={primaryColor}
          status={metaConfigured ? "active" : "warning"}
          header={
            <CardHeader
              icon="fa-database"
              title="Metadata Database"
              subtitle="Stores dashboards, charts, datasets, and user data"
              status={metaConfigured ? "active" : "warning"}
              statusLabel={metaConfigured ? "Connected" : "Not configured"}
              primaryColor={primaryColor}
              action={
                <Button onClick={() => { setMetaBanner(null); setShowMetaModal(true); }}>
                  {metaConfigured ? <><i className="fas fa-edit" /> Edit</> : <><i className="fas fa-plug" /> Configure</>}
                </Button>
              }
            />
          }
        >
          {!metaConfigured ? (
            <div style={{
              display: "flex", alignItems: "center", gap: 14,
              padding: "0.75rem 1rem", borderRadius: 10,
              background: "#fffbeb", border: "1px solid #fde68a",
            }}>
              <div style={{ width: 40, height: 40, borderRadius: 10, flexShrink: 0, background: "#fef3c7", border: "1px solid #fcd34d", display: "flex", alignItems: "center", justifyContent: "center" }}>
                <i className="fas fa-triangle-exclamation" style={{ fontSize: 16, color: "#d97706" }} />
              </div>
              <div>
                <div style={{ fontSize: 13.5, fontWeight: 700, color: "#92400e", marginBottom: 2 }}>No database connected</div>
                <div style={{ fontSize: 12.5, color: "#b45309" }}>Weft cannot store any data until a metadata database is configured.</div>
              </div>
            </div>
          ) : (
            <>
              <div style={{ display: "flex", alignItems: "center", gap: 14, marginBottom: "1.25rem" }}>
                <div style={{
                  width: 48, height: 48, borderRadius: 12, flexShrink: 0,
                  background: metaIconMeta?.bg ?? "#f0f9ff",
                  border: `1.5px solid ${metaIconMeta?.border ?? "#bae6fd"}`,
                  display: "flex", alignItems: "center", justifyContent: "center",
                  boxShadow: `0 1px 4px ${metaIconMeta?.border ?? "#bae6fd"}80`,
                }}>
                  {metaIconMeta?.icon}
                </div>
                <div>
                  <div style={{ fontSize: 14, fontWeight: 700, color: "#0f172a", marginBottom: 3 }}>{metaConfig!.label}</div>
                  <div style={{ fontSize: 12.5, color: "#64748b", fontFamily: "monospace" }}>{hostDisplay || "—"}</div>
                </div>
              </div>

              <div style={{
                display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(190px, 1fr))", gap: "0.75rem",
                padding: "0.875rem 1rem", borderRadius: 10,
                background: "#f8fafc", border: "1px solid #f1f5f9",
              }}>
                <FieldPair label="Database" value={metaConfig!.database || "—"} />
                {metaConfig!.endpoint && <FieldPair label="Endpoint" value={metaConfig!.endpoint} />}
                {metaConfig!.host     && <FieldPair label="Host"     value={metaConfig!.host} />}
                {metaConfig!.port     && <FieldPair label="Port"     value={metaConfig!.port} />}
              </div>

              {fabricWithoutAzureAuth && (
                <div style={{
                  display: "flex", alignItems: "center", gap: 12,
                  marginTop: "1rem", padding: "0.75rem 1rem", borderRadius: 10,
                  background: "#fffbeb", border: "1px solid #fde68a",
                }}>
                  <i className="fas fa-triangle-exclamation" style={{ fontSize: 15, color: "#d97706", flexShrink: 0 }} />
                  <div style={{ fontSize: 12.5, color: "#92400e", flex: 1 }}>
                    <strong>Microsoft Fabric detected</strong> — Azure AD authentication is recommended.
                    Fabric SQL endpoints require Azure AD tokens for user queries.{" "}
                    <button
                      onClick={() => setShowAuthModal(true)}
                      style={{ background: "none", border: "none", padding: 0, cursor: "pointer", fontSize: 12.5, color: "#d97706", fontWeight: 600, textDecoration: "underline" }}
                    >
                      Configure Azure AD →
                    </button>
                  </div>
                </div>
              )}

              <div style={{ marginTop: "1rem", fontSize: 12, color: "#94a3b8", display: "flex", gap: 6, alignItems: "flex-start" }}>
                <i className="fas fa-info-circle" style={{ color: primaryColor, marginTop: 1, flexShrink: 0 }} />
                Changing the metadata server re-runs schema initialisation and restarts the API. Existing data is preserved.
              </div>
            </>
          )}
        </SectionCard>

        {/* ── Azure Key Vault — coming soon ────────────────────────────────── */}
        <div style={{
          borderRadius: 12, overflow: "hidden",
          border: "1.5px dashed #e2e8f0",
          background: "linear-gradient(135deg, #fafbfc 0%, #f8fafc 100%)",
          opacity: 0.9,
        }}>
          <div style={{
            padding: "0.875rem 1.25rem",
            background: `linear-gradient(135deg, ${primaryColor}06 0%, transparent 60%)`,
            borderBottom: "1px solid #f1f5f9",
            display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12,
          }}>
            <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
              <div style={{
                width: 36, height: 36, borderRadius: 9, flexShrink: 0,
                background: "#f0f9ff", border: "1px solid #bae6fd",
                display: "flex", alignItems: "center", justifyContent: "center",
              }}>
                <AzureKeyVaultIcon size={22} />
              </div>
              <div>
                <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 1 }}>
                  <strong style={{ fontSize: 14, color: "#64748b" }}>Azure Key Vault</strong>
                  <span style={{
                    fontSize: 10, fontWeight: 700, letterSpacing: "0.06em", textTransform: "uppercase",
                    padding: "2px 8px", borderRadius: 20,
                    background: `${primaryColor}12`, border: `1px solid ${primaryColor}28`,
                    color: primaryColor,
                  }}>
                    Coming soon
                  </span>
                </div>
                <div style={{ fontSize: 11.5, color: "#94a3b8" }}>Secure secret management for connection strings and credentials</div>
              </div>
            </div>
          </div>

          <div style={{ padding: "1.125rem 1.25rem" }}>
            <div style={{ display: "flex", flexWrap: "wrap", gap: "0.875rem" }}>
              {[
                { icon: "fa-key",           text: "Data source connection strings stored as Key Vault secret references — never as plain text in the database" },
                { icon: "fa-shield-halved", text: "Managed Identity access to Key Vault — no client secrets or passwords to manage" },
                { icon: "fa-lock",          text: "Centralised credential management across all data sources from a single vault" },
                { icon: "fa-clock-rotate-left", text: "Full audit trail of secret access via Azure Monitor and Key Vault diagnostics" },
              ].map(({ icon, text }) => (
                <div key={text} style={{
                  display: "flex", alignItems: "flex-start", gap: 8,
                  flex: "1 1 280px", minWidth: 0,
                  padding: "0.6rem 0.75rem", borderRadius: 8,
                  background: "#fff", border: "1px solid #f1f5f9",
                }}>
                  <i className={`fas ${icon}`} style={{ color: primaryColor, fontSize: 12, marginTop: 2, opacity: 0.65, flexShrink: 0 }} />
                  <span style={{ fontSize: 12.5, color: "#64748b", lineHeight: 1.45 }}>{text}</span>
                </div>
              ))}
            </div>
          </div>
        </div>

      </div>

      {showAuthModal && (
        <EditAuthModal
          current={authConfig}
          gradientColors={gradientColors}
          onClose={() => setShowAuthModal(false)}
          onSuccess={() => {
            setShowAuthModal(false);
            // Auth provider changed — clear session and redirect to login
            ["weft_local_token","weft_local_token_exp","weft_auth_provider","weft-auth-cache","weft-user-role"]
              .forEach(k => localStorage.removeItem(k));
            sessionStorage.clear();
            setTimeout(() => { window.location.href = "/"; }, 800);
          }}
        />
      )}
      {showMetaModal && (
        <EditMetadataModal
          current={metaConfig}
          gradientColors={gradientColors}
          onClose={() => setShowMetaModal(false)}
          onSuccess={() => {
            // Modal handles health polling + reload internally; just close on any other success path.
            setShowMetaModal(false);
          }}
        />
      )}
      {showReset && <ResetModal gradientColors={gradientColors} onClose={() => setShowReset(false)} />}
    </ListPageShell>
  );
}

// ── Tiny helpers ───────────────────────────────────────────────────────────────

// ── Section card with themed accent border ──────────────────────────────────

function SectionCard({
  accentColor, status, header, children,
}: {
  accentColor: string;
  status: "active" | "warning";
  header: React.ReactNode;
  children: React.ReactNode;
}) {
  const borderColor = status === "active" ? accentColor : "#f59e0b";
  return (
    <div
      className="card"
      style={{
        borderLeft: `3px solid ${borderColor}`,
        overflow: "hidden",
        boxShadow: `0 1px 4px rgba(0,0,0,0.06), 0 0 0 0.5px #e2e8f0`,
      }}
    >
      {header}
      <div style={{ padding: "1.25rem" }}>{children}</div>
    </div>
  );
}

function CardHeader({
  icon, title, subtitle, status, statusLabel, primaryColor, action,
}: {
  icon: string;
  title: string;
  subtitle: string;
  status: "active" | "warning";
  statusLabel?: string;
  primaryColor: string;
  action: React.ReactNode;
}) {
  return (
    <div style={{
      padding: "0.875rem 1.25rem",
      borderBottom: "1px solid #f1f5f9",
      background: `linear-gradient(135deg, ${primaryColor}08 0%, transparent 60%)`,
      display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12,
    }}>
      <div style={{ flex: 1, minWidth: 0, display: "flex", alignItems: "center", gap: 12 }}>
        {/* Icon badge */}
        <div style={{
          width: 36, height: 36, borderRadius: 9, flexShrink: 0,
          background: `${primaryColor}14`,
          border: `1px solid ${primaryColor}28`,
          display: "flex", alignItems: "center", justifyContent: "center",
        }}>
          <i className={`fas ${icon}`} style={{ color: primaryColor, fontSize: 14 }} />
        </div>
        {/* Title + subtitle */}
        <div>
          <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 1 }}>
            <strong style={{ fontSize: 14, color: "#0f172a" }}>{title}</strong>
            <StatusPill active={status === "active"} label={statusLabel ?? (status === "active" ? "Active" : "Not configured")} primaryColor={primaryColor} />
          </div>
          <div style={{ fontSize: 11.5, color: "#94a3b8" }}>{subtitle}</div>
        </div>
      </div>
      {action}
    </div>
  );
}

function StatusPill({ active, label, primaryColor }: { active: boolean; label: string; primaryColor?: string }) {
  const dotColor  = active ? (primaryColor ?? "#22c55e") : "#f59e0b";
  const bgColor   = active ? `${primaryColor ?? "#22c55e"}14` : "#fffbeb";
  const bdrColor  = active ? `${primaryColor ?? "#22c55e"}38` : "#fde68a";
  const txtColor  = active ? (primaryColor ?? "#15803d") : "#b45309";
  return (
    <span style={{
      display: "inline-flex", alignItems: "center", gap: 5,
      fontSize: 11, fontWeight: 600, letterSpacing: "0.02em",
      padding: "2px 9px", borderRadius: 20,
      background: bgColor, border: `1px solid ${bdrColor}`,
      color: txtColor, whiteSpace: "nowrap",
    }}>
      <span style={{
        width: 6, height: 6, borderRadius: "50%", flexShrink: 0,
        background: dotColor,
      }} />
      {label}
    </span>
  );
}

function FieldPair({ label, value }: { label: string; value: string | undefined }) {
  return (
    <div>
      <div style={{ fontSize: 11, fontWeight: 700, color: "#94a3b8", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 3 }}>{label}</div>
      <div style={{ fontSize: 13, color: "#334155", fontFamily: "monospace", wordBreak: "break-all" }}>{value ?? "—"}</div>
    </div>
  );
}

function InlineBanner({ banner, onDismiss }: { banner: { ok: boolean; message: string }; onDismiss: () => void }) {
  return (
    <div style={{
      display: "flex", alignItems: "center", gap: 10, padding: "0.65rem 1rem",
      borderRadius: 8, background: banner.ok ? "#f0fdf4" : "#fef2f2",
      border: `1px solid ${banner.ok ? "#bbf7d0" : "#fecaca"}`,
      fontSize: 13.5, color: banner.ok ? "#166534" : "#991b1b",
    }}>
      <i className={`fas ${banner.ok ? "fa-check-circle" : "fa-exclamation-circle"}`} />
      <span style={{ flex: 1 }}>{banner.message}</span>
      <button onClick={onDismiss} style={{ background: "none", border: "none", cursor: "pointer", color: "inherit", opacity: 0.5, padding: 0 }}>
        <i className="fas fa-times" style={{ fontSize: 12 }} />
      </button>
    </div>
  );
}

// Azure Key Vault official icon (approximated from Azure icon set)
function AzureKeyVaultIcon({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 18 18" fill="none" xmlns="http://www.w3.org/2000/svg">
      {/* Shield / hex background */}
      <path
        d="M9 1L2 4v5c0 3.55 2.96 6.87 7 7.93C13.04 15.87 16 12.55 16 9V4L9 1z"
        fill="#50e6ff"
        opacity="0.25"
      />
      <path
        d="M9 1L2 4v5c0 3.55 2.96 6.87 7 7.93C13.04 15.87 16 12.55 16 9V4L9 1z"
        stroke="#0078d4"
        strokeWidth="1"
        fill="none"
      />
      {/* Key head (circle) */}
      <circle cx="8" cy="8" r="2.2" stroke="#0078d4" strokeWidth="1.1" fill="none" />
      {/* Key shaft */}
      <line x1="9.9" y1="8" x2="13.5" y2="8" stroke="#0078d4" strokeWidth="1.1" strokeLinecap="round" />
      {/* Key teeth */}
      <line x1="12.5" y1="8" x2="12.5" y2="9.4" stroke="#0078d4" strokeWidth="1" strokeLinecap="round" />
      <line x1="11" y1="8" x2="11" y2="9.1" stroke="#0078d4" strokeWidth="1" strokeLinecap="round" />
    </svg>
  );
}

function BetaBadge() {
  return (
    <span style={{ fontSize: 10, fontWeight: 700, letterSpacing: "0.04em", padding: "1px 7px", borderRadius: 20, textTransform: "uppercase", background: "#fef9c3", border: "1px solid #fde047", color: "#713f12" }}>
      Beta
    </span>
  );
}

function MicrosoftIcon({ size = 24 }: { size?: number }) {
  const s = size / 2 - 1;
  return (
    <svg width={size} height={size} viewBox="0 0 21 21" fill="none">
      <rect x="1"   y="1"   width={s} height={s} fill="#f25022" rx="1" />
      <rect x={s+2} y="1"   width={s} height={s} fill="#7fba00" rx="1" />
      <rect x="1"   y={s+2} width={s} height={s} fill="#00a4ef" rx="1" />
      <rect x={s+2} y={s+2} width={s} height={s} fill="#ffb900" rx="1" />
    </svg>
  );
}

function GoogleIcon({ size = 22 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24">
      <path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z" fill="#4285F4"/>
      <path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853"/>
      <path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z" fill="#FBBC05"/>
      <path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335"/>
    </svg>
  );
}

// ── Reset Modal ────────────────────────────────────────────────────────────────

function ResetModal({ gradientColors, onClose }: { gradientColors: { light: string; dark: string }; onClose: () => void }) {
  const [busy,  setBusy]  = useState(false);
  const [done,  setDone]  = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleConfirm = async () => {
    setBusy(true); setError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/admin/reset`, { method: "POST" });
      if (res.ok) {
        ["weft_local_token","weft_local_token_exp","weft_auth_provider","weft-auth-cache","weft-user-role"]
          .forEach(k => localStorage.removeItem(k));
        sessionStorage.removeItem("weft_setup_ok");
        setDone(true);
      } else {
        const body = await res.json().catch(() => ({}));
        setError((body as any)?.detail?.message ?? (body as any)?.detail ?? (body as any)?.error?.message ?? "Reset failed. Please try again.");
      }
    } catch (e) { setError(String(e)); }
    setBusy(false);
  };

  return (
    <>
      <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.45)", zIndex: 1000 }} onClick={done ? undefined : onClose} />
      <div style={{ position: "fixed", top: "50%", left: "50%", transform: "translate(-50%,-50%)", background: "white", borderRadius: 14, boxShadow: "0 24px 64px rgba(0,0,0,0.22)", zIndex: 1001, width: "90%", maxWidth: 460, display: "flex", flexDirection: "column", overflow: "hidden" }}>
        <div style={{ height: 4, background: done ? "linear-gradient(90deg,#10b981,#059669)" : "linear-gradient(90deg,#ef4444,#dc2626)", flexShrink: 0 }} />
        <div style={{ padding: "1.5rem" }}>
          {done ? (
            <>
              <div style={{ display: "flex", alignItems: "flex-start", gap: 14, marginBottom: "1.25rem" }}>
                <div style={{ width: 44, height: 44, borderRadius: 10, flexShrink: 0, background: "#f0fdf4", border: "1px solid #bbf7d0", display: "flex", alignItems: "center", justifyContent: "center" }}>
                  <i className="fas fa-check" style={{ color: "#16a34a", fontSize: 18 }} />
                </div>
                <div>
                  <h2 style={{ margin: 0, fontSize: "1.05rem", fontWeight: 700, color: "#0f172a" }}>Reset complete</h2>
                  <p style={{ margin: "4px 0 0", fontSize: 13, color: "#64748b", lineHeight: 1.5 }}>
                    Configuration has been cleared. Restart the API, then open Weft in a new tab to run the setup wizard.
                  </p>
                </div>
              </div>
              <div style={{ padding: "0.75rem 1rem", borderRadius: 8, fontSize: 12.5, background: "#f0fdf4", border: "1px solid #bbf7d0", color: "#166534", display: "flex", gap: 8, marginBottom: "1.25rem" }}>
                <i className="fas fa-circle-info" style={{ marginTop: 1, flexShrink: 0 }} />
                Your data is <strong style={{ margin: "0 3px" }}>not deleted</strong>. Reconnect to the same database in the setup wizard.
              </div>
              <div style={{ display: "flex", justifyContent: "flex-end" }}>
                <button
                  onClick={() => { window.open("/", "_blank"); }}
                  style={{ padding: "0.5rem 1.25rem", borderRadius: 8, border: "none", background: "#16a34a", color: "white", fontSize: 13.5, fontWeight: 600, cursor: "pointer", display: "flex", alignItems: "center", gap: 7 }}
                >
                  <i className="fas fa-arrow-up-right-from-square" /> Open in new tab
                </button>
              </div>
            </>
          ) : (
            <>
              <div style={{ display: "flex", alignItems: "flex-start", gap: 14, marginBottom: "1rem" }}>
                <div style={{ width: 44, height: 44, borderRadius: 10, flexShrink: 0, background: "#fef2f2", border: "1px solid #fecaca", display: "flex", alignItems: "center", justifyContent: "center" }}>
                  <i className="fas fa-triangle-exclamation" style={{ color: "#dc2626", fontSize: 18 }} />
                </div>
                <div>
                  <h2 style={{ margin: 0, fontSize: "1.05rem", fontWeight: 700, color: "#0f172a" }}>Reset System?</h2>
                  <p style={{ margin: "4px 0 0", fontSize: 13, color: "#64748b", lineHeight: 1.5 }}>
                    This will clear the metadata database configuration and authentication settings. Weft will restart and show the setup wizard.
                  </p>
                </div>
              </div>
              <div style={{ padding: "0.75rem 1rem", borderRadius: 8, fontSize: 12.5, background: "#fffbeb", border: "1px solid #fde68a", color: "#92400e", display: "flex", gap: 8, marginBottom: "1rem" }}>
                <i className="fas fa-circle-info" style={{ marginTop: 1, flexShrink: 0 }} />
                Your data is <strong style={{ margin: "0 3px" }}>not deleted</strong>. You can reconnect to the same database in the setup wizard.
              </div>
              {error && (
                <div style={{ padding: "0.75rem 1rem", borderRadius: 8, fontSize: 13, background: "#fef2f2", border: "1px solid #fecaca", color: "#991b1b", display: "flex", gap: 8, marginBottom: "1rem" }}>
                  <i className="fas fa-exclamation-circle" />{error}
                </div>
              )}
              <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
                <Button variant="secondary" onClick={onClose} disabled={busy}>Cancel</Button>
                <button onClick={handleConfirm} disabled={busy} style={{ padding: "0.5rem 1.25rem", borderRadius: 8, cursor: busy ? "not-allowed" : "pointer", border: "none", background: busy ? "#fca5a5" : "#dc2626", color: "white", fontSize: 13.5, fontWeight: 600, display: "flex", alignItems: "center", gap: 7, opacity: busy ? 0.7 : 1 }}>
                  {busy ? <><i className="fas fa-spinner fa-spin" /> Resetting…</> : <><i className="fas fa-rotate-left" /> Yes, Reset</>}
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </>
  );
}

// ── Edit Auth Modal ────────────────────────────────────────────────────────────

function EditAuthModal({ current, gradientColors, onClose, onSuccess }: {
  current: AuthConfig | null;
  gradientColors: { light: string; dark: string };
  onClose: () => void;
  onSuccess: (msg: string) => void;
}) {
  const [provider,        setProvider]        = useState<AuthProvider>(current?.provider ?? "local");
  const [azureTenantId,   setAzureTenantId]   = useState(current?.azure_tenant_id  ?? "");
  const [azureClientId,   setAzureClientId]   = useState(current?.azure_client_id  ?? "");
  const [googleClientId,  setGoogleClientId]  = useState(current?.google_client_id ?? "");
  const [googleSecret,    setGoogleSecret]    = useState("");
  const [errors,          setErrors]          = useState<Record<string, string>>({});
  const [saving,          setSaving]          = useState(false);
  const [saveError,       setSaveError]       = useState<string | null>(null);
  const [manifestCopied,  setManifestCopied]  = useState(false);
  const [settingUpRoles,  setSettingUpRoles]  = useState(false);
  const [rolesStatus,     setRolesStatus]     = useState<{ ok: boolean; message: string; showManifest?: boolean } | null>(null);

  const switchProvider = (p: AuthProvider) => { setProvider(p); setErrors({}); setSaveError(null); setRolesStatus(null); };

  const APP_ROLES_MANIFEST = JSON.stringify([
    { id: "7f6e4b2a-1c3d-4e5f-8a9b-0d1e2f3c4b5a", displayName: "Viewer",  value: "Viewer",  description: "Read-only access to published content",               allowedMemberTypes: ["User"], isEnabled: true },
    { id: "2b3c4d5e-6f7a-8b9c-0d1e-2f3a4b5c6d7e", displayName: "Analyst", value: "Analyst", description: "Create and edit own content, SQL Lab access",         allowedMemberTypes: ["User"], isEnabled: true },
    { id: "9a8b7c6d-5e4f-3a2b-1c0d-ef9a8b7c6d5e", displayName: "Editor",  value: "Editor",  description: "Create, edit and publish any content",                 allowedMemberTypes: ["User"], isEnabled: true },
    { id: "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d", displayName: "Admin",   value: "Admin",   description: "Full access including user and data source management", allowedMemberTypes: ["User"], isEnabled: true },
  ], null, 2);

  const copyManifest = () => {
    navigator.clipboard.writeText(APP_ROLES_MANIFEST).then(() => {
      setManifestCopied(true);
      setTimeout(() => setManifestCopied(false), 2500);
    });
  };

  const handleSetupRoles = async () => {
    setSettingUpRoles(true);
    setRolesStatus(null);
    try {
      // Initialize MSAL with the Azure AD credentials from the form.
      // This is needed when the current session uses local auth (MSAL not yet configured).
      const { configureMsal, getMsalInstance, ensureMsalInitialized } = await import("../../../auth/msalConfig");
      configureMsal(azureClientId.trim(), azureTenantId.trim());
      await ensureMsalInitialized();
      const msal = getMsalInstance();

      // Acquire a delegated Graph token via MSAL popup — no client secret needed.
      // Works if the logged-in user is an Owner of the App Registration.
      let graphToken: string;
      try {
        const tok = await msal.acquireTokenPopup({
          scopes: ["https://graph.microsoft.com/Application.ReadWrite.All"],
          redirectUri: `${window.location.origin}/auth-popup.html`,
        });
        graphToken = tok.accessToken;
      } catch {
        setRolesStatus({ ok: false, message: "Could not acquire a Microsoft Graph token. Make sure you are signed in with an account that has Owner access to this App Registration, then try again.", showManifest: true });
        return;
      }

      const res = await msalFetch(`${API_BASE}/api/v1/admin/auth/setup-app-roles`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ graph_token: graphToken, client_id: azureClientId.trim(), tenant_id: azureTenantId.trim() }),
      });
      const b = await res.json().catch(() => ({}));
      if (res.ok) {
        setRolesStatus({ ok: true, message: (b as any).message ?? "App Roles created successfully." });
      } else {
        const detail = (b as any)?.detail;
        const msg = typeof detail === "object" ? detail?.message : detail;
        setRolesStatus({ ok: false, message: msg ?? "Failed to set up App Roles.", showManifest: res.status === 403 });
      }
    } catch (e) {
      setRolesStatus({ ok: false, message: String(e), showManifest: true });
    } finally {
      setSettingUpRoles(false);
    }
  };

  const validate = () => {
    const e: Record<string, string> = {};
    if (provider === "azure_ad") {
      if (!azureTenantId.trim()) e.azureTenantId = "Tenant ID is required.";
      if (!azureClientId.trim()) e.azureClientId  = "Client ID is required.";
    }
    if (provider === "google" && !googleClientId.trim()) e.googleClientId = "Client ID is required.";
    setErrors(e);
    return Object.keys(e).length === 0;
  };

  const handleSave = async () => {
    if (!validate()) return;
    setSaving(true); setSaveError(null);
    try {
      const payload: Record<string, unknown> = { provider };
      if (provider === "azure_ad") { payload.azure_tenant_id = azureTenantId.trim(); payload.azure_client_id = azureClientId.trim(); }
      if (provider === "google")   { payload.google_client_id = googleClientId.trim(); if (googleSecret) payload.google_client_secret = googleSecret; }
      const res = await msalFetch(`${API_BASE}/api/v1/admin/auth`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(payload) });
      const b = await res.json().catch(() => ({}));
      if (res.ok) { onSuccess("Authentication configuration updated."); }
      else {
        const detail = (b as any)?.detail;
        const msg = typeof detail === "object" ? detail?.message : detail;
        setSaveError(msg ?? "Failed to save.");
      }
    } catch (e) { setSaveError(String(e)); }
    finally { setSaving(false); }
  };

  return (
    <>
      <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.45)", zIndex: 1000 }} onClick={onClose} />
      <div style={{ position: "fixed", top: "50%", left: "50%", transform: "translate(-50%,-50%)", background: "white", borderRadius: 14, boxShadow: "0 24px 64px rgba(0,0,0,0.22)", zIndex: 1001, width: "90%", maxWidth: 520, maxHeight: "90vh", display: "flex", flexDirection: "column", overflow: "hidden" }}>
        <div style={{ height: 4, background: `linear-gradient(90deg,${gradientColors.light},${gradientColors.dark})`, flexShrink: 0 }} />
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "1.25rem 1.5rem", borderBottom: "1px solid #f1f5f9", flexShrink: 0 }}>
          <div>
            <h2 style={{ margin: 0, fontSize: "1.1rem", fontWeight: 700, color: "#0f172a" }}>Edit Authentication</h2>
            <p style={{ margin: "2px 0 0", fontSize: 12, color: "#64748b" }}>Choose how users sign in to Weft.</p>
          </div>
          <button onClick={onClose} style={{ background: "none", border: "none", fontSize: "1.1rem", cursor: "pointer", color: "#94a3b8", width: 30, height: 30, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6 }}>
            <i className="fas fa-times" />
          </button>
        </div>
        <div style={{ overflowY: "auto", flex: 1, padding: "1.25rem 1.5rem", display: "flex", flexDirection: "column", gap: "1.25rem" }}>
          {saveError && (
            <div style={{ padding: "0.75rem 1rem", borderRadius: 8, background: "#fef2f2", color: "#991b1b", border: "1px solid #fecaca", fontSize: 13, display: "flex", gap: "0.6rem", alignItems: "center" }}>
              <i className="fas fa-exclamation-circle" />{saveError}
            </div>
          )}
          <div className="chart-builder-field">
            <label className="chart-builder-label"><span>Sign-in Method</span></label>
            <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 4 }}>
              {AUTH_PROVIDERS.map(({ key, label, description, color, bg, border, beta }) => {
                const active = provider === key;
                return (
                  <button key={key} type="button" onClick={() => switchProvider(key)} style={{ display: "flex", alignItems: "center", gap: 12, padding: "0.75rem 1rem", borderRadius: 10, cursor: "pointer", textAlign: "left", transition: "all 0.15s", border: `2px solid ${active ? color : "#e2e8f0"}`, background: active ? bg : "white" }}>
                    <div style={{ width: 36, height: 36, borderRadius: 9, flexShrink: 0, background: active ? bg : "#f8fafc", border: `1px solid ${active ? border : "#e2e8f0"}`, display: "flex", alignItems: "center", justifyContent: "center" }}>
                      {key === "azure_ad" ? <MicrosoftIcon size={20} /> : key === "google" ? <GoogleIcon size={18} /> : <i className="fas fa-lock" style={{ fontSize: 15, color: active ? "#475569" : "#94a3b8" }} />}
                    </div>
                    <div style={{ flex: 1 }}>
                      <div style={{ display: "flex", alignItems: "center", gap: 7 }}>
                        <span style={{ fontSize: 13.5, fontWeight: active ? 700 : 500, color: active ? color : "#374151" }}>{label}</span>
                        {beta && <BetaBadge />}
                      </div>
                      <div style={{ fontSize: 11.5, color: "#94a3b8", marginTop: 1 }}>{description}</div>
                    </div>
                    {active && <i className="fas fa-check-circle" style={{ color, fontSize: 16, flexShrink: 0 }} />}
                  </button>
                );
              })}
            </div>
          </div>

          {provider === "azure_ad" && (
            <div style={{ display: "flex", flexDirection: "column", gap: "1rem", padding: "1rem", borderRadius: 10, background: "#f8fafc", border: "1px solid #e2e8f0" }}>
              <div style={{ fontSize: 12, fontWeight: 600, color: "#475569", display: "flex", alignItems: "center", gap: 6 }}>
                <MicrosoftIcon size={14} /> Microsoft Entra ID (Azure AD)
              </div>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Tenant ID *</span></label>
                <input className="chart-builder-input" type="text" value={azureTenantId} onChange={e => { setAzureTenantId(e.target.value); setErrors(v => { const n={...v}; delete n.azureTenantId; return n; }); setRolesStatus(null); }} placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" style={{ borderColor: errors.azureTenantId ? "#ef4444" : undefined }} />
                {errors.azureTenantId && <p style={{ margin: "2px 0 0", fontSize: 11.5, color: "#dc2626" }}>{errors.azureTenantId}</p>}
              </div>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Client ID *</span></label>
                <input className="chart-builder-input" type="text" value={azureClientId} onChange={e => { setAzureClientId(e.target.value); setErrors(v => { const n={...v}; delete n.azureClientId; return n; }); setRolesStatus(null); }} placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" style={{ borderColor: errors.azureClientId ? "#ef4444" : undefined }} />
                {errors.azureClientId && <p style={{ margin: "2px 0 0", fontSize: 11.5, color: "#dc2626" }}>{errors.azureClientId}</p>}
              </div>

              {/* ── App Roles setup ─────────────────────────────────────────── */}
              <div style={{ borderTop: "1px solid #e2e8f0", paddingTop: "1rem" }}>
                <div style={{ fontSize: 12, fontWeight: 600, color: "#374151", marginBottom: 6, display: "flex", alignItems: "center", gap: 6 }}>
                  <i className="fas fa-shield-halved" style={{ color: "#6366f1" }} />
                  App Roles
                </div>
                <p style={{ margin: "0 0 10px", fontSize: 11.5, color: "#64748b", lineHeight: 1.55 }}>
                  Weft requires <strong>Viewer, Analyst, Editor</strong> and <strong>Admin</strong> App Roles in your Azure AD App Registration. If you are a <strong>Global Admin</strong> in this tenant, click below to create them automatically — otherwise copy the manifest and add them manually.
                </p>

                {/* Auto-setup button — only shown when tenant + client are filled */}
                {azureTenantId.trim() && azureClientId.trim() && (
                  <button
                    type="button"
                    onClick={handleSetupRoles}
                    disabled={settingUpRoles}
                    style={{ display: "flex", alignItems: "center", gap: 7, padding: "8px 14px", borderRadius: 8, cursor: settingUpRoles ? "not-allowed" : "pointer", border: "1.5px solid #6366f1", background: settingUpRoles ? "#f8fafc" : "#eef2ff", color: "#4338ca", fontSize: 12.5, fontWeight: 600, opacity: settingUpRoles ? 0.6 : 1, marginBottom: 10 }}
                  >
                    {settingUpRoles
                      ? <><i className="fas fa-spinner fa-spin" /> Contacting Azure AD…</>
                      : <><i className="fas fa-wand-magic-sparkles" /> Auto-create App Roles</>}
                  </button>
                )}

                {/* Result */}
                {rolesStatus && (
                  <div style={{ display: "flex", gap: 8, padding: "8px 12px", borderRadius: 8, background: rolesStatus.ok ? "#f0fdf4" : "#fef2f2", border: `1px solid ${rolesStatus.ok ? "#bbf7d0" : "#fecaca"}`, fontSize: 12.5, color: rolesStatus.ok ? "#166534" : "#991b1b", lineHeight: 1.55, marginBottom: rolesStatus.showManifest ? 10 : 0 }}>
                    <i className={`fas ${rolesStatus.ok ? "fa-check-circle" : "fa-exclamation-circle"}`} style={{ marginTop: 1, flexShrink: 0 }} />
                    <span>{rolesStatus.message}</span>
                  </div>
                )}

                {/* Manual fallback — visible until roles are successfully created */}
                {(!rolesStatus || !rolesStatus.ok) && (
                  <div>
                    <p style={{ margin: "0 0 6px", fontSize: 11.5, color: "#64748b" }}>Paste this into the <strong>appRoles</strong> array in Azure Portal → App Registrations → your app → Manifest:</p>
                    <div style={{ position: "relative" }}>
                      <pre style={{ margin: 0, padding: "10px 14px", background: "#f1f5f9", border: "1px solid #e2e8f0", borderRadius: 8, fontSize: 10.5, color: "#334155", overflowX: "auto", maxHeight: 130, lineHeight: 1.5 }}>{APP_ROLES_MANIFEST}</pre>
                      <button type="button" onClick={copyManifest} style={{ position: "absolute", top: 6, right: 6, display: "flex", alignItems: "center", gap: 5, padding: "4px 10px", borderRadius: 6, border: "1px solid #cbd5e1", background: manifestCopied ? "#f0fdf4" : "white", color: manifestCopied ? "#166534" : "#475569", fontSize: 11, fontWeight: 600, cursor: "pointer", transition: "all 0.2s" }}>
                        <i className={`fas ${manifestCopied ? "fa-check" : "fa-copy"}`} />
                        {manifestCopied ? "Copied!" : "Copy"}
                      </button>
                    </div>
                  </div>
                )}
              </div>
            </div>
          )}

          {provider === "google" && (
            <div style={{ display: "flex", flexDirection: "column", gap: "1rem", padding: "1rem", borderRadius: 10, background: "#f8fafc", border: "1px solid #e2e8f0" }}>
              <div style={{ fontSize: 12, fontWeight: 600, color: "#475569", display: "flex", alignItems: "center", gap: 6 }}>
                <GoogleIcon size={14} /> Google OAuth2 <BetaBadge />
              </div>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Client ID *</span></label>
                <input className="chart-builder-input" type="text" value={googleClientId} onChange={e => { setGoogleClientId(e.target.value); setErrors(v => { const n={...v}; delete n.googleClientId; return n; }); }} placeholder="xxxxxxxxxxxx-xxxxxxxx.apps.googleusercontent.com" style={{ borderColor: errors.googleClientId ? "#ef4444" : undefined }} />
                {errors.googleClientId ? <p style={{ margin: "2px 0 0", fontSize: 11.5, color: "#dc2626" }}>{errors.googleClientId}</p> : <p style={{ margin: "4px 0 0", fontSize: 11.5, color: "#94a3b8" }}>Google Cloud Console → APIs &amp; Services → Credentials.</p>}
              </div>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Client Secret</span></label>
                <input className="chart-builder-input" type="password" value={googleSecret} onChange={e => setGoogleSecret(e.target.value)} placeholder="Leave blank to keep existing secret" />
              </div>
            </div>
          )}

          {provider === "local" && (
            <div style={{ padding: "0.75rem 1rem", borderRadius: 8, fontSize: 12.5, background: "#f8fafc", border: "1px solid #e2e8f0", color: "#475569", display: "flex", gap: 8 }}>
              <i className="fas fa-info-circle" style={{ marginTop: 1, flexShrink: 0, color: "#64748b" }} />
              Local users are managed under <strong style={{ margin: "0 3px" }}>Users</strong> in the sidebar.
            </div>
          )}

          {(provider === "azure_ad" || provider === "google") && (
            <div style={{ padding: "0.75rem 1rem", borderRadius: 8, fontSize: 12.5, background: "#eff6ff", border: "1px solid #bfdbfe", color: "#1e40af", display: "flex", gap: 8, lineHeight: 1.6 }}>
              <i className="fas fa-circle-info" style={{ marginTop: 1, flexShrink: 0 }} />
              <div>
                <div style={{ fontWeight: 600, marginBottom: 4 }}>Before saving, complete these steps in Azure Portal:</div>
                <ol style={{ margin: 0, paddingLeft: 16, display: "flex", flexDirection: "column", gap: 3 }}>
                  <li>Add the App Roles to your App Registration (use the manifest above)</li>
                  <li>Set <strong>Assignment required = Yes</strong> — Enterprise Applications → your app → Properties</li>
                  <li>Assign yourself the <strong>Admin</strong> role — Enterprise Applications → your app → Users and groups</li>
                </ol>
                <div style={{ marginTop: 6, color: "#3b82f6" }}>Users without an assigned role will be blocked from signing in.</div>
              </div>
            </div>
          )}
          <div style={{ padding: "0.75rem 1rem", borderRadius: 8, fontSize: 12.5, background: "#fffbeb", border: "1px solid #fde68a", color: "#92400e", display: "flex", gap: 8 }}>
            <i className="fas fa-triangle-exclamation" style={{ marginTop: 1, flexShrink: 0 }} />
            Changing the provider will <strong style={{ margin: "0 3px" }}>sign out all current users</strong>.
          </div>
        </div>
        <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.75rem", padding: "1rem 1.5rem", borderTop: "1px solid #f1f5f9", background: "#f9fafb", flexShrink: 0 }}>
          <Button variant="secondary" onClick={onClose} disabled={saving}>Cancel</Button>
          <Button onClick={handleSave} disabled={saving}>
            {saving ? <><i className="fas fa-spinner fa-spin" /> Saving…</> : <><i className="fas fa-right-from-bracket" /> Save &amp; Sign Out</>}
          </Button>
        </div>
      </div>
    </>
  );
}

// ── Edit Metadata Modal — dark theme (matches SetupModal design) ───────────────

const DM = {
  overlay: { position: "fixed" as const, inset: 0, background: "rgba(10,16,30,0.93)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 9999, padding: 20 },
  card: { background: "#1e293b", border: "1px solid #2d3f5c", borderRadius: 18, width: "100%", maxWidth: 520, boxShadow: "0 32px 72px rgba(0,0,0,0.6)", maxHeight: "calc(100vh - 40px)", display: "flex", flexDirection: "column" as const, overflowY: "hidden" as const },
  header: { padding: "22px 32px 18px", borderBottom: "1px solid #2d3f5c", display: "flex", justifyContent: "space-between", alignItems: "center", flexShrink: 0 },
  body: { padding: "24px 32px 8px", overflowY: "auto" as const, flex: 1 },
  footer: { padding: "16px 32px 24px", borderTop: "1px solid #2d3f5c", flexShrink: 0 },
  label: { display: "block", fontSize: 12, fontWeight: 700, color: "#94a3b8", textTransform: "uppercase" as const, letterSpacing: "0.06em", marginBottom: 5 } as React.CSSProperties,
  input: (err: boolean): React.CSSProperties => ({ width: "100%", background: "#0f172a", border: `1px solid ${err ? "#ef4444" : "#334155"}`, borderRadius: 8, padding: "10px 14px", fontSize: 13, color: "#f1f5f9", outline: "none", boxSizing: "border-box" as const, marginBottom: 4, transition: "border-color 0.15s" }),
  hint: { fontSize: 11.5, color: "#475569", marginBottom: 14, lineHeight: 1.5 } as React.CSSProperties,
  dbTypeCard: (active: boolean): React.CSSProperties => ({ background: active ? "rgba(99,102,241,0.15)" : "#0f172a", border: `1px solid ${active ? "#6366f1" : "#1e293b"}`, borderRadius: 10, padding: "10px 12px", cursor: "pointer", display: "flex", alignItems: "center", gap: 8, transition: "all 0.15s" }),
  dbTypeLabel: (active: boolean): React.CSSProperties => ({ fontSize: 12, fontWeight: active ? 700 : 500, color: active ? "#a5b4fc" : "#64748b", lineHeight: 1.3, flex: 1 }),
  btnPrimary: (disabled = false): React.CSSProperties => ({ width: "100%", background: disabled ? "#1e3a5f" : "linear-gradient(135deg,#3b82f6,#6366f1)", border: "none", borderRadius: 10, padding: "12px 20px", fontSize: 14, fontWeight: 600, color: disabled ? "#475569" : "#fff", cursor: disabled ? "not-allowed" : "pointer", marginBottom: 10, transition: "opacity 0.15s" }),
  btnSecondary: { width: "100%", background: "transparent", border: "1px solid #334155", borderRadius: 10, padding: "11px 20px", fontSize: 14, fontWeight: 500, color: "#64748b", cursor: "pointer" } as React.CSSProperties,
  errorBox: { background: "rgba(239,68,68,0.08)", border: "1px solid rgba(239,68,68,0.3)", borderRadius: 10, padding: "12px 16px", marginBottom: 18 } as React.CSSProperties,
  successBox: { background: "rgba(34,197,94,0.08)", border: "1px solid rgba(34,197,94,0.3)", borderRadius: 10, padding: "12px 16px", marginBottom: 18, fontSize: 13, color: "#86efac", display: "flex", alignItems: "center", gap: 8 } as React.CSSProperties,
};

const META_ERROR_HINTS: Record<string, { title: string; hint: string }> = {
  connection_failed: { title: "Cannot reach this host",   hint: "Check the hostname for typos and confirm the required port is reachable from this machine." },
  timeout:           { title: "Connection timed out",      hint: "The host port may be blocked by a corporate firewall or VPN. Contact your network team if needed." },
  access_denied:     { title: "Access denied",             hint: "Authentication failed. Check credentials or verify Azure AD role assignments for MSSQL-based databases." },
  db_not_found:      { title: "Database not found",        hint: "The database name was not found at this host. Names are case-sensitive — copy the exact name." },
};

function MetaConnectionTestingPanel({ target }: { target: string }) {
  return (
    <div style={{ textAlign: "center", padding: "28px 0 32px" }}>
      <style>{`
        @keyframes lx-meta-pulse { 0%{transform:scale(0.85);opacity:0.7} 50%{transform:scale(1.25);opacity:0.15} 100%{transform:scale(0.85);opacity:0.7} }
        @keyframes lx-meta-dot   { 0%,80%,100%{transform:translateY(0);opacity:0.3} 40%{transform:translateY(-5px);opacity:1} }
      `}</style>
      <div style={{ position: "relative", display: "inline-flex", alignItems: "center", justifyContent: "center", marginBottom: 20 }}>
        <div style={{ position: "absolute", width: 64, height: 64, borderRadius: "50%", border: "2px solid #6366f1", animation: "lx-meta-pulse 1.6s ease-in-out infinite" }} />
        <div style={{ width: 48, height: 48, borderRadius: "50%", background: "rgba(99,102,241,0.12)", border: "1px solid rgba(99,102,241,0.25)", display: "flex", alignItems: "center", justifyContent: "center" }}>
          <i className="fas fa-database" style={{ fontSize: 18, color: "#6366f1" }} />
        </div>
      </div>
      <div style={{ fontSize: 14, fontWeight: 600, color: "#e2e8f0", marginBottom: 4 }}>Verifying connection</div>
      {target && <div style={{ fontSize: 11.5, color: "#475569", fontFamily: "monospace", margin: "0 auto 14px", maxWidth: 320, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{target}</div>}
      <div style={{ display: "flex", justifyContent: "center", gap: 6 }}>
        {[0, 1, 2].map(i => <div key={i} style={{ width: 6, height: 6, borderRadius: "50%", background: "#6366f1", animation: "lx-meta-dot 1.2s ease-in-out infinite", animationDelay: `${i * 0.18}s` }} />)}
      </div>
    </div>
  );
}

function MetaErrorDisplay({ message, errType }: { message: string; errType?: string }) {
  const info = META_ERROR_HINTS[errType ?? ""] ?? null;
  return (
    <div style={DM.errorBox}>
      {info && <div style={{ fontSize: 13, fontWeight: 700, color: "#f87171", marginBottom: 4 }}>{info.title}</div>}
      {info && <div style={{ fontSize: 12, color: "#fca5a5", lineHeight: 1.6, marginBottom: 6 }}>{info.hint}</div>}
      <div style={{ fontSize: 11, color: "#6b7280", fontFamily: "monospace", wordBreak: "break-all" }}>{message}</div>
    </div>
  );
}

type MetaStep = "form" | "testing" | "ready";
function MetaStepBar({ step, testOk }: { step: MetaStep; testOk: boolean }) {
  type S = "done" | "active" | "pending";
  const s2: S = testOk ? "done" : step === "testing" ? "active" : "pending";
  const s3: S = testOk ? "active" : "pending";
  const dot = (st: S) => { const c = { done: "#22c55e", active: "#6366f1", pending: "#334155" }; return <div style={{ width: 10, height: 10, borderRadius: "50%", background: c[st], transition: "background 0.2s" }} />; };
  const line = (done: boolean) => <div style={{ flex: 1, height: 1, background: done ? "#22c55e" : "#334155", margin: "0 6px", transition: "background 0.2s" }} />;
  return (
    <div style={{ marginBottom: 24 }}>
      <div style={{ display: "flex", alignItems: "center", marginBottom: 6 }}>
        {dot("done")}{line(s2 === "done")}{dot(s2)}{line(s3 !== "pending")}{dot(s3)}
      </div>
      <div style={{ display: "flex", justifyContent: "space-between", fontSize: 10.5, color: "#475569", letterSpacing: "0.04em", textTransform: "uppercase" as const }}>
        <span style={{ color: "#94a3b8" }}>Configure</span>
        <span style={{ color: s2 !== "pending" ? "#94a3b8" : undefined }}>Test</span>
        <span style={{ color: s3 !== "pending" ? "#94a3b8" : undefined }}>Save</span>
      </div>
    </div>
  );
}

function EditMetadataModal({ current, onClose, onSuccess }: {
  current: MetadataConfig | null;
  gradientColors?: { light: string; dark: string };
  onClose: () => void;
  onSuccess: (msg: string) => void;
}) {
  const [form, setForm] = useState<MetadataFormState>({
    db_type: current?.db_type ?? "fabric_sql", connection_string: "",
    endpoint: current?.endpoint ?? "", host: current?.host ?? "",
    port: current?.port ?? "", database: current?.database ?? "",
  });
  const [errors,    setErrors]    = useState<Record<string, string>>({});
  const [step,      setStep]      = useState<MetaStep>("form");
  const [testResult,setTestResult]= useState<{ ok: boolean; message: string; errType?: string } | null>(null);
  const [saving,    setSaving]    = useState(false);
  const [saved,     setSaved]     = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const setField = (k: keyof MetadataFormState, v: string) => {
    setForm(f => ({ ...f, [k]: v }));
    setErrors(e => { const n = { ...e }; delete n[k]; return n; });
    setTestResult(null);
  };
  const switchType = (t: DbType) => {
    setForm({ db_type: t, connection_string: "", endpoint: "", host: "", port: "", database: "" });
    setErrors({}); setTestResult(null);
  };
  const validate = () => {
    const e: Record<string, string> = {};
    if (usesFabricConnStr(form.db_type) && !form.connection_string.trim() && !form.endpoint.trim())
      e.connection_string = "Provide a connection string (or the endpoint will be reused).";
    if (usesEndpoint(form.db_type) && !form.endpoint.trim())  e.endpoint = "Server name is required.";
    if (usesHostPort(form.db_type) && !form.host.trim())      e.host     = "Host is required.";
    if (!form.database.trim() && !usesFabricConnStr(form.db_type))
      e.database = "Database name is required.";
    setErrors(e); return Object.keys(e).length === 0;
  };
  const buildPayload = () => {
    const t = form.db_type;
    const p: Record<string, unknown> = { db_type: t };
    if (form.database.trim()) p.database = form.database;
    if (usesFabricConnStr(t)) { if (form.connection_string.trim()) p.connection_string = form.connection_string; else p.endpoint = form.endpoint; }
    else if (usesEndpoint(t)) p.endpoint = form.endpoint;
    else if (usesHostPort(t)) { p.host = form.host; p.port = form.port ? parseInt(form.port) : (t === "postgresql" ? 5432 : 3306); }
    return p;
  };
  const testingTarget = usesFabricConnStr(form.db_type)
    ? form.connection_string.match(/(?:Server|Data Source)\s*=\s*(?:tcp:)?([^,;\s]+)/i)?.[1] ?? (form.endpoint || "")
    : usesEndpoint(form.db_type) ? form.endpoint : form.host;

  const handleTest = async () => {
    if (!validate()) return;
    setStep("testing"); setTestResult(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/admin/metadata/test`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(buildPayload()) });
      if (res.ok) {
        setTestResult({ ok: true, message: "Connection successful." });
        setStep("ready");
      } else {
        const b = await res.json().catch(() => ({}));
        const msg = (b as any)?.detail?.errors?.[0]?.message ?? (b as any)?.detail ?? "Connection test failed.";
        const errType = (b as any)?.detail?.errors?.[0]?.error_type;
        setTestResult({ ok: false, message: msg, errType });
        setStep("form");
      }
    } catch (e) {
      setTestResult({ ok: false, message: String(e), errType: "connection_failed" });
      setStep("form");
    }
  };
  const handleSave = async () => {
    setSaving(true); setSaveError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/admin/metadata/update`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(buildPayload()) });
      const b = await res.json().catch(() => ({}));
      if (res.ok) {
        setSaved(true);
        // Poll /api/auth/provider — lightest endpoint, no DB needed, returns 200 as soon
        // as the API process is alive. Once it's up, reload so the page re-fetches everything.
        (async () => {
          // Wait for the process to begin shutting down before polling.
          await new Promise<void>(r => setTimeout(r, 2000));
          for (let i = 0; i < 90; i++) {
            await new Promise<void>(r => setTimeout(r, 1000));
            try {
              const p = await fetch(`${API_BASE}/api/auth/provider`, { cache: "no-store" });
              if (p.ok) { window.location.reload(); return; }
            } catch { /* still restarting */ }
          }
        })();
      }
      else { setSaveError((b as any)?.detail?.errors?.[0]?.message ?? (b as any)?.detail ?? "Update failed."); }
    } catch (e) { setSaveError(String(e)); }
    finally { setSaving(false); }
  };

  return (
    <div style={DM.overlay} onClick={saved ? undefined : onClose}>
      <div style={DM.card} onClick={e => e.stopPropagation()}>
        {/* Header */}
        <div style={DM.header}>
          <div>
            <h2 style={{ margin: 0, fontSize: 18, fontWeight: 700, color: "#f1f5f9" }}>Metadata Database</h2>
            <p style={{ margin: "3px 0 0", fontSize: 13, color: "#64748b" }}>Update your connection details and test before saving.</p>
          </div>
          {!saved && <button onClick={onClose} style={{ background: "none", border: "none", color: "#64748b", cursor: "pointer", fontSize: 18, padding: "4px 6px", lineHeight: 1 }}>
            <i className="fas fa-times" />
          </button>}
        </div>

        {/* Body */}
        <div style={DM.body}>

          {saved ? (
            <div style={{ textAlign: "center", padding: "32px 0 24px" }}>
              <div style={{ width: 56, height: 56, borderRadius: "50%", background: "rgba(34,197,94,0.12)", border: "1px solid rgba(34,197,94,0.3)", display: "flex", alignItems: "center", justifyContent: "center", margin: "0 auto 16px" }}>
                <i className="fas fa-check" style={{ fontSize: 22, color: "#4ade80" }} />
              </div>
              <div style={{ fontSize: 16, fontWeight: 700, color: "#f1f5f9", marginBottom: 6 }}>Configuration saved</div>
              <div style={{ fontSize: 13, color: "#64748b", marginBottom: 20 }}>Waiting for the API to restart…</div>
              <div style={{ display: "flex", justifyContent: "center", gap: 5, marginBottom: 24 }}>
                {[0,1,2].map(i => <div key={i} style={{ width: 6, height: 6, borderRadius: "50%", background: "#6366f1", animation: "lx-meta-dot 1.2s ease-in-out infinite", animationDelay: `${i * 0.18}s` }} />)}
              </div>
              <button
                onClick={() => window.location.reload()}
                style={{ background: "none", border: "none", cursor: "pointer", fontSize: 12, color: "#475569", textDecoration: "underline" }}
              >
                Taking too long? Reload manually
              </button>
            </div>
          ) : (
          <>
          <MetaStepBar step={step} testOk={testResult?.ok ?? false} />

          {step === "testing" ? (
            <MetaConnectionTestingPanel target={testingTarget} />
          ) : (
            <>
              {/* DB type picker */}
              <label style={DM.label}>Database Type</label>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, marginBottom: 20 }}>
                {DB_TYPES.map(({ key, label, icon }) => {
                  const active = form.db_type === key;
                  return (
                    <div key={key} style={DM.dbTypeCard(active)} onClick={() => switchType(key)}>
                      <span style={{ flexShrink: 0, opacity: active ? 1 : 0.55 }}>{icon}</span>
                      <span style={DM.dbTypeLabel(active)}>{label}</span>
                      {(key === "postgresql" || key === "mysql") && (
                        <span style={{ fontSize: 9, fontWeight: 700, color: "#f59e0b", background: "rgba(245,158,11,0.12)", border: "1px solid rgba(245,158,11,0.3)", borderRadius: 4, padding: "1px 5px", letterSpacing: "0.05em", textTransform: "uppercase" as const }}>Beta</span>
                      )}
                    </div>
                  );
                })}
              </div>

              {/* Error / success feedback */}
              {testResult && !testResult.ok && (
                <MetaErrorDisplay message={testResult.message} errType={testResult.errType} />
              )}
              {testResult?.ok && (
                <div style={DM.successBox}>
                  <i className="fas fa-check-circle" /> Connection successful — ready to save.
                </div>
              )}
              {saveError && (
                <div style={{ ...DM.errorBox, marginBottom: 14 }}>
                  <div style={{ fontSize: 12, color: "#fca5a5" }}>{saveError}</div>
                </div>
              )}

              {/* Fabric SQL */}
              {usesFabricConnStr(form.db_type) && (
                <>
                  <label style={DM.label} htmlFor="meta-connstr">ODBC Connection String</label>
                  <textarea
                    id="meta-connstr"
                    style={{ ...DM.input(!!errors.connection_string), resize: "vertical", minHeight: 80, fontFamily: "monospace", fontSize: 12 }}
                    placeholder={"Server=tcp:xyz.database.fabric.microsoft.com,1433;Initial Catalog=MyDatabase;Authentication=ActiveDirectoryInteractive;Encrypt=True;"}
                    value={form.connection_string}
                    onChange={e => setField("connection_string", e.target.value)}
                    autoComplete="off" spellCheck={false}
                  />
                  {errors.connection_string && <p style={{ margin: "2px 0 6px", fontSize: 11.5, color: "#ef4444" }}>{errors.connection_string}</p>}
                  <p style={DM.hint}>Fabric workspace → SQL Database → Connection strings → ODBC. Leave blank to reuse the saved endpoint.</p>
                  <label style={DM.label} htmlFor="meta-db-fabric">Database Name</label>
                  <input id="meta-db-fabric" style={DM.input(!!errors.database)} type="text" value={form.database} onChange={e => setField("database", e.target.value)} placeholder="Weft" autoComplete="off" />
                  <p style={DM.hint}>Leave blank to use the database parsed from the connection string.</p>
                </>
              )}

              {/* Azure SQL */}
              {usesEndpoint(form.db_type) && (
                <>
                  <label style={DM.label} htmlFor="meta-ep">Server Name *</label>
                  <input id="meta-ep" style={DM.input(!!errors.endpoint)} type="text" value={form.endpoint} onChange={e => setField("endpoint", e.target.value)} placeholder="my-server.database.windows.net" autoComplete="off" />
                  {errors.endpoint && <p style={{ margin: "2px 0 6px", fontSize: 11.5, color: "#ef4444" }}>{errors.endpoint}</p>}
                  <p style={DM.hint}>Azure Portal → SQL Server → Server name.</p>
                  <label style={DM.label} htmlFor="meta-db-az">Database Name *</label>
                  <input id="meta-db-az" style={DM.input(!!errors.database)} type="text" value={form.database} onChange={e => setField("database", e.target.value)} placeholder="weft-metadata" autoComplete="off" />
                  {errors.database && <p style={{ margin: "2px 0 6px", fontSize: 11.5, color: "#ef4444" }}>{errors.database}</p>}
                </>
              )}

              {/* PostgreSQL / MySQL */}
              {usesHostPort(form.db_type) && (
                <>
                  <label style={DM.label}>Host &amp; Port *</label>
                  <div style={{ display: "flex", gap: 8, marginBottom: 4 }}>
                    <input style={{ ...DM.input(!!errors.host), flex: 1, marginBottom: 0 }} type="text" value={form.host} onChange={e => setField("host", e.target.value)} placeholder="localhost or db.example.com" autoComplete="off" />
                    <input style={{ ...DM.input(false), width: 90, marginBottom: 0 }} type="number" value={form.port} onChange={e => setField("port", e.target.value)} placeholder={form.db_type === "postgresql" ? "5432" : "3306"} />
                  </div>
                  {errors.host && <p style={{ margin: "2px 0 6px", fontSize: 11.5, color: "#ef4444" }}>{errors.host}</p>}
                  <p style={DM.hint}>Hostname or IP address of your database server.</p>
                  <label style={DM.label} htmlFor="meta-db-pg">Database Name *</label>
                  <input id="meta-db-pg" style={DM.input(!!errors.database)} type="text" value={form.database} onChange={e => setField("database", e.target.value)} placeholder="weft" autoComplete="off" />
                  {errors.database && <p style={{ margin: "2px 0 6px", fontSize: 11.5, color: "#ef4444" }}>{errors.database}</p>}
                  <p style={{ ...DM.hint, color: "#4ade80", marginBottom: 4 }}>
                    <i className="fas fa-shield-check" style={{ marginRight: 6 }} />Connects via Azure AD Managed Identity — no credentials required.
                  </p>
                </>
              )}

              <div style={{ background: "rgba(245,158,11,0.07)", border: "1px solid rgba(245,158,11,0.25)", borderRadius: 8, padding: "10px 14px", fontSize: 12, color: "#fcd34d", display: "flex", gap: 8, marginBottom: 16, marginTop: 4 }}>
                <i className="fas fa-triangle-exclamation" style={{ marginTop: 1, flexShrink: 0 }} />
                Saving will restart the Weft API. Existing data is preserved.
              </div>
            </>
          )}
          </>
          )}
        </div>

        {/* Footer */}
        <div style={DM.footer}>
          {!saved && step !== "testing" && (
            testResult?.ok ? (
              <>
                <button style={DM.btnPrimary(saving)} onClick={handleSave} disabled={saving}>
                  {saving
                    ? <><i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />Saving…</>
                    : <><i className="fas fa-rocket" style={{ marginRight: 8 }} />Save &amp; Restart</>}
                </button>
                <button style={DM.btnSecondary} onClick={() => { setTestResult(null); setStep("form"); }}>
                  <i className="fas fa-edit" style={{ marginRight: 8 }} />Edit Connection
                </button>
              </>
            ) : (
              <>
                <button style={DM.btnPrimary(false)} onClick={handleTest}>
                  <i className="fas fa-plug" style={{ marginRight: 8 }} />Test Connection
                </button>
                <button style={DM.btnSecondary} onClick={onClose}>Cancel</button>
              </>
            )
          )}
        </div>
      </div>
    </div>
  );
}
