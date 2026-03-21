"use client";

import React, { useEffect, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useAuth } from "../../../auth/useAuth";
import { useTheme } from "../../../contexts/ThemeContext";
import { useRole } from "../../../hooks/useRole";
import { Button } from "../../../components/Button";
import { ListPageShell } from "../../../components/ListPageShell";

interface AIProvider {
  id: number;
  provider: string;
  label: string;
  model: string;
  is_active: boolean;
  created_by?: string;
}

interface UserKey {
  provider: string;
  model: string | null;
  api_key_hint: string;
  created_at: string;
}

// SVG logos for providers that don't have Font Awesome free icons
const PROVIDER_LOGOS: Record<string, React.ReactNode> = {
  anthropic: (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="#c17b3f">
      <path d="M13.827 3.52h3.603L24 20h-3.603l-6.57-16.48zm-3.654 0H6.57L0 20h3.603l1.351-3.384h6.932l1.351 3.384h3.603L10.173 3.52zm-3.93 10.14 2.26-5.668 2.26 5.668H6.243z"/>
    </svg>
  ),
  openai: (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="#0d9488">
      <path d="M22.282 9.821a5.985 5.985 0 0 0-.516-4.91 6.046 6.046 0 0 0-6.51-2.9A6.065 6.065 0 0 0 4.981 4.18a5.985 5.985 0 0 0-3.998 2.9 6.046 6.046 0 0 0 .743 7.097 5.98 5.98 0 0 0 .51 4.911 6.051 6.051 0 0 0 6.515 2.9A5.985 5.985 0 0 0 13.26 24a6.056 6.056 0 0 0 5.772-4.206 5.99 5.99 0 0 0 3.997-2.9 6.056 6.056 0 0 0-.747-7.073zM13.26 22.43a4.476 4.476 0 0 1-2.876-1.04l.141-.081 4.779-2.758a.795.795 0 0 0 .392-.681v-6.737l2.02 1.168a.071.071 0 0 1 .038.052v5.583a4.504 4.504 0 0 1-4.494 4.494zM3.6 18.304a4.47 4.47 0 0 1-.535-3.014l.142.085 4.783 2.759a.771.771 0 0 0 .78 0l5.843-3.369v2.332a.08.08 0 0 1-.033.062L9.74 19.95a4.5 4.5 0 0 1-6.14-1.646zM2.34 7.896a4.485 4.485 0 0 1 2.366-1.973V11.6a.766.766 0 0 0 .388.677l5.815 3.354-2.02 1.168a.076.076 0 0 1-.071 0l-4.83-2.786A4.504 4.504 0 0 1 2.34 7.896zm16.597 3.855-5.843-3.369 2.02-1.168a.076.076 0 0 1 .071 0l4.83 2.782a4.492 4.492 0 0 1-.676 8.101v-5.676a.79.79 0 0 0-.402-.67zm2.01-3.023-.141-.085-4.774-2.782a.776.776 0 0 0-.785 0L9.409 9.23V6.897a.066.066 0 0 1 .028-.061l4.83-2.787a4.5 4.5 0 0 1 6.68 4.66zm-12.64 4.135-2.02-1.164a.08.08 0 0 1-.038-.057V6.075a4.5 4.5 0 0 1 7.375-3.453l-.142.08L8.704 5.46a.795.795 0 0 0-.393.681zm1.097-2.365 2.602-1.5 2.607 1.5v2.999l-2.597 1.5-2.607-1.5z"/>
    </svg>
  ),
  github: (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="#0f172a">
      <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12"/>
    </svg>
  ),
};

