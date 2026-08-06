"use client";

import { useEffect, useRef, useState, KeyboardEvent } from "react";
import { useRouter } from "next/navigation";
import { useAuth } from "../auth/useAuth";
import { useSetup } from "../components/ClientLayout";
import { msalFetch } from "../utils/msalFetch";
import { KaveonMark } from "../components/KaveonMark";

// ── Types ──────────────────────────────────────────────────────────────────────

interface MetadataSummary {
  dataset_count: number;
}

interface DataSourceItem {
  id: number;
  database_name?: string;
  name?: string;
}

interface ActiveSourceItem {
  table_count?: number;
  tables?: unknown[];
}

interface PageData {
  datasetCount: number;
  sourceCount: number;
  tableCount: number;
  sourceNames: string[];
}

// ── Constants ──────────────────────────────────────────────────────────────────

const DEFAULT_SUGGESTIONS = [
  "What were total sales last month?",
  "Show revenue trend by quarter",
  "Top 10 customers by revenue",
  "Compare this month vs last month",
];

const EMPTY_SUGGESTIONS = [
  { label: "Connect Fabric SQL", href: "/data-sources" },
  { label: "Connect PostgreSQL", href: "/data-sources" },
  { label: "Connect Azure SQL", href: "/data-sources" },
];

// ── Component ──────────────────────────────────────────────────────────────────

