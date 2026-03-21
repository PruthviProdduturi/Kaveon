"use client";

import React, { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import { useTheme } from "../../../contexts/ThemeContext";
import { Button } from "../../../components/Button";
import { ListPageShell } from "../../../components/ListPageShell";
import { type AuthProvider } from "../../../auth/useAuth";
import { type UserRole } from "../../../auth/useAuth";

// ── Types ──────────────────────────────────────────────────────────────────────

interface AuthConfig {
  provider: AuthProvider;
  azure_client_id?: string;
  azure_tenant_id?: string;
  google_client_id?: string;
}

interface LocalUser {
  id: number;
  username: string;
  email: string;
  role: UserRole;
  is_active: boolean;
  created_at: string;
}

const PROVIDER_META: Record<AuthProvider, { label: string; icon: string; color: string; bg: string; border: string }> = {
  local:    { label: "Local Login",          icon: "fa-lock",     color: "#0f172a", bg: "#f8fafc", border: "#e2e8f0" },
  azure_ad: { label: "Microsoft Azure AD",   icon: "fa-microsoft", color: "#0078d4", bg: "#eff6ff", border: "#bfdbfe" },
  google:   { label: "Google OAuth2",        icon: "fa-google",   color: "#ea4335", bg: "#fff1f0", border: "#fecaca" },
};

const ROLES: UserRole[] = ["Viewer", "Analyst", "Editor", "Admin"];

function formatDate(val: string | null | undefined) {
  if (!val) return "—";
  try {
    return new Date(val).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
  } catch { return val; }
}

// ── Page ───────────────────────────────────────────────────────────────────────

export default function AuthSettingsPage() {
  const router = useRouter();
  const { isAdmin, loading: roleLoading } = useRole() as ReturnType<typeof useRole> & { loading?: boolean };
  const { primaryColor } = useTheme();

  const [config, setConfig] = useState<AuthConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showModal, setShowModal] = useState(false);
  const [banner, setBanner] = useState<{ ok: boolean; message: string } | null>(null);

  // Local users (only loaded when provider === "local")
  const [localUsers, setLocalUsers] = useState<LocalUser[]>([]);
  const [usersLoading, setUsersLoading] = useState(false);
  const [usersError, setUsersError] = useState<string | null>(null);
  const [showAddUser, setShowAddUser] = useState(false);
  const [deletingId, setDeletingId] = useState<number | null>(null);

  useEffect(() => {
    if (!roleLoading && !isAdmin) router.replace("/");
  }, [roleLoading, isAdmin, router]);

  const loadConfig = () => {
    setLoading(true);
    msalFetch(`${API_BASE}/api/v1/admin/auth`)
      .then(r => r.json())
      .then((d: AuthConfig) => { setConfig(d); setLoading(false); })
      .catch(e => { setError(String(e)); setLoading(false); });
  };

  const loadLocalUsers = () => {
    setUsersLoading(true);
    msalFetch(`${API_BASE}/api/v1/admin/local-users`)
      .then(r => r.json())
      .then((d: LocalUser[]) => { setLocalUsers(Array.isArray(d) ? d : []); setUsersLoading(false); })
      .catch(e => { setUsersError(String(e)); setUsersLoading(false); });
  };

  useEffect(() => {
    if (!roleLoading && isAdmin) loadConfig();
  }, [roleLoading, isAdmin]);

  useEffect(() => {
    if (config?.provider === "local" && isAdmin) loadLocalUsers();
  }, [config?.provider, isAdmin]);

  if (!isAdmin && !roleLoading) return null;

  const meta = config ? PROVIDER_META[config.provider] : PROVIDER_META.local;

  const handleDeleteUser = async (id: number, username: string) => {
    if (!confirm(`Deactivate user "${username}"?`)) return;
    setDeletingId(id);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/admin/local-users/${id}`, { method: "DELETE" });
      if (!res.ok) throw new Error(`Failed (${res.status})`);
      await loadLocalUsers();
    } catch (e) {
      setUsersError(e instanceof Error ? e.message : "Failed to deactivate user");
    } finally {
      setDeletingId(null);
    }
  };

  return (
    <ListPageShell
      icon="fa-lock"
      title="Authentication"
      subtitle="Configure how users sign in to LooMX. Changes take effect immediately; all sessions will need to re-authenticate."
      loading={loading || roleLoading}
      loadingMessage="Loading authentication config"
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
        <div className="card" style={{ padding: "1.5rem", marginBottom: "1.5rem" }}>
          {/* Icon + label row */}
          <div style={{ display: "flex", alignItems: "center", gap: 14, marginBottom: "1.5rem" }}>
            <div style={{
              width: 52, height: 52, borderRadius: 13, flexShrink: 0,
              background: meta.bg,
              border: `1px solid ${meta.border}`,
              display: "flex", alignItems: "center", justifyContent: "center",
            }}>
              <i className={`fab ${meta.icon}`} style={{ fontSize: 22, color: meta.color }} />
            </div>
            <div>
              <div style={{ fontSize: 16, fontWeight: 700, color: "#0f172a" }}>{meta.label}</div>
              <div style={{ fontSize: 12.5, color: "#64748b", marginTop: 1 }}>Active provider</div>
            </div>
          </div>

          {/* Field grid */}
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(200px, 1fr))", gap: "1.25rem" }}>
            {[
              { label: "Provider",  value: config.provider,          mono: true  },
              ...(config.azure_tenant_id ? [{ label: "Azure Tenant ID",  value: config.azure_tenant_id,  mono: true }] : []),
              ...(config.azure_client_id ? [{ label: "Azure Client ID",  value: config.azure_client_id,  mono: true }] : []),
              ...(config.google_client_id ? [{ label: "Google Client ID", value: config.google_client_id, mono: true }] : []),
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

          {/* Info note */}
          <div style={{ marginTop: "1.5rem", paddingTop: "1.25rem", borderTop: "1px solid #f1f5f9", fontSize: 12.5, color: "#64748b", display: "flex", gap: 8 }}>
            <i className="fas fa-info-circle" style={{ color: primaryColor, marginTop: 1, flexShrink: 0 }} />
            Switching providers will sign out all current users. Make sure the new provider is fully configured before saving.
          </div>
        </div>
      )}

      {/* Local users section */}
      {config?.provider === "local" && (
        <div>
          {/* Section header */}
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: "1rem" }}>
            <div>
              <h2 style={{ margin: 0, fontSize: "1rem", fontWeight: 700, color: "#0f172a" }}>Local Users</h2>
              <p style={{ margin: "2px 0 0", fontSize: 12.5, color: "#64748b" }}>
                Manage users who sign in with a local username and password.
              </p>
            </div>
            <Button onClick={() => setShowAddUser(true)}>
              <i className="fas fa-user-plus" /> Add User
            </Button>
          </div>

          {/* Users error */}
          {usersError && (
            <div style={{ padding: "0.75rem 1rem", borderRadius: 8, background: "#fef2f2", color: "#991b1b", border: "1px solid #fecaca", fontSize: 13, marginBottom: "1rem" }}>
              <i className="fas fa-exclamation-circle" style={{ marginRight: 6 }} />{usersError}
            </div>
          )}

          {/* Users loading */}
          {usersLoading && (
            <div style={{ padding: "1.5rem", textAlign: "center", color: "#64748b", fontSize: 13 }}>
              <i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />Loading users…
            </div>
          )}

          {/* Users table */}
          {!usersLoading && localUsers.length > 0 && (
            <div className="card">
              <div className="results-table-container">
                <table className="results-table">
                  <thead>
                    <tr>
                      <th><span className="column-header-label">Username</span></th>
                      <th><span className="column-header-label">Email</span></th>
                      <th><span className="column-header-label">Role</span></th>
                      <th><span className="column-header-label">Created</span></th>
                      <th><span className="column-header-label">Status</span></th>
                      <th><span className="column-header-label">Actions</span></th>
                    </tr>
                  </thead>
                  <tbody>
                    {localUsers.map(u => (
                      <tr key={u.id}>
                        <td style={{ fontSize: 13, fontFamily: "monospace" }}>{u.username}</td>
                        <td style={{ fontSize: 13, color: "#64748b" }}>{u.email || "—"}</td>
                        <td>
                          <span style={{
                            display: "inline-flex", alignItems: "center", gap: "0.35rem",
                            padding: "0.2rem 0.6rem", borderRadius: 6,
                            fontSize: "0.75rem", fontWeight: 600,
                            background: "#f1f5f9", color: "#475569",
                          }}>
                            {u.role}
                          </span>
                        </td>
                        <td className="muted" style={{ fontSize: 13 }}>{formatDate(u.created_at)}</td>
                        <td>
                          <span style={{
                            display: "inline-flex", alignItems: "center", gap: "0.3rem",
                            padding: "0.2rem 0.6rem", borderRadius: 6,
                            fontSize: "0.75rem", fontWeight: 600,
                            background: u.is_active ? "#f0fdf4" : "#fef2f2",
                            color: u.is_active ? "#166534" : "#991b1b",
                          }}>
                            <i className={`fas ${u.is_active ? "fa-check-circle" : "fa-times-circle"}`} style={{ fontSize: "0.65rem" }} />
                            {u.is_active ? "Active" : "Inactive"}
                          </span>
                        </td>
                        <td className="actions-cell">
                          <div className="row-actions">
                            <button
                              type="button"
                              className="action-icon-btn"
                              title="Deactivate user"
                              aria-label="Deactivate user"
                              disabled={deletingId === u.id}
                              onClick={() => handleDeleteUser(u.id, u.username)}
                            >
                              <i className={deletingId === u.id ? "fas fa-spinner fa-spin" : "fas fa-trash"} aria-hidden="true" />
                            </button>
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}

          {/* Empty state */}
          {!usersLoading && localUsers.length === 0 && !usersError && (
            <div className="card page-empty-card">
              <p className="page-empty-title">No local users yet</p>
              <p className="page-empty-body">Click "Add User" to create the first local account.</p>
              <Button style={{ marginTop: 12 }} onClick={() => setShowAddUser(true)}>
                <i className="fas fa-user-plus" /> Add First User
              </Button>
            </div>
          )}
        </div>
      )}

      {/* Edit auth config modal */}
      {showModal && (
        <EditAuthModal
          current={config}
          onClose={() => setShowModal(false)}
          onSuccess={(msg) => {
            setShowModal(false);
            setBanner({ ok: true, message: msg });
            setTimeout(() => loadConfig(), 1500);
          }}
        />
      )}

      {/* Add user modal */}
      {showAddUser && (
        <AddUserModal
          onClose={() => setShowAddUser(false)}
          onSuccess={() => { setShowAddUser(false); loadLocalUsers(); }}
        />
      )}
    </ListPageShell>
  );
}

// ── Edit Auth Modal ────────────────────────────────────────────────────────────

interface EditAuthModalProps {
  current: AuthConfig | null;
  onClose: () => void;
  onSuccess: (msg: string) => void;
}

function EditAuthModal({ current, onClose, onSuccess }: EditAuthModalProps) {
  const { gradientColors } = useTheme();

  const [selectedProvider, setSelectedProvider] = useState<AuthProvider>(current?.provider ?? "local");
  const [azureTenantId, setAzureTenantId]   = useState(current?.azure_tenant_id ?? "");
  const [azureClientId, setAzureClientId]   = useState(current?.azure_client_id ?? "");
  const [googleClientId, setGoogleClientId] = useState(current?.google_client_id ?? "");
  const [googleClientSecret, setGoogleClientSecret] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const PROVIDERS: Array<{ key: AuthProvider; label: string; icon: string; description: string }> = [
    { key: "local",    label: "Local Login",        icon: "fa-lock",      description: "Username & password stored in LooMX database" },
    { key: "azure_ad", label: "Microsoft Azure AD", icon: "fa-microsoft",  description: "Single sign-on via Microsoft Entra ID / Azure AD" },
    { key: "google",   label: "Google OAuth2",      icon: "fa-google",    description: "Sign in with Google accounts" },
  ];

  const handleSave = async () => {
    setSaving(true);
    setSaveError(null);
    try {
      const payload: Record<string, unknown> = { provider: selectedProvider };
      if (selectedProvider === "azure_ad") {
        payload.azure_tenant_id = azureTenantId;
        payload.azure_client_id = azureClientId;
      }
      if (selectedProvider === "google") {
        payload.google_client_id = googleClientId;
        if (googleClientSecret) payload.google_client_secret = googleClientSecret;
      }

      const res = await msalFetch(`${API_BASE}/api/v1/admin/auth`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      const body = await res.json().catch(() => ({}));
      if (res.ok) {
        onSuccess("Auth provider updated. Users will need to sign in again.");
      } else {
        const msg: string = (body as any)?.detail?.errors?.[0]?.message ?? (body as any)?.detail ?? "Update failed.";
        setSaveError(msg);
      }
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  };

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
            <h2 style={{ margin: 0, fontSize: "1.1rem", fontWeight: 700, color: "#0f172a" }}>Edit Authentication</h2>
            <p style={{ margin: "2px 0 0", fontSize: 12, color: "#64748b" }}>Choose a sign-in method for LooMX.</p>
          </div>
          <button onClick={onClose} style={{ background: "none", border: "none", fontSize: "1.1rem", cursor: "pointer", color: "#94a3b8", width: 30, height: 30, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6 }}>
            <i className="fas fa-times" />
          </button>
        </div>

        {/* Body */}
        <div style={{ overflowY: "auto", flex: 1, padding: "1.25rem 1.5rem", display: "flex", flexDirection: "column", gap: "1rem" }}>

          {/* Save error */}
          {saveError && (
            <div style={{ padding: "0.75rem 1rem", borderRadius: 8, background: "#fef2f2", color: "#991b1b", border: "1px solid #fecaca", fontSize: 13, display: "flex", gap: "0.6rem", alignItems: "center" }}>
              <i className="fas fa-exclamation-circle" />{saveError}
            </div>
          )}

          {/* Provider picker */}
          <div className="chart-builder-field">
            <label className="chart-builder-label"><span>Auth Provider</span></label>
            <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 4 }}>
              {PROVIDERS.map(({ key, label, icon, description }) => {
                const active = selectedProvider === key;
                const m = PROVIDER_META[key];
                return (
                  <button
                    key={key}
                    type="button"
                    onClick={() => setSelectedProvider(key)}
                    style={{
                      display: "flex", alignItems: "center", gap: 12, padding: "0.75rem 1rem",
                      borderRadius: 8, cursor: "pointer", textAlign: "left",
                      border: `2px solid ${active ? m.color : "#e2e8f0"}`,
                      background: active ? m.bg : "white",
                      transition: "all 0.15s",
                    }}
                  >
                    <div style={{
                      width: 36, height: 36, borderRadius: 9, flexShrink: 0,
                      background: active ? m.bg : "#f8fafc",
                      border: `1px solid ${active ? m.border : "#e2e8f0"}`,
                      display: "flex", alignItems: "center", justifyContent: "center",
                    }}>
                      <i className={`fab ${icon}`} style={{ fontSize: 16, color: active ? m.color : "#94a3b8" }} />
                    </div>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontSize: 13.5, fontWeight: active ? 700 : 500, color: active ? m.color : "#374151" }}>{label}</div>
                      <div style={{ fontSize: 11.5, color: "#94a3b8", marginTop: 1 }}>{description}</div>
                    </div>
                    {active && <i className="fas fa-check-circle" style={{ color: m.color, fontSize: 16, flexShrink: 0 }} />}
                  </button>
                );
              })}
            </div>
          </div>

          {/* Local: info note */}
          {selectedProvider === "local" && (
            <div style={{
              padding: "0.75rem 1rem", borderRadius: 8, fontSize: 12.5,
              background: "#f8fafc", border: "1px solid #e2e8f0",
              color: "#475569", display: "flex", gap: 8, alignItems: "flex-start",
            }}>
              <i className="fas fa-info-circle" style={{ marginTop: 1, flexShrink: 0, color: "#64748b" }} />
              <span>Local users are managed in the <strong>Local Users</strong> section below the config card. No extra credentials are required here.</span>
            </div>
          )}

          {/* Azure AD fields */}
          {selectedProvider === "azure_ad" && (
            <>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Tenant ID *</span></label>
                <input className="chart-builder-input" type="text" value={azureTenantId}
                  onChange={e => setAzureTenantId(e.target.value)}
                  placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" />
                <p style={{ margin: "4px 0 0", fontSize: 11.5, color: "#94a3b8" }}>
                  Found in Azure Portal → Microsoft Entra ID → Overview → Tenant ID.
                </p>
              </div>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Client ID *</span></label>
                <input className="chart-builder-input" type="text" value={azureClientId}
                  onChange={e => setAzureClientId(e.target.value)}
                  placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" />
                <p style={{ margin: "4px 0 0", fontSize: 11.5, color: "#94a3b8" }}>
                  Found in Azure Portal → App Registrations → your app → Application (client) ID.
                </p>
              </div>
            </>
          )}

          {/* Google OAuth2 fields */}
          {selectedProvider === "google" && (
            <>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Client ID *</span></label>
                <input className="chart-builder-input" type="text" value={googleClientId}
                  onChange={e => setGoogleClientId(e.target.value)}
                  placeholder="xxxxxxxxxxxx-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.apps.googleusercontent.com" />
                <p style={{ margin: "4px 0 0", fontSize: 11.5, color: "#94a3b8" }}>
                  Found in Google Cloud Console → APIs &amp; Services → Credentials.
                </p>
              </div>
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Client Secret</span></label>
                <input className="chart-builder-input" type="password" value={googleClientSecret}
                  onChange={e => setGoogleClientSecret(e.target.value)}
                  placeholder="Leave blank to keep existing secret" />
              </div>
            </>
          )}

          {/* Warning */}
          <div style={{
            padding: "0.75rem 1rem", borderRadius: 8, fontSize: 12.5,
            background: "#fffbeb", border: "1px solid #fde68a",
            color: "#92400e", display: "flex", gap: 8, alignItems: "flex-start",
          }}>
            <i className="fas fa-triangle-exclamation" style={{ marginTop: 1, flexShrink: 0 }} />
            <span>Changing the auth provider will <strong>sign out all current users</strong>. Ensure the new configuration is correct before saving.</span>
          </div>
        </div>

        {/* Footer */}
        <div style={{
          display: "flex", justifyContent: "flex-end", alignItems: "center", gap: "0.75rem",
          padding: "1rem 1.5rem", borderTop: "1px solid #f1f5f9",
          background: "#f9fafb", flexShrink: 0,
        }}>
          <Button type="button" onClick={handleSave} disabled={saving}>
            {saving
              ? <><i className="fas fa-spinner fa-spin" /> Saving…</>
              : <><i className="fas fa-save" /> Save</>
            }
          </Button>
          <Button type="button" variant="secondary" onClick={onClose} disabled={saving}>Cancel</Button>
        </div>
      </div>
    </>
  );
}

// ── Add User Modal ─────────────────────────────────────────────────────────────

interface AddUserModalProps {
  onClose: () => void;
  onSuccess: () => void;
}

function AddUserModal({ onClose, onSuccess }: AddUserModalProps) {
  const { gradientColors } = useTheme();

  const [username, setUsername]   = useState("");
  const [email, setEmail]         = useState("");
  const [password, setPassword]   = useState("");
  const [role, setRole]           = useState<UserRole>("Viewer");
  const [saving, setSaving]       = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!username.trim() || !password.trim()) return;
    setSaving(true);
    setSaveError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/admin/local-users`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ username: username.trim(), email: email.trim() || null, password, role }),
      });
      const body = await res.json().catch(() => ({}));
      if (res.ok) {
        onSuccess();
      } else {
        const msg: string = (body as any)?.detail?.errors?.[0]?.message ?? (body as any)?.detail ?? "Failed to create user.";
        setSaveError(msg);
      }
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      {/* Backdrop */}
      <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.45)", zIndex: 1000 }} onClick={onClose} />

      {/* Dialog */}
      <div style={{
        position: "fixed", top: "50%", left: "50%", transform: "translate(-50%,-50%)",
        background: "white", borderRadius: 14, boxShadow: "0 24px 64px rgba(0,0,0,0.22)",
        zIndex: 1001, width: "90%", maxWidth: 460, maxHeight: "90vh",
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
            <h2 style={{ margin: 0, fontSize: "1.1rem", fontWeight: 700, color: "#0f172a" }}>Add Local User</h2>
            <p style={{ margin: "2px 0 0", fontSize: 12, color: "#64748b" }}>Create a new local account.</p>
          </div>
          <button onClick={onClose} style={{ background: "none", border: "none", fontSize: "1.1rem", cursor: "pointer", color: "#94a3b8", width: 30, height: 30, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6 }}>
            <i className="fas fa-times" />
          </button>
        </div>

        {/* Body */}
        <form onSubmit={handleSubmit} style={{ overflowY: "auto", flex: 1, padding: "1.25rem 1.5rem", display: "flex", flexDirection: "column", gap: "1rem" }}>

          {saveError && (
            <div style={{ padding: "0.75rem 1rem", borderRadius: 8, background: "#fef2f2", color: "#991b1b", border: "1px solid #fecaca", fontSize: 13, display: "flex", gap: "0.6rem", alignItems: "center" }}>
              <i className="fas fa-exclamation-circle" />{saveError}
            </div>
          )}

          <div className="chart-builder-field">
            <label className="chart-builder-label"><span>Username *</span></label>
            <input className="chart-builder-input" type="text" value={username}
              onChange={e => setUsername(e.target.value)} placeholder="e.g. jdoe" required autoFocus />
          </div>

          <div className="chart-builder-field">
            <label className="chart-builder-label"><span>Email (optional)</span></label>
            <input className="chart-builder-input" type="email" value={email}
              onChange={e => setEmail(e.target.value)} placeholder="jdoe@example.com" />
          </div>

          <div className="chart-builder-field">
            <label className="chart-builder-label"><span>Password *</span></label>
            <input className="chart-builder-input" type="password" value={password}
              onChange={e => setPassword(e.target.value)} placeholder="Initial password" required />
          </div>

          <div className="chart-builder-field">
            <label className="chart-builder-label"><span>Role</span></label>
            <select
              className="chart-builder-input"
              value={role}
              onChange={e => setRole(e.target.value as UserRole)}
            >
              {ROLES.map(r => <option key={r} value={r}>{r}</option>)}
            </select>
          </div>

          {/* Footer */}
          <div style={{
            display: "flex", justifyContent: "flex-end", alignItems: "center", gap: "0.75rem",
            paddingTop: "0.75rem", borderTop: "1px solid #f1f5f9", marginTop: "auto",
          }}>
            <Button type="submit" disabled={saving || !username.trim() || !password.trim()}>
              {saving
                ? <><i className="fas fa-spinner fa-spin" /> Creating…</>
                : <><i className="fas fa-user-plus" /> Create User</>
              }
            </Button>
            <Button type="button" variant="secondary" onClick={onClose} disabled={saving}>Cancel</Button>
          </div>
        </form>
      </div>
    </>
  );
}