const PROVIDER_META: Record<string, {
  label: string; color: string; bg: string; border: string;
  models: string[];
  keyLabel: string;
  keyPlaceholder: string;
  howToGet: { step: string; url?: string }[];
}> = {
  anthropic: {
    label: "Anthropic (Claude)",
    color: "#c17b3f",
    bg: "#fdf8f0",
    border: "#f0d5a8",
    models: ["claude-sonnet-4-6", "claude-opus-4-6", "claude-haiku-4-5-20251001"],
    keyLabel: "Anthropic API Key",
    keyPlaceholder: "sk-ant-api03-...",
    howToGet: [
      { step: "Sign in to the Anthropic Console", url: "https://console.anthropic.com" },
      { step: "Go to API Keys in the left sidebar" },
      { step: "Click Create Key, give it a name, copy it here" },
    ],
  },
  openai: {
    label: "OpenAI",
    color: "#0d9488",
    bg: "#f0fdfa",
    border: "#99f6e4",
    models: ["gpt-4o", "gpt-4o-mini", "gpt-4-turbo"],
    keyLabel: "OpenAI API Key",
    keyPlaceholder: "sk-proj-...",
    howToGet: [
      { step: "Sign in to the OpenAI Platform", url: "https://platform.openai.com" },
      { step: "Go to API Keys under your account" },
      { step: "Click Create new secret key, copy it here" },
    ],
  },
  github: {
    label: "GitHub Models (Copilot)",
    color: "#0f172a",
    bg: "#f8fafc",
    border: "#cbd5e1",
    models: ["gpt-4o", "gpt-4o-mini", "claude-3-5-sonnet", "mistral-large"],
    keyLabel: "GitHub Personal Access Token",
    keyPlaceholder: "github_pat_...",
    howToGet: [
      { step: "Sign in to GitHub", url: "https://github.com" },
      { step: "Go to Settings → Developer settings → Personal access tokens → Fine-grained tokens", url: "https://github.com/settings/tokens" },
      { step: "Generate a new token — no special scopes needed for GitHub Models" },
      { step: "Requires GitHub Copilot subscription or GitHub Models access (currently in beta)" },
    ],
  },
};

