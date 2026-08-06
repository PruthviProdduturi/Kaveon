"use client";

import React, { useEffect, useRef, useState } from "react";
import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";
import { useAuth } from "../../auth/useAuth";
import { useTheme } from "../../contexts/ThemeContext";
import { useRouter } from "next/navigation";
import { nlToSql, DatasetSchema, NlToSqlResult } from "../../utils/nlToSql";
import { InlineChart } from "../../components/chat/InlineChart";

interface ChartData {
  rows: (string | number | null)[][];
  columns: string[];
  chartType: "bar" | "line" | "pie" | "kpi" | "table";
  xAxis: string | null;
  yAxis: string | null;
  title: string;
  sql: string;
}

interface Message {
  role: "user" | "assistant";
  content: string;
  loading?: boolean;
  chart?: ChartData;
}

interface SourceOption {
  id: number;
  name: string;
  type: string;
  database_name: string;
}

interface DatasetOption {
  id: number;
  name: string;
  database_name?: string;
}

const QUICK_ACTIONS = [
  { label: "Generate SQL", icon: "fa-code", prompt: "Write a SQL query that " },
  { label: "Explain Query", icon: "fa-search", prompt: "Explain what this SQL query does:\n\n```sql\n\n```" },
  { label: "Suggest Chart", icon: "fa-chart-bar", prompt: "What chart type best visualises " },
  { label: "Design Dashboard", icon: "fa-tachometer-alt", prompt: "Suggest a dashboard layout for " },
  { label: "Optimise Query", icon: "fa-bolt", prompt: "Optimise this SQL query for performance:\n\n```sql\n\n```" },
  { label: "Explore Dataset", icon: "fa-database", prompt: "What interesting insights can I find in this dataset? Give me 5 SQL queries to explore it." },
];

// ── Markdown-lite renderer ────────────────────────────────────────────────────
function renderMessage(content: string, primaryColor: string, onCopySql: (sql: string) => void, onOpenLab: (sql: string) => void, onCreateChart: (json: string) => void) {
  const parts: React.ReactNode[] = [];
  let remaining = content;
  let key = 0;

  while (remaining.length > 0) {
    // Match code blocks: ```sql, ```chart, ```dashboard, or generic ```
    const fence = remaining.match(/```(sql|chart|dashboard|json|)?\n?([\s\S]*?)```/);
    if (!fence || fence.index === undefined) {
      // No more code blocks — render rest as text with basic markdown
      parts.push(<span key={key++} dangerouslySetInnerHTML={{ __html: toHtml(remaining) }} />);
      break;
    }

    // Text before the fence
    if (fence.index > 0) {
      parts.push(<span key={key++} dangerouslySetInnerHTML={{ __html: toHtml(remaining.slice(0, fence.index)) }} />);
    }

    const lang = (fence[1] || "").toLowerCase();
    const code = fence[2].trim();

    if (lang === "chart") {
      // Chart suggestion block
      parts.push(
        <ChartBlock key={key++} json={code} primaryColor={primaryColor} onCreate={() => onCreateChart(code)} />
      );
    } else if (lang === "dashboard") {
      parts.push(
        <DashboardBlock key={key++} json={code} primaryColor={primaryColor} />
      );
    } else {
      // SQL or generic code block
      parts.push(
        <CodeBlock key={key++} code={code} lang={lang} primaryColor={primaryColor}
          onCopy={() => onCopySql(code)}
          onOpenLab={lang === "sql" ? () => onOpenLab(code) : undefined}
        />
      );
    }

    remaining = remaining.slice(fence.index + fence[0].length);
    key++;
  }

  return <>{parts}</>;
}

function toHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/\*\*(.*?)\*\*/g, "<strong>$1</strong>")
    .replace(/`([^`]+)`/g, '<code style="background:#f1f5f9;padding:2px 5px;border-radius:4px;font-size:0.88em;font-family:monospace">$1</code>')
    .replace(/\n/g, "<br />");
}

function CodeBlock({ code, lang, primaryColor, onCopy, onOpenLab }: {
  code: string; lang: string; primaryColor: string;
  onCopy: () => void; onOpenLab?: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const handleCopy = () => {
    void navigator.clipboard.writeText(code);
    setCopied(true);
    onCopy();
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div style={{ margin: "0.75rem 0", borderRadius: 10, overflow: "hidden", border: "1px solid #e2e8f0" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "0.4rem 0.75rem", background: "#1e293b", borderBottom: "1px solid #334155" }}>
        <span style={{ fontSize: 11, color: "#94a3b8", fontFamily: "monospace", textTransform: "uppercase" }}>{lang || "code"}</span>
        <div style={{ display: "flex", gap: "0.4rem" }}>
          <button type="button" onClick={handleCopy} style={{ background: "none", border: "none", color: copied ? "#6ee7b7" : "#94a3b8", cursor: "pointer", fontSize: 12, padding: "0.2rem 0.5rem", borderRadius: 4 }}>
            <i className={copied ? "fas fa-check" : "fas fa-copy"} style={{ marginRight: 4 }} />{copied ? "Copied" : "Copy"}
          </button>
          {onOpenLab && (
            <button type="button" onClick={onOpenLab} style={{ background: "none", border: "none", color: "#7dd3fc", cursor: "pointer", fontSize: 12, padding: "0.2rem 0.5rem", borderRadius: 4 }}>
              <i className="fas fa-flask" style={{ marginRight: 4 }} />Open in Lab
            </button>
          )}
        </div>
      </div>
      <pre style={{ margin: 0, padding: "0.85rem 1rem", background: "#0f172a", color: "#e2e8f0", fontSize: 13, fontFamily: "monospace", overflowX: "auto", lineHeight: 1.6, whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
        {code}
      </pre>
    </div>
  );
}

function ChartBlock({ json, primaryColor, onCreate }: { json: string; primaryColor: string; onCreate: () => void }) {
  let parsed: Record<string, string> = {};
  try { parsed = JSON.parse(json); } catch { return null; }
  return (
    <div style={{ margin: "0.75rem 0", padding: "1rem", borderRadius: 10, background: `${primaryColor}08`, border: `1px solid ${primaryColor}25` }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: "0.5rem" }}>
        <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
          <i className="fas fa-chart-bar" style={{ color: primaryColor }} />
          <strong style={{ fontSize: 13, color: "#0f172a" }}>Chart Suggestion</strong>
        </div>
        <button type="button" onClick={onCreate} style={{ background: primaryColor, color: "white", border: "none", borderRadius: 6, padding: "0.3rem 0.75rem", fontSize: 12, fontWeight: 600, cursor: "pointer" }}>
          <i className="fas fa-plus" style={{ marginRight: 4 }} />Create Chart
        </button>
      </div>
      <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem", fontSize: 12 }}>
        {Object.entries(parsed).map(([k, v]) => (
          <span key={k} style={{ background: "white", border: "1px solid #e2e8f0", borderRadius: 6, padding: "0.2rem 0.5rem", color: "#374151" }}>
            <strong>{k}:</strong> {String(v)}
          </span>
        ))}
      </div>
    </div>
  );
}

function DashboardBlock({ json, primaryColor }: { json: string; primaryColor: string }) {
  let parsed: { title?: string; description?: string; charts?: Array<Record<string, string>> } = {};
  try { parsed = JSON.parse(json); } catch { return null; }
  return (
    <div style={{ margin: "0.75rem 0", padding: "1rem", borderRadius: 10, background: "#f8fafc", border: "1px solid #e2e8f0" }}>
      <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", marginBottom: "0.5rem" }}>
        <i className="fas fa-tachometer-alt" style={{ color: primaryColor }} />
        <strong style={{ fontSize: 13, color: "#0f172a" }}>{parsed.title ?? "Dashboard Suggestion"}</strong>
      </div>
      {parsed.description && <p style={{ margin: "0 0 0.5rem", fontSize: 13, color: "#64748b" }}>{parsed.description}</p>}
      {(parsed.charts || []).map((c, i) => (
        <div key={i} style={{ padding: "0.4rem 0.6rem", borderRadius: 6, background: "white", border: "1px solid #e2e8f0", marginBottom: "0.35rem", fontSize: 12, color: "#374151" }}>
          <strong>{c.title ?? c.chart_type}</strong>
          {c.x_axis && <span className="muted"> · {c.x_axis} × {c.y_axis}</span>}
        </div>
      ))}
    </div>
  );
}

// ── Main page ─────────────────────────────────────────────────────────────────

export default function AIPage() {
  const { isAuthenticated, account } = useAuth();
  const { primaryColor, gradientColors } = useTheme();
  const router = useRouter();

  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [sources, setSources] = useState<SourceOption[]>([]);
  const [datasets, setDatasets] = useState<DatasetOption[]>([]);
  const [selectedSource, setSelectedSource] = useState<SourceOption | null>(null);
  const [selectedDataset, setSelectedDataset] = useState<number | null>(null);
  const [sqlContext, setSqlContext] = useState("");
  const [showSqlContext, setShowSqlContext] = useState(false);
  const [sending, setSending] = useState(false);
  const [noKey, setNoKey] = useState(false);
  const [datasetSchema, setDatasetSchema] = useState<DatasetSchema | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // Fetch dataset schema when a dataset is selected (for NL→SQL parser)
  useEffect(() => {
    if (!selectedDataset) { setDatasetSchema(null); return; }
    msalFetch(`${API_BASE}/api/v1/datasets/${selectedDataset}`)
      .then(r => r.ok ? r.json() : null)
      .then(d => {
        if (!d) { setDatasetSchema(null); return; }
        const cols = (d.columns || []).map((c: any) => ({
          name: c.column_name || c.name,
          type: c.data_type?.includes("int") || c.data_type?.includes("float") || c.data_type?.includes("decimal") || c.data_type?.includes("numeric") || c.data_type?.includes("money") || c.data_type?.includes("real") ? "number"
            : c.data_type?.includes("date") || c.data_type?.includes("time") ? "date"
            : "string",
          description: c.description || "",
        }));
        const metrics = (d.metrics || []).map((m: any) => ({
          name: m.name || m.metric_name,
          expression: m.sql_expression || m.expression || `SUM(${m.name})`,
          description: m.description || "",
        }));
        const table = d.fact_table
          ? (d.schema_name ? `${d.schema_name}.${d.fact_table}` : d.fact_table)
          : d.table_name || "data";
        setDatasetSchema({ tableName: table, columns: cols, metrics: metrics });
      })
      .catch(() => setDatasetSchema(null));
  }, [selectedDataset]);

  // Load data sources + datasets for context pickers
  useEffect(() => {
    if (!isAuthenticated) return;
    const userEmail = account?.email || account?.username || "";
    Promise.all([
      msalFetch(`${API_BASE}/api/v1/data-sources${userEmail ? `?email=${encodeURIComponent(userEmail)}` : ""}`),
      msalFetch(`${API_BASE}/api/v1/datasets/summary`),
    ]).then(async ([sRes, dRes]) => {
      if (sRes.ok) {
        const sd = await sRes.json();
        setSources((sd.dataSources || [])
          .filter((s: SourceOption & { is_active?: boolean }) => s.is_active !== false)
          .map((s: SourceOption) => ({ id: s.id, name: s.name, type: s.type, database_name: s.database_name })));
      }
      if (dRes.ok) {
        const dd = await dRes.json();
        setDatasets((dd.recent || []).map((ds: { id: number; dataset_name?: string; name?: string; database_name?: string }) => ({
          id: ds.id,
          name: ds.dataset_name || ds.name || `Dataset ${ds.id}`,
          database_name: ds.database_name,
        })));
      }
    }).catch(() => {});
  }, [isAuthenticated]);

  // Auto-scroll to latest message
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const sendMessage = async (text: string) => {
    if (!text.trim() || sending) return;
    const userMsg: Message = { role: "user", content: text.trim() };
    const loadingMsg: Message = { role: "assistant", content: "", loading: true };
    setMessages(prev => [...prev, userMsg, loadingMsg]);
    setInput("");
    setSending(true);
    setNoKey(false);

    try {
      // ── Try NL→SQL parser first (no API key needed) ────────────────────
      if (datasetSchema) {
        const parsed = nlToSql(text.trim(), datasetSchema);
        if (parsed && parsed.confidence >= 0.5) {
          // Execute the generated SQL
          const sourceId = selectedSource?.id;
          const execRes = await msalFetch(`${API_BASE}/api/v1/sql/execute`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              sql: parsed.sql,
              data_source_id: sourceId,
              database: selectedSource?.database_name,
            }),
          });

          if (execRes.ok) {
            const execData = await execRes.json();
            const rows = execData.rows || execData.data || [];
            const columns = execData.columns || execData.column_names || [];

            if (rows.length > 0) {
              setMessages(prev => [...prev.slice(0, -1), {
                role: "assistant",
                content: `Here's what I found for "${text.trim()}"`,
                chart: {
                  rows,
                  columns,
                  chartType: parsed.chartType,
                  xAxis: parsed.xAxis,
                  yAxis: parsed.yAxis,
                  title: parsed.title,
                  sql: parsed.sql,
                },
              }]);
              return;
            }
            // No rows — fall through to text response
            setMessages(prev => [...prev.slice(0, -1), {
              role: "assistant",
              content: `I ran the query but it returned no results.\n\n\`\`\`sql\n${parsed.sql}\n\`\`\`\n\nTry rephrasing or check if the data exists.`,
            }]);
            return;
          }
          // SQL execution failed — fall through to AI API
        }
      }

      // ── Fall back to AI API ────────────────────────────────────────────
      const history = [...messages, userMsg].map(m => ({ role: m.role, content: m.content }));
      const ctx: Record<string, unknown> = {};
      if (selectedSource) ctx.data_source_id = selectedSource.id;
      if (selectedDataset) ctx.dataset_id = selectedDataset;
      if (sqlContext.trim()) ctx.sql = sqlContext.trim();
      const res = await msalFetch(`${API_BASE}/api/v1/ai/chat`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ messages: history, context: Object.keys(ctx).length ? ctx : undefined }),
      });
      if (res.status === 400) {
        const d = await res.json();
        if (d?.detail?.code === "ai_error" || d?.detail?.code === "invalid_api_key") {
          // No API key — show a helpful message instead of an error
          if (datasetSchema) {
            setMessages(prev => [...prev.slice(0, -1), {
              role: "assistant",
              content: "I couldn't understand that query well enough to generate SQL automatically. Try something like:\n\n• \"Show [metric] by [column]\"\n• \"Top 10 [column] by [metric]\"\n• \"Total [metric]\"\n• \"Trend of [metric] over time\"",
            }]);
          } else {
            setMessages(prev => [...prev.slice(0, -1), {
              role: "assistant",
              content: "Select a dataset above so I can understand your data schema, then ask me a question like \"show revenue by region\" or \"top 10 customers by sales\".",
            }]);
          }
          return;
        }
        throw new Error(d?.detail?.message || "Failed");
      }
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      setMessages(prev => [...prev.slice(0, -1), { role: "assistant", content: data.reply }]);
    } catch (e) {
      setMessages(prev => [...prev.slice(0, -1), {
        role: "assistant",
        content: `Something went wrong. ${e instanceof Error ? e.message : "Please try again."}`,
      }]);
    } finally {
      setSending(false);
      setTimeout(() => inputRef.current?.focus(), 100);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void sendMessage(input);
    }
  };

  const handleCopySql = (sql: string) => void navigator.clipboard.writeText(sql);
  const handleOpenLab = (sql: string) => void router.push(`/lab?sql=${encodeURIComponent(sql)}`);
  const handleCreateChart = (json: string) => {
    try {
      const c = JSON.parse(json);
      void router.push(`/charts/new?ai=${encodeURIComponent(JSON.stringify(c))}`);
    } catch {}
  };

  return (
    <div className="animate-fade-in" style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0, gap: 0 }}>

      {/* Header */}
      <div style={{
        display: "flex", alignItems: "center", justifyContent: "space-between",
        flexWrap: "wrap", gap: "1rem",
        padding: "1rem 1.5rem",
        background: "white",
        borderBottom: "1px solid #e5e7eb",
        flexShrink: 0,
      }}>
        <div style={{ display: "flex", alignItems: "center", gap: "1rem" }}>
          <div style={{
            width: 42, height: 42, borderRadius: 10, flexShrink: 0,
            background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
            display: "flex", alignItems: "center", justifyContent: "center",
            boxShadow: `0 4px 14px ${primaryColor}35`,
          }}>
            <i className="fas fa-magic" style={{ color: "white", fontSize: 18 }} />
          </div>
          <div>
            <h1 style={{ margin: 0, fontSize: "1.1rem", fontWeight: 700, color: "#0f172a" }}>AI Assistant</h1>
            <p style={{ margin: 0, fontSize: "0.82rem", color: "#64748b" }}>Ask questions, generate SQL, create charts and dashboards</p>
          </div>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", flexWrap: "wrap" }}>

          {/* 1. Data source */}
          <div style={{ display: "flex", alignItems: "center", gap: "0.35rem" }}>
            <i className="fas fa-server" style={{ fontSize: 12, color: "#94a3b8" }} />
            <select
              value={selectedSource?.id ?? ""}
              onChange={e => {
                const src = sources.find(s => s.id === Number(e.target.value)) ?? null;
                setSelectedSource(src);
                setSelectedDataset(null);
              }}
              style={{ fontSize: 13, border: "1px solid #e2e8f0", borderRadius: 7, padding: "0.3rem 0.55rem", color: "#374151", background: "white", cursor: "pointer", maxWidth: 160 }}
            >
              <option value="">All sources</option>
              {sources.map(s => <option key={s.id} value={s.id}>{s.name}</option>)}
            </select>
          </div>

          {/* Arrow */}
          <i className="fas fa-chevron-right" style={{ fontSize: 10, color: "#cbd5e1" }} />

          {/* 2. Dataset (filtered by source) */}
          <div style={{ display: "flex", alignItems: "center", gap: "0.35rem" }}>
            <i className="fas fa-database" style={{ fontSize: 12, color: "#94a3b8" }} />
            <select
              value={selectedDataset ?? ""}
              onChange={e => setSelectedDataset(e.target.value ? Number(e.target.value) : null)}
              style={{ fontSize: 13, border: "1px solid #e2e8f0", borderRadius: 7, padding: "0.3rem 0.55rem", color: "#374151", background: "white", cursor: "pointer", maxWidth: 160 }}
            >
              <option value="">No dataset</option>
              {datasets
                .filter(d => !selectedSource || d.database_name === selectedSource.database_name)
                .map(d => <option key={d.id} value={d.id}>{d.name}</option>)}
            </select>
          </div>

          {/* 3. SQL context toggle */}
          <button
            type="button"
            onClick={() => setShowSqlContext(v => !v)}
            title="Add SQL context"
            style={{
              display: "inline-flex", alignItems: "center", gap: "0.3rem",
              padding: "0.3rem 0.65rem", borderRadius: 7, fontSize: 12, cursor: "pointer",
              border: `1px solid ${showSqlContext ? primaryColor : "#e2e8f0"}`,
              background: showSqlContext ? `${primaryColor}10` : "white",
              color: showSqlContext ? primaryColor : "#64748b",
              fontWeight: showSqlContext ? 600 : 400,
            }}
          >
            <i className="fas fa-code" style={{ fontSize: 11 }} />
            {sqlContext.trim() ? "SQL ✓" : "SQL"}
          </button>

          <div style={{ width: 1, height: 18, background: "#e2e8f0" }} />

          {messages.length > 0 && (
            <button type="button" onClick={() => setMessages([])}
              style={{ background: "none", border: "1px solid #e2e8f0", borderRadius: 7, padding: "0.3rem 0.65rem", fontSize: 12, color: "#64748b", cursor: "pointer" }}>
              <i className="fas fa-trash-alt" style={{ marginRight: 4 }} />Clear
            </button>
          )}
          <a href="/settings/ai" style={{ background: "none", border: "1px solid #e2e8f0", borderRadius: 7, padding: "0.3rem 0.65rem", fontSize: 12, color: "#64748b", cursor: "pointer", textDecoration: "none", display: "inline-flex", alignItems: "center", gap: "0.3rem" }}>
            <i className="fas fa-cog" />Keys
          </a>
        </div>
      </div>

      {/* SQL context panel (collapsible) */}
      {showSqlContext && (
        <div style={{ padding: "0.75rem 1.5rem", background: "#f8fafc", borderBottom: "1px solid #e5e7eb", flexShrink: 0 }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: "0.4rem" }}>
            <span style={{ fontSize: 12, fontWeight: 600, color: "#475569" }}>
              <i className="fas fa-code" style={{ marginRight: 5 }} />SQL context — paste your current query so the AI can reference it
            </span>
            {sqlContext.trim() && (
              <button type="button" onClick={() => setSqlContext("")} style={{ background: "none", border: "none", fontSize: 11, color: "#94a3b8", cursor: "pointer" }}>
                Clear
              </button>
            )}
          </div>
          <textarea
            value={sqlContext}
            onChange={e => setSqlContext(e.target.value)}
            placeholder="SELECT * FROM my_table WHERE ..."
            rows={4}
            style={{
              width: "100%", boxSizing: "border-box", resize: "vertical",
              fontFamily: "monospace", fontSize: 12, lineHeight: 1.6,
              border: "1px solid #e2e8f0", borderRadius: 8, padding: "0.6rem 0.75rem",
              color: "#1e293b", background: "white", outline: "none",
            }}
          />
        </div>
      )}

      {/* Messages area */}
      <div style={{ flex: 1, overflowY: "auto", padding: "1.25rem 1.5rem", display: "flex", flexDirection: "column", gap: "1rem" }}>

        {/* Welcome screen */}
        {messages.length === 0 && (
          <div style={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", padding: "2rem 0", gap: "2rem" }}>
            <div style={{ textAlign: "center" }}>
              <div style={{
                width: 72, height: 72, borderRadius: 20, margin: "0 auto 1rem",
                background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
                display: "flex", alignItems: "center", justifyContent: "center",
                boxShadow: `0 8px 28px ${primaryColor}40`,
              }}>
                <i className="fas fa-magic" style={{ color: "white", fontSize: 30 }} />
              </div>
              <h2 style={{ margin: "0 0 0.5rem", fontSize: "1.4rem", fontWeight: 700, color: "#0f172a" }}>What would you like to explore?</h2>
              <p style={{ margin: 0, fontSize: "0.9rem", color: "#64748b", maxWidth: 420 }}>
                Ask anything about your data, generate SQL, create charts, or design a dashboard.
              </p>
            </div>
            <div style={{ display: "flex", flexWrap: "wrap", gap: "0.6rem", justifyContent: "center", maxWidth: 640 }}>
              {QUICK_ACTIONS.map(a => (
                <button
                  key={a.label}
                  type="button"
                  onClick={() => { setInput(a.prompt); setTimeout(() => inputRef.current?.focus(), 50); }}
                  style={{
                    display: "inline-flex", alignItems: "center", gap: "0.45rem",
                    padding: "0.5rem 1rem", borderRadius: 20,
                    border: `1px solid ${primaryColor}30`,
                    background: `${primaryColor}08`,
                    color: primaryColor, fontSize: 13, fontWeight: 500,
                    cursor: "pointer", transition: "all 0.15s",
                  }}
                  onMouseEnter={e => { (e.currentTarget as HTMLButtonElement).style.background = `${primaryColor}15`; }}
                  onMouseLeave={e => { (e.currentTarget as HTMLButtonElement).style.background = `${primaryColor}08`; }}
                >
                  <i className={`fas ${a.icon}`} style={{ fontSize: 11 }} />
                  {a.label}
                </button>
              ))}
            </div>
          </div>
        )}

        {/* No key configured notice */}
        {noKey && (
          <div style={{ padding: "1rem 1.25rem", borderRadius: 10, background: "#fffbeb", border: "1px solid #fcd34d", display: "flex", alignItems: "flex-start", gap: "0.75rem" }}>
            <i className="fas fa-key" style={{ color: "#d97706", marginTop: 2 }} />
            <div style={{ flex: 1, fontSize: 14 }}>
              <strong style={{ color: "#92400e" }}>No AI key configured.</strong>
              <p style={{ margin: "0.25rem 0 0", color: "#78350f" }}>
                Add your Anthropic or OpenAI API key to start using the AI Assistant.
                {" "}<a href="/settings/ai" style={{ color: "#d97706", fontWeight: 600 }}>Go to AI Settings →</a>
              </p>
            </div>
          </div>
        )}

        {/* Message bubbles */}
        {messages.map((m, i) => (
          <div key={i} style={{
            display: "flex",
            flexDirection: m.role === "user" ? "row-reverse" : "row",
            alignItems: "flex-start",
            gap: "0.75rem",
          }}>
            {/* Avatar */}
            <div style={{
              width: 32, height: 32, borderRadius: "50%", flexShrink: 0,
              display: "flex", alignItems: "center", justifyContent: "center",
              background: m.role === "user" ? `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})` : "var(--bg-hover)",
              fontSize: 13, fontWeight: 600, color: m.role === "user" ? "white" : "var(--text-secondary)",
            }}>
              {m.role === "user" ? (account?.name?.[0] ?? "U") : <i className="fas fa-magic" />}
            </div>
            {/* Bubble */}
            <div style={{
              maxWidth: "78%",
              padding: "0.75rem 1rem",
              borderRadius: m.role === "user" ? "14px 4px 14px 14px" : "4px 14px 14px 14px",
              background: m.role === "user" ? `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})` : "var(--bg-surface)",
              color: m.role === "user" ? "white" : "var(--text-primary)",
              border: m.role === "user" ? "none" : "1px solid var(--border)",
              boxShadow: "0 1px 3px rgba(15,23,42,0.06)",
              fontSize: 14,
              lineHeight: 1.65,
            }}>
              {m.loading ? (
                <div style={{ display: "flex", gap: "0.3rem", alignItems: "center", padding: "0.2rem 0" }}>
                  {[0, 1, 2].map(d => (
                    <div key={d} style={{
                      width: 6, height: 6, borderRadius: "50%", background: "var(--accent)",
                      animation: `bounce 1.2s ease-in-out ${d * 0.2}s infinite`,
                    }} />
                  ))}
                  <style>{`@keyframes bounce { 0%,80%,100% { transform:translateY(0) } 40% { transform:translateY(-6px) } }`}</style>
                </div>
              ) : m.role === "user" ? (
                <span style={{ whiteSpace: "pre-wrap" }}>{m.content}</span>
              ) : (
                <>
                  {renderMessage(m.content, primaryColor, handleCopySql, handleOpenLab, handleCreateChart)}
                  {m.chart && (
                    <div style={{ marginTop: 12 }}>
                      <InlineChart
                        rows={m.chart.rows}
                        columns={m.chart.columns}
                        chartType={m.chart.chartType}
                        xAxis={m.chart.xAxis}
                        yAxis={m.chart.yAxis}
                        title={m.chart.title}
                        sql={m.chart.sql}
                      />
                    </div>
                  )}
                </>
              )}
            </div>
          </div>
        ))}
        <div ref={bottomRef} />
      </div>

      {/* Input bar */}
      <div style={{
        padding: "1rem 1.5rem",
        background: "white",
        borderTop: "1px solid #e5e7eb",
        flexShrink: 0,
      }}>
        <div style={{ display: "flex", gap: "0.75rem", alignItems: "flex-end", maxWidth: 900, margin: "0 auto" }}>
          <textarea
            ref={inputRef}
            value={input}
            onChange={e => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Ask anything about your data… (Enter to send, Shift+Enter for new line)"
            rows={1}
            style={{
              flex: 1,
              resize: "none",
              border: `1.5px solid ${primaryColor}40`,
              borderRadius: 12,
              padding: "0.75rem 1rem",
              fontSize: 14,
              fontFamily: "inherit",
              outline: "none",
              lineHeight: 1.5,
              maxHeight: 160,
              overflowY: "auto",
              transition: "border-color 0.15s",
              boxShadow: `0 0 0 0px ${primaryColor}20`,
            }}
            onFocus={e => { e.currentTarget.style.borderColor = primaryColor; e.currentTarget.style.boxShadow = `0 0 0 3px ${primaryColor}15`; }}
            onBlur={e => { e.currentTarget.style.borderColor = `${primaryColor}40`; e.currentTarget.style.boxShadow = "none"; }}
            onInput={e => {
              const el = e.currentTarget;
              el.style.height = "auto";
              el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
            }}
          />
          <button
            type="button"
            onClick={() => void sendMessage(input)}
            disabled={!input.trim() || sending}
            style={{
              width: 44, height: 44, borderRadius: 12, flexShrink: 0,
              background: input.trim() && !sending ? `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})` : "#e2e8f0",
              border: "none", cursor: input.trim() && !sending ? "pointer" : "not-allowed",
              color: "white", fontSize: 16, display: "flex", alignItems: "center", justifyContent: "center",
              transition: "all 0.15s",
            }}
          >
            <i className={sending ? "fas fa-spinner fa-spin" : "fas fa-paper-plane"} style={{ fontSize: 15 }} />
          </button>
        </div>
        <p style={{ margin: "0.4rem auto 0", maxWidth: 900, fontSize: 11, color: "#94a3b8", textAlign: "center" }}>
          AI can make mistakes. Verify SQL before running in production.
        </p>
      </div>
    </div>
  );
}
