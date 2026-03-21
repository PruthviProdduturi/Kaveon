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

const PROVIDER_META: Record<string, {
  label: string; icon: string; color: string; bg: string; border: string;
  models: string[];
  keyLabel: string;
  keyPlaceholder: string;
  howToGet: { step: string; url?: string }[];
}> = {
  anthropic: {
    label: "Anthropic (Claude)",
    icon: "fa-robot",
    color: "#7c3aed",
    bg: "#f5f3ff",
    border: "#c4b5fd",
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
    icon: "fa-brain",
    color: "#059669",
    bg: "#ecfdf5",
    border: "#6ee7b7",
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
    icon: "fa-github",
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
      subtitle="Configure AI API keys to power the LoomX AI Assistant."
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
                            <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
                              <span style={{ padding: "0.2rem 0.6rem", borderRadius: 6, fontSize: 12, fontWeight: 600, background: meta?.bg ?? "#f1f5f9", border: `1px solid ${meta?.border ?? "#e2e8f0"}`, color: meta?.color ?? "#374151" }}>
                                <i className={`fas ${meta?.icon ?? "fa-robot"}`} style={{ marginRight: 4 }} />
                                {meta?.label ?? p.provider}
                              </span>
                            </div>
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
                          <span style={{ padding: "0.2rem 0.6rem", borderRadius: 6, fontSize: 12, fontWeight: 600, background: meta?.bg ?? "#f1f5f9", border: `1px solid ${meta?.border ?? "#e2e8f0"}`, color: meta?.color ?? "#374151" }}>
                            <i className={`fas ${meta?.icon ?? "fa-robot"}`} style={{ marginRight: 4 }} />
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
                      transition: "all 0.15s",
                    }}
                  >
                    <i className={`fab ${m.icon}`} style={{ fontSize: 18, color: provider === key ? m.color : "#94a3b8", display: "block", marginBottom: 4 }} />
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