export default function AISettingsPage() {
  const { account, isAuthenticated } = useAuth();
  const { primaryColor, gradientColors } = useTheme();
  const { isAdmin } = useRole();

  const [providers, setProviders] = useState<AIProvider[]>([]);
  const [userKeys, setUserKeys] = useState<UserKey[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showAddModal, setShowAddModal] = useState(false);
  const [showPersonalModal, setShowPersonalModal] = useState(false);
  const [deletingId, setDeletingId] = useState<number | null>(null);
  const [deletingProvider, setDeletingProvider] = useState<string | null>(null);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const [pRes, kRes] = await Promise.all([
        msalFetch(`${API_BASE}/api/v1/ai/providers`),
        msalFetch(`${API_BASE}/api/v1/ai/my-keys`),
      ]);
      if (pRes.ok) setProviders((await pRes.json()).providers || []);
      if (kRes.ok) setUserKeys((await kRes.json()).keys || []);
    } catch {
      setError("Failed to load AI settings");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { if (isAuthenticated) void load(); }, [isAuthenticated]);

  const handleDeleteProvider = async (id: number) => {
    if (!confirm("Remove this AI provider key?")) return;
    setDeletingId(id);
    try {
      await msalFetch(`${API_BASE}/api/v1/ai/providers/${id}`, { method: "DELETE" });
      setProviders(prev => prev.filter(p => p.id !== id));
    } finally { setDeletingId(null); }
  };

  const handleDeletePersonalKey = async (provider: string) => {
    if (!confirm(`Remove your personal ${PROVIDER_META[provider]?.label ?? provider} key?`)) return;
    setDeletingProvider(provider);
    try {
      await msalFetch(`${API_BASE}/api/v1/ai/my-keys/${provider}`, { method: "DELETE" });
      setUserKeys(prev => prev.filter(k => k.provider !== provider));
    } finally { setDeletingProvider(null); }
  };

  return (
    <ListPageShell
      icon="fa-magic"
      title="AI Providers"
      subtitle="Configure AI API keys to power the LooMX AI Assistant."
      pills={!loading && !error ? [
        { label: `${providers.length} Global Key${providers.length !== 1 ? "s" : ""}`, icon: "fa-shield-alt" },
        { label: `${userKeys.length} Personal Key${userKeys.length !== 1 ? "s" : ""}`, icon: "fa-user", bg: "#f5f3ff", border: "#c4b5fd", color: "#7c3aed" },
      ] : []}
      action={
        <div style={{ display: "flex", gap: "0.5rem" }}>
          <Button variant="secondary" onClick={() => setShowPersonalModal(true)}>
            <i className="fas fa-key" /> Add My Key
          </Button>
          {isAdmin && (
            <Button onClick={() => setShowAddModal(true)}>
              <i className="fas fa-plus" /> Add Global Key
            </Button>
          )}
        </div>
      }
      loading={loading}
      loadingMessage="Loading AI settings"
      error={error}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: "1.25rem" }}>

        {/* Global keys (Admin managed) */}
        {isAdmin && (
          <div className="card">
            <div style={{ padding: "1rem 1.25rem", borderBottom: "1px solid #f1f5f9" }}>
              <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
                <i className="fas fa-shield-alt" style={{ color: primaryColor, fontSize: 14 }} />
                <strong style={{ fontSize: 14, color: "#0f172a" }}>Global Keys</strong>
                <span style={{ fontSize: 12, color: "#64748b" }}>— shared by all users</span>
              </div>
            </div>
            {providers.length === 0 ? (
              <div style={{ padding: "2rem", textAlign: "center", color: "#64748b", fontSize: 14 }}>
                No global keys configured yet.{" "}
                <button onClick={() => setShowAddModal(true)} style={{ color: primaryColor, background: "none", border: "none", cursor: "pointer", fontWeight: 600 }}>
                  Add one
                </button>
              </div>
            ) : (
              <div className="results-table-container">
                <table className="results-table">
                  <thead>
                    <tr>
                      <th><span className="column-header-label">Provider</span></th>
                      <th><span className="column-header-label">Label</span></th>
                      <th><span className="column-header-label">Model</span></th>
                      <th><span className="column-header-label">Status</span></th>
                      <th><span className="column-header-label">Added by</span></th>
                      <th><span className="column-header-label">Actions</span></th>
                    </tr>
                  </thead>
                  <tbody>
                    {providers.map(p => {
                      const meta = PROVIDER_META[p.provider];
                      return (
                        <tr key={p.id}>
                          <td>
                            <span style={{ display: "inline-flex", alignItems: "center", gap: "0.4rem", padding: "0.2rem 0.6rem", borderRadius: 6, fontSize: 12, fontWeight: 600, background: meta?.bg ?? "#f1f5f9", border: `1px solid ${meta?.border ?? "#e2e8f0"}`, color: meta?.color ?? "#374151" }}>
                              <span style={{ width: 14, height: 14, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0 }}>
                                {PROVIDER_LOGOS[p.provider] ?? <i className="fas fa-robot" />}
                              </span>
                              {meta?.label ?? p.provider}
                            </span>
                          </td>
                          <td className="muted" style={{ fontSize: 13 }}>{p.label}</td>
                          <td className="muted" style={{ fontSize: 12, fontFamily: "monospace" }}>{p.model}</td>
                          <td>
                            <span style={{ padding: "0.15rem 0.5rem", borderRadius: 6, fontSize: 12, fontWeight: 600, background: p.is_active ? "#d1fae5" : "#f3f4f6", color: p.is_active ? "#065f46" : "#6b7280" }}>
                              {p.is_active ? "Active" : "Inactive"}
                            </span>
                          </td>
                          <td className="muted" style={{ fontSize: 13 }}>{p.created_by || "—"}</td>
                          <td className="actions-cell">
                            <div className="row-actions">
                              <button type="button" className="action-icon-btn" title="Remove" onClick={() => void handleDeleteProvider(p.id)} disabled={deletingId === p.id}>
                                <i className={deletingId === p.id ? "fas fa-spinner fa-spin" : "fas fa-trash"} />
                              </button>
                            </div>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        )}

        {/* Personal keys */}
        <div className="card">
          <div style={{ padding: "1rem 1.25rem", borderBottom: "1px solid #f1f5f9" }}>
            <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
              <i className="fas fa-user" style={{ color: "#7c3aed", fontSize: 14 }} />
              <strong style={{ fontSize: 14, color: "#0f172a" }}>My Personal Keys</strong>
              <span style={{ fontSize: 12, color: "#64748b" }}>— override global keys, only used for your sessions</span>
            </div>
          </div>
          {userKeys.length === 0 ? (
            <div style={{ padding: "2rem", textAlign: "center", color: "#64748b", fontSize: 14 }}>
              No personal keys added yet.{" "}
              <button onClick={() => setShowPersonalModal(true)} style={{ color: primaryColor, background: "none", border: "none", cursor: "pointer", fontWeight: 600 }}>
                Add yours
              </button>
            </div>
          ) : (
            <div className="results-table-container">
              <table className="results-table">
                <thead>
                  <tr>
                    <th><span className="column-header-label">Provider</span></th>
                    <th><span className="column-header-label">Model</span></th>
                    <th><span className="column-header-label">Key</span></th>
                    <th><span className="column-header-label">Actions</span></th>
                  </tr>
                </thead>
                <tbody>
                  {userKeys.map(k => {
                    const meta = PROVIDER_META[k.provider];
                    return (
                      <tr key={k.provider}>
                        <td>
                          <span style={{ display: "inline-flex", alignItems: "center", gap: "0.4rem", padding: "0.2rem 0.6rem", borderRadius: 6, fontSize: 12, fontWeight: 600, background: meta?.bg ?? "#f1f5f9", border: `1px solid ${meta?.border ?? "#e2e8f0"}`, color: meta?.color ?? "#374151" }}>
                            <span style={{ width: 14, height: 14, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0 }}>
                              {PROVIDER_LOGOS[k.provider] ?? <i className="fas fa-robot" />}
                            </span>
                            {meta?.label ?? k.provider}
                          </span>
                        </td>
                        <td className="muted" style={{ fontSize: 12, fontFamily: "monospace" }}>{k.model || "—"}</td>
                        <td className="muted" style={{ fontSize: 12, fontFamily: "monospace" }}>{k.api_key_hint}</td>
                        <td className="actions-cell">
                          <div className="row-actions">
                            <button type="button" className="action-icon-btn" title="Remove" onClick={() => void handleDeletePersonalKey(k.provider)} disabled={deletingProvider === k.provider}>
                              <i className={deletingProvider === k.provider ? "fas fa-spinner fa-spin" : "fas fa-trash"} />
                            </button>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </div>

        {/* Info boxes */}
        <div style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
          <div style={{ padding: "1rem 1.25rem", borderRadius: 10, background: "#f8fafc", border: "1px solid #e2e8f0", fontSize: 13, color: "#475569", lineHeight: 1.6 }}>
            <strong style={{ color: "#0f172a" }}>Key priority:</strong> Your personal key always takes priority over a global key.
            If no key is configured, the AI Assistant will prompt you to add one.
            Keys are encrypted at rest using AES-256.
          </div>
          <div style={{ padding: "1rem 1.25rem", borderRadius: 10, background: "#fffbeb", border: "1px solid #fcd34d", fontSize: 13, color: "#78350f", lineHeight: 1.6, display: "flex", gap: "0.75rem", alignItems: "flex-start" }}>
            <i className="fas fa-shield-alt" style={{ color: "#d97706", marginTop: 2, flexShrink: 0 }} />
            <div>
              <strong style={{ color: "#92400e" }}>Future roadmap — Azure Key Vault integration.</strong>{" "}
              Keys are currently stored encrypted in the metadata database. For production enterprise deployments, we plan to support Azure Key Vault so no secrets ever touch the DB.
            </div>
          </div>
        </div>
      </div>

      {showAddModal && isAdmin && (
        <AddKeyModal
          isGlobal
          onClose={() => setShowAddModal(false)}
          onSuccess={() => { setShowAddModal(false); void load(); }}
        />
      )}
      {showPersonalModal && (
        <AddKeyModal
          isGlobal={false}
          onClose={() => setShowPersonalModal(false)}
          onSuccess={() => { setShowPersonalModal(false); void load(); }}
        />
      )}
    </ListPageShell>
  );
}

function AddKeyModal({ isGlobal, onClose, onSuccess }: { isGlobal: boolean; onClose: () => void; onSuccess: () => void }) {
  const { primaryColor } = useTheme();
  const [provider, setProvider] = useState("anthropic");
  const [label, setLabel] = useState("Default");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("claude-sonnet-4-6");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const meta = PROVIDER_META[provider];

  const handleProviderChange = (p: string) => {
    setProvider(p);
    setModel(PROVIDER_META[p]?.models[0] ?? "");
    setApiKey("");
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setSaving(true);
    setError(null);
    try {
      const body = isGlobal
        ? { provider, label, api_key: apiKey, model, is_active: true }
        : { provider, api_key: apiKey, model };
      const res = await msalFetch(
        `${API_BASE}/api/v1/ai/${isGlobal ? "providers" : "my-keys"}`,
        { method: isGlobal ? "POST" : "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }
      );
      if (!res.ok) {
        const d = await res.json();
        throw new Error(d?.detail?.message || "Failed to save key");
      }
      onSuccess();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to save");
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.45)", zIndex: 1000 }} onClick={onClose} />
      <div style={{ position: "fixed", top: "50%", left: "50%", transform: "translate(-50%,-50%)", background: "white", borderRadius: 14, boxShadow: "0 20px 60px rgba(0,0,0,0.25)", zIndex: 1001, width: "90%", maxWidth: 520, maxHeight: "90vh", overflowY: "auto" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "1.25rem 1.5rem", borderBottom: "1px solid #e5e7eb", position: "sticky", top: 0, background: "white", zIndex: 1 }}>
          <h2 style={{ margin: 0, fontSize: "1.1rem", fontWeight: 700 }}>
            {isGlobal ? "Add Global AI Key" : "Add My Personal Key"}
          </h2>
          <button onClick={onClose} style={{ background: "none", border: "none", fontSize: "1.25rem", cursor: "pointer", color: "#6b7280", borderRadius: 6, width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center" }}>
            <i className="fas fa-times" />
          </button>
        </div>
        <form onSubmit={handleSubmit}>
          <div style={{ padding: "1.25rem 1.5rem", display: "flex", flexDirection: "column", gap: "1rem" }}>
            {error && (
              <div style={{ padding: "0.75rem 1rem", borderRadius: 8, background: "#fef2f2", color: "#991b1b", border: "1px solid #fecaca", fontSize: 13 }}>
                <i className="fas fa-exclamation-circle" style={{ marginRight: 6 }} />{error}
              </div>
            )}

            {/* Provider selector — card style */}
            <div>
              <label className="chart-builder-label"><span>Provider</span></label>
              <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: "0.5rem", marginTop: "0.4rem" }}>
                {Object.entries(PROVIDER_META).map(([key, m]) => (
                  <button
                    key={key}
                    type="button"
                    onClick={() => handleProviderChange(key)}
                    style={{
                      padding: "0.65rem 0.5rem", borderRadius: 10, cursor: "pointer", textAlign: "center",
                      border: `2px solid ${provider === key ? m.color : "#e2e8f0"}`,
                      background: provider === key ? m.bg : "white",
                      transition: "all 0.15s", display: "flex", flexDirection: "column", alignItems: "center", gap: 4,
                    }}
                  >
                    <div style={{ opacity: provider === key ? 1 : 0.35, transition: "opacity 0.15s" }}>
                      {PROVIDER_LOGOS[key]}
                    </div>
                    <span style={{ fontSize: 11, fontWeight: 600, color: provider === key ? m.color : "#64748b", lineHeight: 1.3 }}>{m.label}</span>
                  </button>
                ))}
              </div>
            </div>

            {/* How to get this key */}
            {meta && (
              <div style={{ padding: "0.85rem 1rem", borderRadius: 9, background: `${meta.bg}`, border: `1px solid ${meta.border}`, fontSize: 13 }}>
                <div style={{ fontWeight: 600, color: meta.color, marginBottom: "0.5rem", display: "flex", alignItems: "center", gap: "0.4rem" }}>
                  <i className="fas fa-circle-info" />
                  How to get your {meta.label} key
                </div>
                <ol style={{ margin: 0, paddingLeft: "1.25rem", color: "#374151", lineHeight: 1.8 }}>
                  {meta.howToGet.map((s, i) => (
                    <li key={i}>
                      {s.url ? (
                        <a href={s.url} target="_blank" rel="noopener noreferrer" style={{ color: meta.color, fontWeight: 500 }}>
                          {s.step} <i className="fas fa-external-link-alt" style={{ fontSize: 10 }} />
                        </a>
                      ) : s.step}
                    </li>
                  ))}
                </ol>
              </div>
            )}

            {isGlobal && (
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>Label</span></label>
                <input className="chart-builder-input" value={label} onChange={e => setLabel(e.target.value)} placeholder="e.g. Team Key" required />
              </div>
            )}
            <div className="chart-builder-field">
              <label className="chart-builder-label"><span>{meta?.keyLabel ?? "API Key"}</span></label>
              <input className="chart-builder-input" type="password" value={apiKey} onChange={e => setApiKey(e.target.value)}
                placeholder={meta?.keyPlaceholder ?? "paste your key here"}
                autoComplete="new-password" required />
              <p style={{ margin: "0.3rem 0 0", fontSize: 11, color: "#94a3b8" }}>
                <i className="fas fa-lock" style={{ marginRight: 4 }} />Encrypted with AES-256 before storage. Never shown in plain text.
              </p>
            </div>
            <div className="chart-builder-field">
              <label className="chart-builder-label"><span>Model</span></label>
              <select className="chart-builder-select" value={model} onChange={e => setModel(e.target.value)}>
                {(meta?.models ?? []).map(m => <option key={m} value={m}>{m}</option>)}
              </select>
            </div>
          </div>
          <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.75rem", padding: "1rem 1.5rem", borderTop: "1px solid #e5e7eb", background: "#f9fafb", borderRadius: "0 0 14px 14px" }}>
            <Button type="button" variant="secondary" onClick={onClose} disabled={saving}>Cancel</Button>
            <Button type="submit" disabled={saving}>
              {saving ? <><i className="fas fa-spinner fa-spin" /> Saving…</> : <><i className="fas fa-save" /> Save Key</>}
            </Button>
          </div>
        </form>
      </div>
    </>
  );
}