export default function HomePage() {
  const router = useRouter();
  const { account } = useAuth();
  const { isSetupOk } = useSetup();

  const [query, setQuery] = useState("");
  const [data, setData] = useState<PageData | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const email = account?.email ?? "";
  const hasData = data !== null && data.sourceCount > 0;
  const isEmpty = data !== null && data.sourceCount === 0;

  // ── Data fetching ────────────────────────────────────────────────────────────

  useEffect(() => {
    if (!isSetupOk || !email) return;

    const headers = { "x-user-email": email };

    async function load() {
      try {
        const [summaryRes, listRes, activeRes] = await Promise.all([
          msalFetch("/api/v1/metadata/summary", { headers }),
          msalFetch("/api/v1/data-sources/list", { headers }),
          msalFetch("/api/v1/data-sources/active", { headers }),
        ]);

        const summary: MetadataSummary = summaryRes.ok ? await summaryRes.json() : { dataset_count: 0 };
        const listRaw = listRes.ok ? await listRes.json() : [];
        const list: DataSourceItem[] = Array.isArray(listRaw) ? listRaw : (listRaw.dataSources || listRaw.data_sources || listRaw.sources || []);
        const active: ActiveSourceItem = activeRes.ok ? await activeRes.json() : {};

        const tableCount =
          typeof active.table_count === "number"
            ? active.table_count
            : Array.isArray(active.tables)
            ? active.tables.length
            : 0;

        setData({
          datasetCount: summary.dataset_count ?? 0,
          sourceCount: Array.isArray(list) ? list.length : 0,
          tableCount,
          sourceNames: Array.isArray(list)
            ? list.map((s) => s.database_name ?? s.name ?? "Unknown")
            : [],
        });
      } catch {
        setData({ datasetCount: 0, sourceCount: 0, tableCount: 0, sourceNames: [] });
      }
    }

    load();
  }, [isSetupOk, email]);

  // ── Interaction ──────────────────────────────────────────────────────────────

  function submit() {
    const q = query.trim();
    if (!q) return;
    router.push(`/ai?q=${encodeURIComponent(q)}`);
  }

  function handleKey(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter") submit();
  }

  function applySuggestion(text: string) {
    setQuery(text);
    inputRef.current?.focus();
  }

  // ── Render ───────────────────────────────────────────────────────────────────

  const heroText = isEmpty ? "Connect your first data source." : "Your data has answers.";
  const placeholder = isEmpty
    ? "Set up a connection to get started..."
    : "Ask anything about your data…";

  const metaLine =
    data && !isEmpty
      ? `${data.sourceCount} source${data.sourceCount !== 1 ? "s" : ""} connected · ${data.tableCount} table${data.tableCount !== 1 ? "s" : ""} · ${data.datasetCount} dataset${data.datasetCount !== 1 ? "s" : ""}`
      : null;

  return (
    <div
      style={{
        position: "relative",
        minHeight: "100vh",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        background: "var(--bg-primary)",
        overflow: "hidden",
        paddingBottom: "3rem",
      }}
    >
      {/* Guardian O watermark — large, faded arc behind content */}
      <div
        aria-hidden
        style={{
          position: "absolute",
          top: "50%",
          left: "50%",
          transform: "translate(-50%, -50%)",
          pointerEvents: "none",
        }}
      >
        <KaveonMark size={280} opacity={0.04} useDirectColor />
      </div>

      {/* Center content */}
      <div
        style={{
          position: "relative",
          zIndex: 1,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: "1rem",
          width: "100%",
          maxWidth: "680px",
          padding: "0 1.5rem",
        }}
      >
        {/* Hero */}
        <h1
          style={{
            margin: 0,
            fontSize: "28px",
            fontWeight: 600,
            color: "var(--text-primary)",
            textAlign: "center",
            lineHeight: 1.25,
          }}
        >
          {heroText}
        </h1>

        {/* Meta line */}
        {metaLine && (
          <p
            style={{
              margin: 0,
              fontSize: "13px",
              color: "var(--text-muted)",
              textAlign: "center",
            }}
          >
            {metaLine}
          </p>
        )}

        {/* Chat input bar */}
        <div
          style={{
            width: "100%",
            maxWidth: "640px",
            display: "flex",
            alignItems: "center",
            gap: 0,
            background: "var(--bg-surface)",
            border: "1px solid var(--border)",
            borderRadius: "14px",
            padding: "10px 12px",
            marginTop: metaLine ? "0.5rem" : "1rem",
            boxShadow: "var(--shadow-md)",
          }}
        >
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={handleKey}
            placeholder={placeholder}
            aria-label="Ask a question about your data"
            style={{
              flex: 1,
              border: "none",
              outline: "none",
              background: "transparent",
              color: "var(--text-primary)",
              fontSize: "14px",
              lineHeight: 1.5,
            }}
          />
          <button
            onClick={submit}
            disabled={!query.trim()}
            aria-label="Send"
            style={{
              flexShrink: 0,
              width: "30px",
              height: "30px",
              borderRadius: "8px",
              border: "none",
              background: query.trim() ? "var(--accent)" : "var(--bg-hover)",
              color: query.trim() ? "#fff" : "var(--text-faint)",
              cursor: query.trim() ? "pointer" : "default",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: "16px",
              transition: "background 0.15s",
            }}
          >
            ↑
          </button>
        </div>

        {/* Suggestion chips */}
        <div
          style={{
            display: "flex",
            flexWrap: "wrap",
            gap: "0.5rem",
            justifyContent: "center",
            marginTop: "0.25rem",
          }}
        >
          {isEmpty
            ? EMPTY_SUGGESTIONS.map((s) => (
                <a
                  key={s.label}
                  href={s.href}
                  style={{
                    padding: "6px 14px",
                    borderRadius: "999px",
                    border: "1px solid var(--border)",
                    background: "var(--bg-surface)",
                    color: "var(--text-secondary)",
                    fontSize: "13px",
                    cursor: "pointer",
                    textDecoration: "none",
                    whiteSpace: "nowrap",
                    boxShadow: "var(--shadow-sm)",
                    transition: "background 0.12s, color 0.12s",
                  }}
                  onMouseEnter={(e) => {
                    (e.currentTarget as HTMLAnchorElement).style.background = "var(--bg-hover)";
                    (e.currentTarget as HTMLAnchorElement).style.color = "var(--text-primary)";
                  }}
                  onMouseLeave={(e) => {
                    (e.currentTarget as HTMLAnchorElement).style.background = "var(--bg-surface)";
                    (e.currentTarget as HTMLAnchorElement).style.color = "var(--text-secondary)";
                  }}
                >
                  {s.label}
                </a>
              ))
            : DEFAULT_SUGGESTIONS.map((s) => (
                <button
                  key={s}
                  onClick={() => applySuggestion(s)}
                  style={{
                    padding: "6px 14px",
                    borderRadius: "999px",
                    border: "1px solid var(--border)",
                    background: "var(--bg-surface)",
                    color: "var(--text-secondary)",
                    fontSize: "13px",
                    cursor: "pointer",
                    whiteSpace: "nowrap",
                    transition: "background 0.12s, color 0.12s",
                    boxShadow: "var(--shadow-sm)",
                  }}
                  onMouseEnter={(e) => {
                    (e.currentTarget as HTMLButtonElement).style.background = "var(--bg-hover)";
                    (e.currentTarget as HTMLButtonElement).style.color = "var(--text-primary)";
                  }}
                  onMouseLeave={(e) => {
                    (e.currentTarget as HTMLButtonElement).style.background = "var(--bg-surface)";
                    (e.currentTarget as HTMLButtonElement).style.color = "var(--text-secondary)";
                  }}
                >
                  {s}
                </button>
              ))}
        </div>

        {/* Connected sources */}
        {hasData && data.sourceNames.length > 0 && (
          <div
            style={{
              display: "flex",
              flexWrap: "wrap",
              gap: "0.5rem",
              justifyContent: "center",
              marginTop: "0.5rem",
            }}
          >
            {data.sourceNames.map((name) => (
              <span
                key={name}
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  gap: "6px",
                  fontSize: "12px",
                  color: "var(--text-muted)",
                }}
              >
                <span
                  aria-hidden
                  style={{
                    width: "6px",
                    height: "6px",
                    borderRadius: "50%",
                    background: "var(--success)",
                    flexShrink: 0,
                  }}
                />
                {name}
              </span>
            ))}
          </div>
        )}
      </div>

    </div>
  );
}
