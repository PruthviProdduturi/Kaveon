"use client";

import { useEffect, useRef, useState, KeyboardEvent } from "react";
import { useAuth } from "../auth/useAuth";
import { useSetup } from "../components/ClientLayout";
import { msalFetch } from "../utils/msalFetch";
import { KaveonMark } from "../components/KaveonMark";
import { nlToSql, DatasetSchema } from "../utils/nlToSql";
import { InlineChart } from "../components/chat/InlineChart";
import { API_BASE } from "../config";
import { useRecents } from "../hooks/useRecents";

// ── Types ──────────────────────────────────────────────────────────────────────

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

interface PageData {
  datasetCount: number;
  sourceCount: number;
  tableCount: number;
  sourceNames: string[];
}

interface DatasetOption {
  id: number;
  name: string;
  database_name?: string;
}

interface SourceOption {
  id: number;
  name: string;
  database_name: string;
}

// ── Constants ──────────────────────────────────────────────────────────────────

const DEFAULT_SUGGESTIONS = [
  "Show total confirmed cases by country",
  "Top 10 countries by total deaths",
  "NYC taxi trips by borough",
  "Trend of new cases over time",
];

const EMPTY_SUGGESTIONS = [
  { label: "Connect Fabric SQL", href: "/data-sources" },
  { label: "Connect PostgreSQL", href: "/data-sources" },
  { label: "Connect Azure SQL", href: "/data-sources" },
];

// ── Page ───────────────────────────────────────────────────────────────────────

