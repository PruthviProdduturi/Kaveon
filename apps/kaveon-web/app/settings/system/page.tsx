"use client";

import React, { useEffect, useState, useRef } from "react";
import { useRouter } from "next/navigation";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useRole } from "../../../hooks/useRole";
import { useTheme } from "../../../contexts/ThemeContext";
import { Button } from "../../../components/Button";
import { ListPageShell } from "../../../components/ListPageShell";
import { SETUP_DB_ICONS } from "../../../components/DataSourceIcons";

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

// ── Page ───────────────────────────────────────────────────────────────────────

export default function SystemSettingsPage() {
  const router = useRouter();
  const { isAdmin, loading: roleLoading } = useRole();
  const { primaryColor, theme } = useTheme();

  const [metaConfig, setMetaConfig] = useState<MetadataConfig | null>(null);
  const [metaLoading, setMetaLoading] = useState(true);
  const [pingMs, setPingMs] = useState<number | null>(null);

  const [thumbStatus, setThumbStatus] = useState<
    { running: boolean; done: number; total: number; label: string } | null
  >(null);

  // Ping the API to measure latency
  useEffect(() => {
    let cancelled = false;
    const ping = async () => {
      try {
        const t0 = performance.now();
        const res = await msalFetch(`${API_BASE}/api/v1/admin/metadata`);
        if (res.ok && !cancelled) setPingMs(Math.round(performance.now() - t0));
      } catch { /* ignore */ }
    };
    ping();
    const iv = setInterval(ping, 30000);
    return () => { cancelled = true; clearInterval(iv); };
  }, []);

  const refreshAllThumbnails = async () => {
    if (thumbStatus?.running) return;
    setThumbStatus({ running: true, done: 0, total: 0, label: "Loading dashboards\u2026" });

    let dashboards: { id: string; name: string }[] = [];
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/dashboards`);
      const data = await res.json();
      const arr = Array.isArray(data) ? data : data.result || data.items || [];
      dashboards = arr.map((d: any) => ({ id: String(d.id), name: d.name || "Untitled" }));
    } catch {
      setThumbStatus({ running: false, done: 0, total: 0, label: "Failed to load dashboards" });
      setTimeout(() => setThumbStatus(null), 5000);
      return;
    }

    const themes: ("light" | "dark")[] = ["light", "dark"];
    const jobs = dashboards.flatMap((d) => themes.map((t) => ({ ...d, theme: t })));
    const total = jobs.length;
    if (total === 0) {
      setThumbStatus({ running: false, done: 0, total: 0, label: "No dashboards to refresh" });
      setTimeout(() => setThumbStatus(null), 5000);
      return;
    }

    const iframe = document.createElement("iframe");
    iframe.style.cssText =
      "position:fixed;left:-9999px;top:0;width:1280px;height:800px;border:0;visibility:hidden;";
    document.body.appendChild(iframe);

    let resolveDone: (() => void) | null = null;
    const onMsg = (e: MessageEvent) => {
      if (e.data && e.data.type === "kaveon-thumb-done") resolveDone?.();
    };
    window.addEventListener("message", onMsg);

    try {
      for (let i = 0; i < jobs.length; i++) {
        const job = jobs[i];
        setThumbStatus({ running: true, done: i, total, label: `${job.name} \u00b7 ${job.theme}` });
        await new Promise<void>((resolve) => {
          const finish = () => { clearTimeout(timer); resolveDone = null; resolve(); };
          const timer = setTimeout(finish, 25000);
          resolveDone = finish;
          iframe.src = `/dashboards/${job.id}/view?capture=1&forceTheme=${job.theme}`;
        });
      }
      setThumbStatus({ running: false, done: total, total, label: "All thumbnails refreshed" });
    } finally {
      window.removeEventListener("message", onMsg);
      iframe.remove();
      setTimeout(() => setThumbStatus(null), 6000);
    }
  };

  useEffect(() => {
    if (!roleLoading && !isAdmin) router.replace("/");
  }, [roleLoading, isAdmin, router]);

  useEffect(() => {
    if (!roleLoading && isAdmin) {
      setMetaLoading(true);
      msalFetch(`${API_BASE}/api/v1/admin/metadata`)
        .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
        .then((d: MetadataConfig) => { setMetaConfig(d); setMetaLoading(false); })
        .catch(() => { setMetaConfig(null); setMetaLoading(false); });
    }
  }, [roleLoading, isAdmin]);

  if (!isAdmin && !roleLoading) return null;

  const metaIconMeta = SETUP_DB_ICONS[metaConfig?.db_type ?? "fabric_sql"];
  const metaConfigured = !!metaConfig?.ui_configured;

  // Build connection detail items
  const connDetails: { label: string; value: string; icon: string }[] = [];
  if (metaConfigured) {
    if (metaConfig!.database)  connDetails.push({ label: "Database", value: metaConfig!.database, icon: "fa-database" });
    if (metaConfig!.host)      connDetails.push({ label: "Host", value: metaConfig!.host, icon: "fa-server" });
    if (metaConfig!.endpoint)  connDetails.push({ label: "Endpoint", value: metaConfig!.endpoint, icon: "fa-globe" });
    if (metaConfig!.port)      connDetails.push({ label: "Port", value: metaConfig!.port, icon: "fa-hashtag" });
    connDetails.push({ label: "Engine", value: metaConfig!.label, icon: "fa-gear" });
  }

  const isDark = theme === "dark";
  const green = "#22c55e";
  const greenBg = isDark ? "rgba(34,197,94,0.08)" : "rgba(34,197,94,0.06)";
  const greenBorder = isDark ? "rgba(34,197,94,0.2)" : "rgba(34,197,94,0.15)";

  return (
    <ListPageShell
      icon="fa-sliders"
      title="System"
      subtitle="Core infrastructure and maintenance tools."
      loading={roleLoading || metaLoading}
    >
      <style>{`
        @keyframes kv-pulse { 0%,100%{opacity:1} 50%{opacity:0.4} }
        @keyframes kv-slide { from{transform:translateX(-100%)} to{transform:translateX(100%)} }
      `}</style>

      <div style={{ display: "flex", flexDirection: "column", gap: "1.25rem" }}>

        {/* ── Database Connection ──────────────────────────────────────────── */}
        <div className="card" style={{
          overflow: "hidden",
          border: `1px solid ${metaConfigured ? greenBorder : "var(--border)"}`,
        }}>
          {/* Accent bar */}
          <div style={{
            height: 3,
            background: metaConfigured
              ? `linear-gradient(90deg, ${green}, ${primaryColor})`
              : "var(--border)",
          }} />

          <div style={{ padding: "1.5rem" }}>
            {metaLoading ? (
              <div style={{ display: "flex", alignItems: "center", gap: 10, color: "var(--text-muted)", fontSize: 13 }}>
                <i className="fas fa-spinner fa-spin" /> Connecting to metadata database...
              </div>
            ) : !metaConfigured ? (
              <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
                <div style={{
                  width: 52, height: 52, borderRadius: 14, flexShrink: 0,
                  background: "color-mix(in srgb, var(--warning) 10%, var(--bg-surface))",
                  border: "1.5px solid color-mix(in srgb, var(--warning) 30%, transparent)",
                  display: "flex", alignItems: "center", justifyContent: "center",
                }}>
                  <i className="fas fa-database" style={{ fontSize: 20, color: "var(--warning)" }} />
                </div>
                <div>
                  <div style={{ fontSize: 15, fontWeight: 700, color: "var(--text-primary)", marginBottom: 4 }}>No Database Connected</div>
                  <div style={{ fontSize: 13, color: "var(--text-muted)", lineHeight: 1.5 }}>
                    Run the setup wizard to configure the metadata database.
                  </div>
                </div>
              </div>
            ) : (
              <>
                {/* Top row: engine + status */}
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: "1.25rem" }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
                    <div style={{
                      width: 52, height: 52, borderRadius: 14, flexShrink: 0,
                      background: metaIconMeta?.bg ?? "var(--bg-primary)",
                      border: `1.5px solid ${metaIconMeta?.border ?? "var(--border)"}`,
                      display: "flex", alignItems: "center", justifyContent: "center",
                      boxShadow: `0 2px 8px ${metaIconMeta?.border ?? "var(--border)"}40`,
                    }}>
                      {metaIconMeta?.icon}
                    </div>
                    <div>
                      <div style={{ fontSize: 16, fontWeight: 700, color: "var(--text-primary)", letterSpacing: "-0.01em" }}>
                        {metaConfig!.label}
                      </div>
                      <div style={{ fontSize: 12, color: "var(--text-muted)", fontFamily: "var(--font-mono, monospace)", marginTop: 2 }}>
                        {metaConfig!.host
                          ? `${metaConfig!.host}${metaConfig!.port ? `:${metaConfig!.port}` : ""}`
                          : metaConfig!.endpoint || "\u2014"}
                      </div>
                    </div>
                  </div>

                  {/* Live status badge */}
                  <div style={{
                    display: "flex", alignItems: "center", gap: 8,
                    padding: "6px 14px", borderRadius: 20,
                    background: greenBg,
                    border: `1px solid ${greenBorder}`,
                  }}>
                    <span style={{
                      width: 8, height: 8, borderRadius: "50%", background: green,
                      boxShadow: `0 0 6px ${green}80`,
                      animation: "kv-pulse 2s ease-in-out infinite",
                    }} />
                    <span style={{ fontSize: 12, fontWeight: 600, color: green }}>Connected</span>
                    {pingMs !== null && (
                      <span style={{ fontSize: 11, color: "var(--text-muted)", fontFamily: "var(--font-mono, monospace)" }}>
                        {pingMs}ms
                      </span>
                    )}
                  </div>
                </div>

                {/* Connection details grid */}
                <div style={{
                  display: "grid",
                  gridTemplateColumns: `repeat(${Math.min(connDetails.length, 3)}, 1fr)`,
                  gap: 1,
                  borderRadius: 12,
                  overflow: "hidden",
                  border: "1px solid var(--border)",
                }}>
                  {connDetails.map(({ label, value, icon }) => (
                    <div key={label} style={{
                      padding: "14px 16px",
                      background: "var(--bg-primary)",
                    }}>
                      <div style={{
                        display: "flex", alignItems: "center", gap: 6,
                        fontSize: 10, fontWeight: 700, color: "var(--text-muted)",
                        textTransform: "uppercase", letterSpacing: "0.08em",
                        marginBottom: 6,
                      }}>
                        <i className={`fas ${icon}`} style={{ fontSize: 9, opacity: 0.5 }} />
                        {label}
                      </div>
                      <div style={{
                        fontSize: 13, fontWeight: 500, color: "var(--text-secondary)",
                        fontFamily: "var(--font-mono, monospace)",
                        overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
                      }}>
                        {value}
                      </div>
                    </div>
                  ))}
                </div>
              </>
            )}
          </div>
        </div>

        {/* ── Dashboard Thumbnails ─────────────────────────────────────────── */}
        <div className="card" style={{ overflow: "hidden" }}>
          <div style={{ height: 3, background: `linear-gradient(90deg, ${primaryColor}, ${primaryColor}60)` }} />
          <div style={{ padding: "1.25rem 1.5rem" }}>
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
                <div style={{
                  width: 38, height: 38, borderRadius: 10, flexShrink: 0,
                  background: `${primaryColor}14`,
                  border: `1px solid ${primaryColor}28`,
                  display: "flex", alignItems: "center", justifyContent: "center",
                }}>
                  <i className="fas fa-images" style={{ fontSize: 15, color: primaryColor }} />
                </div>
                <div>
                  <div style={{ fontSize: 14, fontWeight: 700, color: "var(--text-primary)" }}>
                    Dashboard Thumbnails
                  </div>
                  <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 1 }}>
                    Re-render previews for both light and dark themes
                  </div>
                </div>
              </div>

              <Button onClick={refreshAllThumbnails} disabled={!!thumbStatus?.running}>
                {thumbStatus?.running ? (
                  <><i className="fas fa-spinner fa-spin" /> Refreshing&hellip;</>
                ) : (
                  <><i className="fas fa-arrows-rotate" /> Refresh all</>
                )}
              </Button>
            </div>

            {/* Progress — only visible during/after refresh */}
            {thumbStatus && (
              <div style={{
                marginTop: "1rem", padding: "10px 14px", borderRadius: 10,
                background: thumbStatus.running ? `${primaryColor}08` : greenBg,
                border: `1px solid ${thumbStatus.running ? `${primaryColor}20` : greenBorder}`,
              }}>
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                    {thumbStatus.running ? (
                      <i className="fas fa-spinner fa-spin" style={{ fontSize: 11, color: primaryColor }} />
                    ) : (
                      <i className="fas fa-check-circle" style={{ fontSize: 11, color: green }} />
                    )}
                    <span style={{ fontSize: 12, fontWeight: 600, color: thumbStatus.running ? primaryColor : green }}>
                      {thumbStatus.label}
                    </span>
                  </div>
                  {thumbStatus.total > 0 && (
                    <span style={{ fontSize: 11, color: "var(--text-muted)", fontFamily: "var(--font-mono, monospace)" }}>
                      {thumbStatus.done}/{thumbStatus.total}
                    </span>
                  )}
                </div>
                <div style={{
                  height: 4, borderRadius: 999, overflow: "hidden",
                  background: isDark ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)",
                }}>
                  <div style={{
                    height: "100%", borderRadius: 999,
                    width: thumbStatus.total > 0 ? `${Math.round((thumbStatus.done / thumbStatus.total) * 100)}%` : "0%",
                    background: thumbStatus.running
                      ? `linear-gradient(90deg, ${primaryColor}, ${primaryColor}80)`
                      : green,
                    transition: "width 0.3s ease",
                  }} />
                </div>
              </div>
            )}
          </div>
        </div>

        {/* ── Azure Key Vault — coming soon ────────────────────────────────── */}
        <div className="card" style={{
          overflow: "hidden",
          border: "1.5px dashed var(--border)",
          opacity: 0.85,
        }}>
          <div style={{
            height: 3,
            background: `linear-gradient(90deg, #0078d4, #50e6ff)`,
            opacity: 0.4,
          }} />
          <div style={{ padding: "1.5rem" }}>
            <div style={{ display: "flex", alignItems: "center", gap: 14, marginBottom: "1.25rem" }}>
              <div style={{
                width: 44, height: 44, borderRadius: 12, flexShrink: 0,
                background: isDark ? "rgba(0,120,212,0.1)" : "rgba(0,120,212,0.06)",
                border: `1.5px solid ${isDark ? "rgba(0,120,212,0.2)" : "rgba(0,120,212,0.12)"}`,
                display: "flex", alignItems: "center", justifyContent: "center",
              }}>
                <AzureKeyVaultIcon size={24} />
              </div>
              <div style={{ flex: 1 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                  <span style={{ fontSize: 15, fontWeight: 700, color: "var(--text-muted)" }}>Azure Key Vault</span>
                  <span style={{
                    fontSize: 10, fontWeight: 700, letterSpacing: "0.06em", textTransform: "uppercase",
                    padding: "3px 10px", borderRadius: 20,
                    background: `${primaryColor}12`, border: `1px solid ${primaryColor}25`,
                    color: primaryColor,
                  }}>
                    Coming soon
                  </span>
                </div>
                <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 3 }}>
                  Secure secret management for connection strings and credentials
                </div>
              </div>
            </div>

            <div style={{
              display: "grid", gridTemplateColumns: "repeat(2, 1fr)", gap: 10,
            }}>
              {[
                { icon: "fa-key", title: "Secret References", desc: "Connection strings stored as Key Vault references, never as plain text" },
                { icon: "fa-shield-halved", title: "Managed Identity", desc: "No client secrets or passwords to rotate or manage" },
                { icon: "fa-lock", title: "Centralised Vault", desc: "Single vault for all data source credentials across environments" },
                { icon: "fa-clock-rotate-left", title: "Audit Trail", desc: "Full access logs via Azure Monitor and Key Vault diagnostics" },
              ].map(({ icon, title, desc }) => (
                <div key={title} style={{
                  padding: "14px 16px", borderRadius: 10,
                  background: "var(--bg-primary)", border: "1px solid var(--border)",
                }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 6 }}>
                    <i className={`fas ${icon}`} style={{ fontSize: 12, color: "#0078d4", opacity: 0.6 }} />
                    <span style={{ fontSize: 12, fontWeight: 700, color: "var(--text-secondary)" }}>{title}</span>
                  </div>
                  <div style={{ fontSize: 11.5, color: "var(--text-muted)", lineHeight: 1.5 }}>{desc}</div>
                </div>
              ))}
            </div>
          </div>
        </div>

      </div>
    </ListPageShell>
  );
}

// ── Icons ──────────────────────────────────────────────────────────────────────

function AzureKeyVaultIcon({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 18 18" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M9 1L2 4v5c0 3.55 2.96 6.87 7 7.93C13.04 15.87 16 12.55 16 9V4L9 1z" fill="#50e6ff" opacity="0.25" />
      <path d="M9 1L2 4v5c0 3.55 2.96 6.87 7 7.93C13.04 15.87 16 12.55 16 9V4L9 1z" stroke="#0078d4" strokeWidth="1" fill="none" />
      <circle cx="8" cy="8" r="2.2" stroke="#0078d4" strokeWidth="1.1" fill="none" />
      <line x1="9.9" y1="8" x2="13.5" y2="8" stroke="#0078d4" strokeWidth="1.1" strokeLinecap="round" />
      <line x1="12.5" y1="8" x2="12.5" y2="9.4" stroke="#0078d4" strokeWidth="1" strokeLinecap="round" />
      <line x1="11" y1="8" x2="11" y2="9.1" stroke="#0078d4" strokeWidth="1" strokeLinecap="round" />
    </svg>
  );
}