export default function Home() {
  const { account } = useAuth();
  const { isSetupOk } = useSetup();

  const [query, setQuery] = useState("");
  const [data, setData] = useState<PageData | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [sending, setSending] = useState(false);
  const [datasets, setDatasets] = useState<DatasetOption[]>([]);
  const [sources, setSources] = useState<SourceOption[]>([]);
  const [selectedDataset, setSelectedDataset] = useState<number | null>(null);
  const [selectedSource, setSelectedSource] = useState<SourceOption | null>(null);
  const [datasetSchema, setDatasetSchema] = useState<DatasetSchema | null>(null);
  const [schemasReady, setSchemasReady] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const { addRecent } = useRecents();

  const email = account?.email ?? "";
  const hasData = data !== null && data.sourceCount > 0;
  const isEmpty = data !== null && data.sourceCount === 0;
  const inConversation = messages.length > 0;

  // ── Data fetching ────────────────────────────────────────────────────────────

  useEffect(() => {
    if (!isSetupOk || !email) return;
    const headers = { "x-user-email": email };

    async function load() {
      try {
        const [summaryRes, listRes, activeRes, dsRes] = await Promise.all([
          msalFetch("/api/v1/metadata/summary", { headers }),
          msalFetch("/api/v1/data-sources/list", { headers }),
          msalFetch("/api/v1/data-sources/active", { headers }),
          msalFetch("/api/v1/datasets"),
        ]);

        const summary = summaryRes.ok ? await summaryRes.json() : { dataset_count: 0 };
        const listRaw = listRes.ok ? await listRes.json() : [];
        const list = Array.isArray(listRaw) ? listRaw : (listRaw.dataSources || listRaw.sources || []);
        const active = activeRes.ok ? await activeRes.json() : {};

        let tableCount = 0;
        if (typeof active.table_count === "number") {
          tableCount = active.table_count;
        } else if (Array.isArray(active.tables)) {
          tableCount = active.tables.length;
        } else if (Array.isArray(active)) {
          tableCount = (active as any[]).reduce((sum: number, s: any) => sum + (typeof s.table_count === "number" ? s.table_count : 0), 0);
        } else if (active.dataSources && Array.isArray(active.dataSources)) {
          tableCount = active.dataSources.reduce((sum: number, s: any) => sum + (typeof s.table_count === "number" ? s.table_count : 0), 0);
        }

        const datasetCount = summary.dataset_count ?? summary.datasets_count ?? summary.datasetCount ?? 0;

        setData({
          datasetCount,
          sourceCount: list.length,
          tableCount,
          sourceNames: list.map((s: any) => s.database_name ?? s.name ?? "Unknown"),
        });

        setSources(list.map((s: any) => ({ id: s.id, name: s.name, database_name: s.database_name })));

        if (dsRes.ok) {
          const dd = await dsRes.json();
          const rawDs = Array.isArray(dd) ? dd : (dd.recent || dd.datasets || dd.result || []);
          const dsList = rawDs.map((ds: any) => ({
            id: ds.id,
            name: ds.dataset_name || ds.name || `Dataset ${ds.id}`,
            database_name: ds.database_name,
          }));
          setDatasets(dsList);
          // Auto-select first dataset
          if (dsList.length > 0 && !selectedDataset) {
            setSelectedDataset(dsList[0].id);
            // Auto-select matching source
            const matchSource = list.find((s: any) => s.database_name === dsList[0].database_name);
            if (matchSource) setSelectedSource({ id: matchSource.id, name: matchSource.name, database_name: matchSource.database_name });
          }
        }
      } catch {
        setData({ datasetCount: 0, sourceCount: 0, tableCount: 0, sourceNames: [] });
      }
    }

    load();
  }, [isSetupOk, email]);

  // Fetch dataset schema when selected
  useEffect(() => {
    if (!selectedDataset) { setDatasetSchema(null); return; }
    msalFetch(`/api/v1/datasets/${selectedDataset}`)
      .then(r => r.ok ? r.json() : null)
      .then(d => {
        if (!d) { setDatasetSchema(null); return; }
        const cols = (d.columns || []).map((c: any) => ({
          name: c.column_name || c.name,
          type: c.data_type?.includes("int") || c.data_type?.includes("float") || c.data_type?.includes("decimal") || c.data_type?.includes("numeric") ? "number"
            : c.data_type?.includes("date") || c.data_type?.includes("time") ? "date" : "string",
          description: c.description || "",
        }));
        const metrics = (d.metrics || []).map((m: any) => ({
          name: m.name || m.metric_name,
          expression: m.sql_expression || m.expression || `SUM(${m.name})`,
          description: m.description || "",
        }));
        const table = d.fact_table ? (d.schema_name ? `${d.schema_name}.${d.fact_table}` : d.fact_table) : d.table_name || "data";
        setDatasetSchema({ tableName: table, columns: cols, metrics });
      })
      .catch(() => setDatasetSchema(null));
  }, [selectedDataset]);

  // Auto-scroll
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  // Cache all dataset schemas for auto-matching
  type SchemaEntry = { id: number; name: string; sourceId?: number; sourceName?: string; schema: DatasetSchema };
  const [allSchemas, setAllSchemas] = useState<SchemaEntry[]>([]);
  const allSchemasRef = useRef<SchemaEntry[]>([]);

  useEffect(() => {
    if (datasets.length === 0) return;
    // Fetch schema for each dataset
    Promise.all(
      datasets.map(async (ds) => {
        try {
          const r = await msalFetch(`/api/v1/datasets/${ds.id}`);
          if (!r.ok) return null;
          const d = await r.json();
          const cols = (d.columns || []).map((c: any) => ({
            name: c.column_name || c.name,
            type: c.data_type?.includes("int") || c.data_type?.includes("float") || c.data_type?.includes("decimal") || c.data_type?.includes("numeric") ? "number"
              : c.data_type?.includes("date") || c.data_type?.includes("time") ? "date" : "string",
            description: c.description || "",
          }));
          const metrics = (d.metrics || []).map((m: any) => ({
            name: m.name || m.metric_name,
            expression: m.sql_expression || m.expression || `SUM(${m.name})`,
            description: m.description || "",
          }));
          const table = d.fact_table ? (d.schema_name ? `${d.schema_name}.${d.fact_table}` : d.fact_table) : d.table_name || "data";
          const src = sources.find(s => s.database_name === ds.database_name);
          return { id: ds.id, name: ds.name, sourceId: src?.id, sourceName: src?.database_name, schema: { tableName: table, columns: cols, metrics } };
        } catch { return null; }
      })
    ).then(results => {
      const valid = results.filter(Boolean) as SchemaEntry[];
      allSchemasRef.current = valid;
      setAllSchemas(valid);
      setSchemasReady(true);
    });
  }, [datasets, sources]);

  // ── Send message ─────────────────────────────────────────────────────────────

  function findBestSchema(text: string) {
    const lower = text.toLowerCase();
    const words = lower.split(/\s+/).filter(w => w.length >= 3);
    let best: { schema: DatasetSchema; sourceId?: number; sourceName?: string; confidence: number; name: string; parsed: any } | null = null;

    const schemas = allSchemasRef.current;
    for (const ds of schemas) {
      // Score: how well does this dataset match the query?
      let score = 0;

      // Dataset name overlap
      const nameWords = ds.name.toLowerCase().split(/[\s_\-·:]+/).filter(w => w.length >= 3);
      for (const nw of nameWords) {
        if (words.some(w => w.includes(nw) || nw.includes(w))) score += 0.3;
      }

      // Column name overlap
      for (const col of ds.schema.columns) {
        const colWords = col.name.toLowerCase().replace(/_/g, " ").split(/\s+/).filter(w => w.length >= 3);
        for (const cw of colWords) {
          if (words.some(w => w.includes(cw) || cw.includes(w))) score += 0.2;
        }
      }

      // Metric name overlap
      for (const m of ds.schema.metrics) {
        const mWords = m.name.toLowerCase().replace(/_/g, " ").split(/\s+/).filter(w => w.length >= 3);
        for (const mw of mWords) {
          if (words.some(w => w.includes(mw) || mw.includes(w))) score += 0.2;
        }
      }

      // Try NL→SQL parser
      const parsed = nlToSql(text, ds.schema);
      if (parsed) score += parsed.confidence;

      if (score > (best?.confidence ?? 0)) {
        best = { schema: ds.schema, sourceId: ds.sourceId, sourceName: ds.sourceName, confidence: score, name: ds.name, parsed };
      }
    }

    return best;
  }

  const canSend = schemasReady && !sending && !isEmpty;

  function generateInsight(
    rows: (string | number | null)[][],
    columns: string[],
    parsed: { chartType: string; xAxis: string | null; yAxis: string | null; title: string },
    userQuery: string,
  ): string {
    const xIdx = parsed.xAxis ? columns.indexOf(parsed.xAxis) : 0;
    const yIdx = parsed.yAxis ? columns.indexOf(parsed.yAxis) : (columns.length > 1 ? 1 : 0);

    const fmt = (v: number): string => {
      if (Math.abs(v) >= 1_000_000_000) return (v / 1_000_000_000).toFixed(1) + "B";
      if (Math.abs(v) >= 1_000_000) return (v / 1_000_000).toFixed(1) + "M";
      if (Math.abs(v) >= 1_000) return (v / 1_000).toFixed(1) + "K";
      return v.toLocaleString();
    };

    // KPI — single value
    if (parsed.chartType === "kpi" && rows.length === 1) {
      const val = rows[0][0];
      return `The total **${parsed.yAxis || columns[0]}** is **${fmt(Number(val))}**.`;
    }

    // Grouped data — show top 3 and total count
    if (rows.length > 1 && xIdx >= 0 && yIdx >= 0) {
      const sorted = [...rows].sort((a, b) => Number(b[yIdx] || 0) - Number(a[yIdx] || 0));
      const top3 = sorted.slice(0, 3).map(r => `**${r[xIdx]}** (${fmt(Number(r[yIdx] || 0))})`);
      const total = rows.length;
      const yLabel = parsed.yAxis || columns[yIdx] || "value";

      let insight = `Found **${total}** results for ${yLabel} by ${parsed.xAxis || columns[xIdx]}. `;
      insight += `Top 3: ${top3.join(", ")}. `;

      if (total > 10) {
        insight += `\n\nWant me to show just the **top 10** or filter by a specific value?`;
      }

      return insight;
    }

    // Table / fallback
    return `Here are **${rows.length}** results from your data. ${rows.length > 20 ? "Showing first 50 rows." : ""}`;
  }

  async function sendMessage(text: string) {
    if (!text.trim() || !canSend) return;
    const userMsg: Message = { role: "user", content: text.trim() };
    const loadingMsg: Message = { role: "assistant", content: "", loading: true };
    setMessages(prev => [...prev, userMsg, loadingMsg]);
    setQuery("");
    setSending(true);

    try {
      // Auto-find best matching dataset
      const match = findBestSchema(text.trim());
      const schema = match?.schema || datasetSchema;
      const srcId = match?.sourceId || selectedSource?.id;
      const srcDb = match?.sourceName || selectedSource?.database_name;
      let parsed = match?.parsed || (schema ? nlToSql(text.trim(), schema) : null);

      // Fallback: if we matched a dataset by name but parser returned null,
      // build a simple SELECT query showing the data
      if (schema && !parsed && match && match.confidence >= 0.3) {
        const numCols = schema.columns.filter(c => c.type === "number").slice(0, 2);
        const strCols = schema.columns.filter(c => c.type === "string").slice(0, 1);
        const dateCols = schema.columns.filter(c => c.type === "date").slice(0, 1);
        const selectCols = [...strCols, ...dateCols, ...numCols].map(c => c.name);
        if (selectCols.length > 0) {
          parsed = {
            sql: `SELECT ${selectCols.join(", ")} FROM ${schema.tableName} LIMIT 50`,
            chartType: "table" as const,
            xAxis: strCols[0]?.name || null,
            yAxis: numCols[0]?.name || null,
            title: `Data from ${match.name}`,
            confidence: 0.3,
          };
        }
      }

      if (schema && parsed) {
          const execRes = await msalFetch("/api/v1/sql/execute", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              sql_text: parsed.sql,
              database: srcDb || "neondb",
              source: "chat",
            }),
          });

          if (execRes.ok) {
            const execData = await execRes.json();
            const rows = execData.rows || execData.data || [];
            const columns = execData.columns || execData.column_names || [];

            if (rows.length > 0) {
              const summary = generateInsight(rows, columns, parsed, text.trim());
              setMessages(prev => [...prev.slice(0, -1), {
                role: "assistant",
                content: summary,
                chart: { rows, columns, chartType: parsed.chartType, xAxis: parsed.xAxis, yAxis: parsed.yAxis, title: parsed.title, sql: parsed.sql },
              }]);
              return;
            }

            setMessages(prev => [...prev.slice(0, -1), {
              role: "assistant",
              content: `No results found. The query returned empty.\n\nSQL: \`${parsed.sql}\``,
            }]);
            return;
          }
      }

      // Parser couldn't handle it — show helpful suggestions
      const schemasNow = allSchemasRef.current;
      const availableDatasets = schemasNow.map(s => s.name).join(", ");
      setMessages(prev => [...prev.slice(0, -1), {
        role: "assistant",
        content: schemasNow.length === 0
          ? "No datasets found. Create a dataset in the Workspace first, then come back and ask me about your data."
          : `I couldn't match that to your data. You have these datasets: **${availableDatasets}**\n\nTry something specific like:\n• "Show [metric] by [column]"\n• "Top 10 [column] by [metric]"\n• "Total [metric]"\n• "Trend of [metric] over time"`,
      }]);
    } catch (e) {
      setMessages(prev => [...prev.slice(0, -1), {
        role: "assistant",
        content: `Something went wrong. ${e instanceof Error ? e.message : "Please try again."}`,
      }]);
    } finally {
      setSending(false);
      setTimeout(() => inputRef.current?.focus(), 100);
    }
  }

  function submit() {
    void sendMessage(query);
  }

  function handleKey(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter") submit();
  }

  // ── Render ───────────────────────────────────────────────────────────────────

  const heroText = isEmpty ? "Connect your first data source" : "Your data has answers";
  const placeholder = isEmpty ? "Set up a connection to get started..."
    : !schemasReady ? "Loading your data context…"
    : "Ask anything about your data…";

  // Build meta line — only show positive counts
  let metaParts: string[] = [];
  if (data && !isEmpty) {
    if (data.sourceCount > 0) metaParts.push(`${data.sourceCount} source${data.sourceCount !== 1 ? "s" : ""}`);
    if (data.tableCount > 0) metaParts.push(`${data.tableCount} table${data.tableCount !== 1 ? "s" : ""}`);
    const dsCount = datasets.length || data.datasetCount;
    if (dsCount > 0) metaParts.push(`${dsCount} dataset${dsCount !== 1 ? "s" : ""}`);
  }
  const metaLine = metaParts.length > 0 ? metaParts.join(" · ") : null;

  return (
    <div
      style={{
        position: "relative",
        minHeight: "100vh",
        display: "flex",
        flexDirection: "column",
        background: "var(--bg-primary)",
      }}
    >
      {/* Hero section — collapses when in conversation */}
      {!inConversation && (
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            position: "relative",
            paddingBottom: "8vh",
          }}
        >
          {/* Watermark */}
          <div aria-hidden style={{ position: "absolute", top: "50%", left: "50%", transform: "translate(-50%, -50%)", pointerEvents: "none" }}>
            <KaveonMark size={280} opacity={0.07} useDirectColor />
          </div>

          <div style={{ position: "relative", zIndex: 1, display: "flex", flexDirection: "column", alignItems: "center", gap: "1rem", width: "100%", maxWidth: 680, padding: "0 1.5rem" }}>
            <h1 style={{ margin: 0, fontSize: 28, fontWeight: 600, color: "var(--text-primary)", textAlign: "center" }}>{heroText}</h1>
            {metaLine && <p style={{ margin: 0, fontSize: 13, color: "var(--text-muted)", textAlign: "center" }}>{metaLine}</p>}

            {/* Input */}
            <div style={{ width: "100%", maxWidth: 640, display: "flex", alignItems: "center", background: "var(--bg-surface)", border: "1px solid var(--border)", borderRadius: 14, padding: "10px 12px", boxShadow: "var(--shadow-md)" }}>
              <input ref={inputRef} type="text" value={query} onChange={e => setQuery(e.target.value)} onKeyDown={handleKey} placeholder={placeholder}
                style={{ flex: 1, border: "none", outline: "none", background: "transparent", color: "var(--text-primary)", fontSize: 14, lineHeight: 1.5 }} />
              <button onClick={submit} disabled={!query.trim() || !canSend} style={{ flexShrink: 0, width: 30, height: 30, borderRadius: 8, border: "none", background: query.trim() && canSend ? "var(--accent)" : "var(--bg-hover)", color: query.trim() && canSend ? "#fff" : "var(--text-faint)", cursor: query.trim() && canSend ? "pointer" : "default", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 16, transition: "background 0.15s" }}>
                ↑
              </button>
            </div>

            {/* Suggestions */}
            <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem", justifyContent: "center" }}>
              {isEmpty
                ? EMPTY_SUGGESTIONS.map(s => (
                    <a key={s.label} href={s.href} style={{ padding: "6px 14px", borderRadius: 999, border: "1px solid var(--border)", background: "var(--bg-surface)", color: "var(--text-secondary)", fontSize: 13, textDecoration: "none", boxShadow: "var(--shadow-sm)" }}>{s.label}</a>
                  ))
                : DEFAULT_SUGGESTIONS.map(s => (
                    <button key={s} onClick={() => { if (canSend) { setQuery(s); setTimeout(() => void sendMessage(s), 50); } }}
                      style={{ padding: "8px 16px", borderRadius: 999, border: "1px solid var(--border)", background: "var(--bg-surface)", color: "var(--text-secondary)", fontSize: 13, cursor: "pointer", boxShadow: "var(--shadow-md)", transition: "all 0.15s" }}
                      onMouseEnter={(e) => { e.currentTarget.style.borderColor = "rgba(var(--accent-rgb), 0.3)"; e.currentTarget.style.color = "var(--text-primary)"; }}
                      onMouseLeave={(e) => { e.currentTarget.style.borderColor = "var(--border)"; e.currentTarget.style.color = "var(--text-secondary)"; }}>
                      {s}
                    </button>
                  ))}
            </div>
          </div>
        </div>
      )}

      {/* Conversation view — appears after first message */}
      {inConversation && (
        <div style={{ flex: 1, display: "flex", flexDirection: "column" }}>
          {/* Conversation header */}
          <div style={{ padding: "12px 24px", borderBottom: "1px solid var(--border)", display: "flex", alignItems: "center", gap: 12, background: "var(--bg-surface)", flexShrink: 0 }}>
            <KaveonMark size={24} useDirectColor />
            <span style={{ fontSize: 14, fontWeight: 600, color: "var(--text-primary)", flex: 1 }}>Chat</span>

            <button onClick={() => {
              // Save old conversation to recents
              if (messages.length > 0) {
                const firstUserMsg = messages.find(m => m.role === "user");
                const label = firstUserMsg?.content.slice(0, 50) || "Chat";
                addRecent({ id: `chat-${Date.now()}`, label, href: "/", type: "chat" });
              }
              setMessages([]);
            }} style={{ padding: "4px 10px", borderRadius: 6, border: "1px solid var(--border)", background: "transparent", color: "var(--text-muted)", fontSize: 12, cursor: "pointer" }}>
              New chat
            </button>
          </div>

          {/* Messages */}
          <div style={{ flex: 1, overflow: "auto", padding: "20px 24px", display: "flex", flexDirection: "column", gap: 16 }}>
            {messages.map((m, i) => (
              <div key={i} style={{ display: "flex", flexDirection: m.role === "user" ? "row-reverse" : "row", gap: 10, alignItems: "flex-start" }}>
                {/* Avatar */}
                <div style={{
                  width: 28, height: 28, borderRadius: "50%", flexShrink: 0,
                  display: "flex", alignItems: "center", justifyContent: "center",
                  background: m.role === "user" ? "var(--accent)" : "var(--bg-hover)",
                  fontSize: 12, fontWeight: 600, color: m.role === "user" ? "#fff" : "var(--text-secondary)",
                }}>
                  {m.role === "user" ? (account?.name?.[0] ?? "U") : "K"}
                </div>

                {/* Bubble */}
                <div style={{
                  maxWidth: m.chart ? "90%" : "75%",
                  padding: "10px 14px",
                  borderRadius: m.role === "user" ? "14px 4px 14px 14px" : "4px 14px 14px 14px",
                  background: m.role === "user" ? "var(--accent)" : "var(--bg-surface)",
                  color: m.role === "user" ? "#fff" : "var(--text-primary)",
                  border: m.role === "user" ? "none" : "1px solid var(--border)",
                  fontSize: 14, lineHeight: 1.6,
                  overflow: "hidden",
                }}>
                  {m.loading ? (
                    <div style={{ display: "flex", gap: 4, padding: "8px 14px" }}>
                      {[0, 1, 2].map(d => (
                        <div key={d} style={{ width: 6, height: 6, borderRadius: "50%", background: "var(--accent)", animation: `bounce 1.2s ease-in-out ${d * 0.2}s infinite` }} />
                      ))}
                      <style>{`@keyframes bounce { 0%,80%,100% { transform:translateY(0) } 40% { transform:translateY(-6px) } }`}</style>
                    </div>
                  ) : (
                    <>
                      {m.content && (
                        <div style={{ padding: m.chart ? "0 0 8px" : 0, whiteSpace: "pre-wrap", lineHeight: 1.6 }}
                          dangerouslySetInnerHTML={{ __html: m.content.replace(/\*\*(.*?)\*\*/g, "<strong>$1</strong>") }}
                        />
                      )}
                      {m.chart && (
                        <InlineChart
                          rows={m.chart.rows}
                          columns={m.chart.columns}
                          chartType={m.chart.chartType}
                          xAxis={m.chart.xAxis}
                          yAxis={m.chart.yAxis}
                          title={m.chart.title}
                          sql={m.chart.sql}
                        />
                      )}
                    </>
                  )}
                </div>
              </div>
            ))}
            <div ref={bottomRef} />
          </div>

          {/* Input bar (bottom) */}
          <div style={{ padding: "12px 24px", borderTop: "1px solid var(--border)", background: "var(--bg-surface)", flexShrink: 0 }}>
            <div style={{ maxWidth: 700, margin: "0 auto", display: "flex", alignItems: "center", gap: 0, background: "var(--bg-primary)", border: "1px solid var(--border)", borderRadius: 12, padding: "8px 12px" }}>
              <input ref={inputRef} type="text" value={query} onChange={e => setQuery(e.target.value)} onKeyDown={handleKey} placeholder="Ask a follow-up..."
                style={{ flex: 1, border: "none", outline: "none", background: "transparent", color: "var(--text-primary)", fontSize: 14 }} />
              <button onClick={submit} disabled={!query.trim() || !canSend} style={{ width: 28, height: 28, borderRadius: 7, border: "none", background: query.trim() && canSend ? "var(--accent)" : "var(--bg-hover)", color: query.trim() && canSend ? "#fff" : "var(--text-faint)", cursor: query.trim() && canSend ? "pointer" : "default", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 14 }}>
                ↑
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
