"use client";

import { useEffect, useMemo, useRef, useState, useCallback, KeyboardEvent } from "react";
import { useAuth } from "../../auth/useAuth";
import { useSetup } from "../../components/ClientLayout";
import { msalFetch } from "../../utils/msalFetch";
import { KaveonMark } from "../../components/KaveonMark";
import { KaveonLoading } from "../../components/KaveonLoading";
import { ContextBanner } from "../../components/ContextBanner";
import { nlToSql, DatasetSchema } from "../../utils/nlToSql";
import { InlineChart } from "../../components/chat/InlineChart";
import { EvidencePanel, Evidence, Row, headlineOf } from "../../components/chat/EvidencePanel";
import { API_BASE } from "../../config";
import { useRecents } from "../../hooks/useRecents";
import { useDatasetSchemas } from "../../hooks/useDatasetSchemas";
import { useDemoQuota } from "../../hooks/useDemoQuota";
import { rateLimitNotice } from "../../utils/demoQuota";
import { useSearchParams } from "next/navigation";

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

interface RouteMeta {
  route: "context" | "hybrid" | "query" | "direct" | "dlm";
  durationMs?: number;
  approx?: boolean;
  datasetName?: string;
  /** The lane the DLM's answer took — the Engine's word for an Engine-backed
   *  dataset (`execution.mode`), the DLM's own for a warehouse one. */
  lane?: "context" | "cache" | "live";
  /** The named question class the DLM answered as — `breakdown`,
   *  `comparison_period`, `share_of_total`, … See `api/dlm/classes.py`. */
  questionClass?: string;
  evidence?: Evidence;
  headline?: Row | null;
}

interface ContextHint { label: string; value: number | string | null }

// The DLM found two equally good readings of one slot and asks before
// guessing. Picking an option re-posts the original question with the slot
// pinned; the DLM does not pick silently.
interface Clarification {
  /** `question` is the refusal path: the options are whole questions the
   *  dataset can answer, asked afresh rather than pinning a slot. */
  kind: "metric" | "dimension" | "value" | "question";
  prompt: string;
  options: { id: string; label: string; description?: string }[];
  resume: { question: string; choices: Record<string, string> };
}

interface Message {
  role: "user" | "assistant";
  content: string;
  /** The demo's quota notice: shown quietly, never saved to the session. */
  notice?: boolean;
  loading?: boolean;
  liveSince?: number;   // epoch ms — set once a loading message is running a live query
  contextHints?: ContextHint[];   // relevant precomputed slices shown while live runs
  chart?: ChartData;
  routeMeta?: RouteMeta;
  clarification?: Clarification;
  chosen?: string;      // option id the user picked, once the clarification is answered
}

/** The Engine's quota refusal as the ask box says it: the time it names,
 *  and what does not count against the quota. */
function quotaNotice(message: string): string {
  return `${message}. Answers from precomputed context and the result cache do not count against the quota.`;
}

/** Assistant text is rendered with **bold** markup only. Everything else is
 *  escaped first: dataset names, notes and column values reach this string
 *  from the database, and an Analyst-authored name must never become markup
 *  in another viewer's chat. */
function renderAssistantHtml(content: string): string {
  const escaped = content
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;").replace(/'/g, "&#39;");
  return escaped.replace(/\*\*(.*?)\*\*/g, "<strong>$1</strong>");
}

/** Everything an answer showed beyond its text: the lane it took, the evidence
 *  behind it and the chart's axes. It rides in the message's `data` blob —
 *  free-form JSON on `chat_message` — because reopening a conversation has to
 *  restore the answer, not a paragraph of it. Result rows are stored alongside
 *  (first 100) and are the one part the API will drop to stay inside KaveonDB's
 *  document bound, since the stored statement reproduces them. */
function persistedMeta(meta: RouteMeta, chart: ChartData | null): Record<string, unknown> {
  return {
    lane: meta.lane, approx: meta.approx, datasetName: meta.datasetName,
    questionClass: meta.questionClass, durationMs: meta.durationMs,
    evidence: meta.evidence, headline: meta.headline,
    ...(chart ? { chart: { xAxis: chart.xAxis, yAxis: chart.yAxis, title: chart.title } } : {}),
  };
}

/** The routes that name a lane and so render as a provenance badge. `clarify`,
 *  `error`, `unanswerable`, `out_of_scope` and `no_match` are recorded for
 *  provenance but describe no query; restoring them as a badge labelled every
 *  one of them "Live query". */
const BADGED_ROUTES = new Set<string>(["context", "hybrid", "query", "direct", "dlm"]);

/** A stored message as the transcript renders it — the exact reverse of what
 *  `persistedMeta` writes. */
function restoreMessage(stored: Record<string, unknown>): Message {
  const data = (stored.data ?? null) as Record<string, unknown> | null;
  const meta = (data?.meta ?? null) as Record<string, unknown> | null;
  const chartMeta = (meta?.chart ?? null) as Record<string, unknown> | null;
  const rows = (data?.rows as ChartData["rows"] | undefined) ?? [];
  const columns = (data?.columns as string[] | undefined) ?? [];
  const chartType = (stored.chart_type ?? null) as ChartData["chartType"] | null;
  const route = String(stored.route ?? "");
  return {
    role: stored.role as Message["role"],
    content: String(stored.content ?? ""),
    ...(chartType && rows.length > 0 ? {
      chart: {
        rows, columns, chartType,
        xAxis: (chartMeta?.xAxis as string | null | undefined) ?? null,
        yAxis: (chartMeta?.yAxis as string | null | undefined) ?? null,
        title: (chartMeta?.title as string | undefined) ?? "",
        sql: (stored.sql_query as string | null | undefined) ?? "",
      },
    } : {}),
    ...(BADGED_ROUTES.has(route) ? {
      routeMeta: {
        route: route as RouteMeta["route"],
        lane: meta?.lane as RouteMeta["lane"] | undefined,
        approx: meta?.approx as boolean | undefined,
        datasetName: meta?.datasetName as string | undefined,
        questionClass: meta?.questionClass as string | undefined,
        durationMs: meta?.durationMs as number | undefined,
        evidence: meta?.evidence as Evidence | undefined,
        headline: (meta?.headline as Row | undefined) ?? headlineOf(rows),
      },
    } : {}),
  };
}

/** Compact number format for context hints (3.9M / 12.4K / 1,234). */
function fmtNum(v: number | string | null): string {
  if (v == null) return "—";
  const n = typeof v === "number" ? v : Number(v);
  if (!Number.isFinite(n)) return String(v);
  const a = Math.abs(n);
  if (a >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (a >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return n.toLocaleString();
}

/** Ticking elapsed-seconds readout while a live query runs. */
function LiveTimer({ since }: { since: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 100);
    return () => clearInterval(id);
  }, []);
  const s = Math.max(0, (now - since) / 1000);
  return <span style={{ fontVariantNumeric: "tabular-nums", fontWeight: 700 }}>{s.toFixed(1)}s</span>;
}

const THINKING_PHASES = [
  "Understanding your question",
  "Analyzing the data model",
  "Finding the right approach",
  "Building the query",
];

function ThinkingBubble() {
  const [phase, setPhase] = useState(0);
  const [elapsed, setElapsed] = useState(0);
  const startRef = useRef(Date.now());

  useEffect(() => {
    const timer = setInterval(() => setElapsed(Math.floor((Date.now() - startRef.current) / 1000)), 1000);
    const phaser = setInterval(() => setPhase(p => (p + 1) % THINKING_PHASES.length), 2400);
    return () => { clearInterval(timer); clearInterval(phaser); };
  }, []);

  return (
    <div style={{ padding: "10px 14px", fontSize: 12.5 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 6 }}>
        <div style={{ width: 18, height: 18, borderRadius: 9, background: "linear-gradient(135deg, #4A9EE8, #8b5cf6)", display: "flex", alignItems: "center", justifyContent: "center", animation: "kaveon-breathe 2s ease-in-out infinite" }}>
          <div style={{ width: 6, height: 6, borderRadius: 3, background: "#fff" }} />
        </div>
        <span style={{ color: "var(--text-secondary)", fontWeight: 500, animation: "thinkFade 2.4s ease-in-out infinite" }}>
          {THINKING_PHASES[phase]}
        </span>
        {elapsed > 0 && (
          <span style={{ color: "var(--text-faint)", fontSize: 11, fontVariantNumeric: "tabular-nums", marginLeft: "auto" }}>
            {elapsed}s
          </span>
        )}
      </div>
      <div style={{ width: 120, height: 2, borderRadius: 1, background: "var(--border)", overflow: "hidden" }}>
        <div style={{ width: "40%", height: "100%", background: "linear-gradient(90deg, transparent, #4A9EE8, transparent)", animation: "loadSlide 1.8s ease-in-out infinite" }} />
      </div>
      <style>{`
        @keyframes thinkFade { 0%,100% { opacity:0.6 } 50% { opacity:1 } }
        @keyframes loadSlide { 0% { transform:translateX(-300%) } 100% { transform:translateX(600%) } }
      `}</style>
    </div>
  );
}

interface PageData {
  sourceCount: number;
  tableCount: number;
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

interface ChatSession {
  id: number;
  title: string;
  created_at: string;
  updated_at: string;
}

// ── Constants ──────────────────────────────────────────────────────────────────

const DEFAULT_SUGGESTIONS = [
  "What is current Kaveon usage?",
  "Total queries by plan",
  "Active users by region",
  "Top 10 countries by energy consumption",
];

const EMPTY_SUGGESTIONS = [
  { label: "Connect Fabric SQL", href: "/data-sources" },
  { label: "Connect PostgreSQL", href: "/data-sources" },
  { label: "Connect Azure SQL", href: "/data-sources" },
];

// ── Chat History Sidebar ────────────────────────────────────────────────────────

function ChatHistorySidebar({
  sessions,
  activeSessionId,
  onSelect,
  onDelete,
  onNewChat,
  visible,
  onToggle,
}: {
  sessions: ChatSession[];
  activeSessionId: number | null;
  onSelect: (id: number) => void;
  onDelete: (id: number) => void;
  onNewChat: () => void;
  visible: boolean;
  onToggle: () => void;
}) {
  if (!visible) return null;

  const grouped: Record<string, ChatSession[]> = {};
  const now = new Date();
  for (const s of sessions) {
    const d = new Date(s.updated_at);
    const diffMs = now.getTime() - d.getTime();
    const diffDays = Math.floor(diffMs / 86_400_000);
    const label = diffDays === 0 ? "Today" : diffDays === 1 ? "Yesterday" : diffDays < 7 ? "This week" : diffDays < 30 ? "This month" : "Older";
    (grouped[label] ??= []).push(s);
  }
  const order = ["Today", "Yesterday", "This week", "This month", "Older"];

  return (
    <div style={{
      width: 260, flexShrink: 0, borderRight: "1px solid var(--border)",
      background: "var(--bg-surface)", display: "flex", flexDirection: "column",
      height: "100%", overflow: "hidden",
    }}>
      <div style={{ padding: "16px 14px 10px", display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <span style={{ fontSize: 13, fontWeight: 600, color: "var(--text-secondary)", textTransform: "uppercase", letterSpacing: "0.5px" }}>History</span>
        <div style={{ display: "flex", gap: 6 }}>
          <button onClick={onNewChat} title="New chat" style={{ width: 28, height: 28, borderRadius: 6, border: "1px solid var(--border)", background: "transparent", color: "var(--text-secondary)", cursor: "pointer", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 14 }}>+</button>
          <button onClick={onToggle} title="Close history" style={{ width: 28, height: 28, borderRadius: 6, border: "1px solid var(--border)", background: "transparent", color: "var(--text-secondary)", cursor: "pointer", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 12 }}>
            <i className="fas fa-chevron-left" />
          </button>
        </div>
      </div>

      <div style={{ flex: 1, overflow: "auto", padding: "0 8px 12px" }}>
        {sessions.length === 0 && (
          <p style={{ fontSize: 12, color: "var(--text-faint)", padding: "12px 6px", textAlign: "center" }}>No conversations yet</p>
        )}
        {order.map(label => {
          const items = grouped[label];
          if (!items?.length) return null;
          return (
            <div key={label}>
              <p style={{ fontSize: 10.5, fontWeight: 600, color: "var(--text-faint)", padding: "10px 6px 4px", margin: 0, textTransform: "uppercase", letterSpacing: "0.5px" }}>{label}</p>
              {items.map(s => (
                <div
                  key={s.id}
                  onClick={() => onSelect(s.id)}
                  style={{
                    padding: "8px 10px", borderRadius: 8, cursor: "pointer",
                    background: s.id === activeSessionId ? "rgba(var(--accent-rgb), 0.12)" : "transparent",
                    color: s.id === activeSessionId ? "var(--text-primary)" : "var(--text-secondary)",
                    fontSize: 13, lineHeight: 1.4, marginBottom: 2, display: "flex",
                    alignItems: "center", justifyContent: "space-between", gap: 6,
                    transition: "background 0.1s",
                  }}
                  onMouseEnter={e => { if (s.id !== activeSessionId) e.currentTarget.style.background = "rgba(255,255,255,0.04)"; }}
                  onMouseLeave={e => { if (s.id !== activeSessionId) e.currentTarget.style.background = "transparent"; }}
                >
                  <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>{s.title}</span>
                  <button
                    onClick={e => { e.stopPropagation(); onDelete(s.id); }}
                    title="Delete conversation"
                    style={{ width: 22, height: 22, borderRadius: 4, border: "none", background: "transparent", color: "var(--text-faint)", cursor: "pointer", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 11, opacity: 0.5, flexShrink: 0 }}
                    onMouseEnter={e => { e.currentTarget.style.opacity = "1"; e.currentTarget.style.color = "#ef4444"; }}
                    onMouseLeave={e => { e.currentTarget.style.opacity = "0.5"; e.currentTarget.style.color = "var(--text-faint)"; }}
                  >
                    <i className="fas fa-trash-alt" />
                  </button>
                </div>
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ── Page ───────────────────────────────────────────────────────────────────────

export default function Home() {
  const { account, role } = useAuth();
  const { isSetupOk } = useSetup();
  const canRunLive = role === "Analyst" || role === "Editor" || role === "Admin";

  const [query, setQuery] = useState("");
  const [data, setData] = useState<PageData | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [sending, setSending] = useState(false);
  const [sources, setSources] = useState<SourceOption[]>([]);
  // Every visible dataset's askable shape, read once at mount.
  const catalogueHandle = useDatasetSchemas(isSetupOk === true);
  const { schemas: catalogue, state: catalogueState } = catalogueHandle;
  const datasets: DatasetOption[] = useMemo(
    () => catalogue.map(entry => ({
      id: entry.id,
      name: entry.name,
      database_name: entry.databaseName ?? undefined,
    })),
    [catalogue],
  );
  const [selectedDataset, setSelectedDataset] = useState<number | null>(null);
  const [selectedSource, setSelectedSource] = useState<SourceOption | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const chatAbortRef = useRef<AbortController | null>(null);
  const chatCancelTokenRef = useRef<string | null>(null);
  // The previous DLM answer's frame (dataset, metric, grouping, filters, time).
  // Sent with every question so a follow-up inherits what it does not restate.
  const lastFrame = useRef<Record<string, unknown> | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const { addRecent } = useRecents();

  // A chat question is interactive work. Leaving Chat or closing the tab
  // must release a live Engine statement rather than leaving it scanning for
  // a reader who is no longer present. The token is scoped to this principal
  // by the API and is safe to send with keepalive during page teardown.
  useEffect(() => {
    const release = () => {
      chatAbortRef.current?.abort();
      chatAbortRef.current = null;
      const token = chatCancelTokenRef.current;
      if (!token) return;
      chatCancelTokenRef.current = null;
      void msalFetch(`${API_BASE}/api/v1/sql/engine/cancel/${token}`, {
        method: "DELETE", keepalive: true,
      }).catch(() => {});
    };
    window.addEventListener("pagehide", release);
    return () => {
      window.removeEventListener("pagehide", release);
      release();
    };
  }, []);

  // Chat history state
  const [sessions, setSessions] = useState<ChatSession[]>([]);
  // The demo's live-query quota, shown under the ask box; a self-hosted
  // install has none and shows nothing.
  const demoQuota = useDemoQuota();
  const [activeSessionId, setActiveSessionId] = useState<number | null>(null);
  const [loadingSession, setLoadingSession] = useState(false);

  // Load chat history sessions
  const loadSessions = useCallback(async () => {
    try {
      const res = await msalFetch("/api/v1/chat/history?limit=100");
      if (res.ok) {
        const body = await res.json();
        setSessions(body.sessions || []);
      }
    } catch {
      // History unavailable — tables may not exist yet
    }
  }, []);

  useEffect(() => {
    if (isSetupOk && account?.email) loadSessions();
  }, [isSetupOk, account?.email, loadSessions]);

  // Create a new session for the first message in a conversation
  const ensureSession = useCallback(async (firstMessage: string): Promise<number | null> => {
    try {
      const title = firstMessage.slice(0, 80) || "New conversation";
      const res = await msalFetch("/api/v1/chat/history", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title }),
      });
      if (res.ok) {
        const session = await res.json();
        setActiveSessionId(session.id);
        // Surface the conversation in Recents (single unified surface) with a
        // deep link that resumes this session.
        addRecent({
          id: `chat-${session.id}`,
          label: firstMessage.slice(0, 50) || "New conversation",
          href: `/home?session=${session.id}`,
          type: "chat",
        });
        await loadSessions();
        return session.id;
      }
    } catch {
      // Persistence unavailable
    }
    return null;
  }, [loadSessions, addRecent]);

  // Save a message pair (user + assistant) to the active session
  const saveMessage = useCallback(async (sessionId: number, role: string, content: string, extra?: { sql_query?: string; chart_type?: string; data?: Record<string, unknown>; route?: string }) => {
    try {
      await msalFetch(`/api/v1/chat/history/${sessionId}/messages`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ role, content, ...extra }),
      });
    } catch {
      // Best-effort
    }
  }, []);

  // One turn — the question and the answer — appended in order.
  const saveTurn = useCallback(async (sessionId: number, question: string, answer: string, extra?: { sql_query?: string; chart_type?: string; data?: Record<string, unknown>; route?: string }) => {
    // Appending a message also touches the conversation it belongs to, so two
    // appends to one conversation contend on that record's revision and
    // KaveonDB rejects whichever commit loses. Firing both at once therefore
    // dropped one of every pair, which is why a reopened conversation came
    // back holding the questions and none of the answers. They go in order,
    // and the answer is attempted whatever became of the question.
    await saveMessage(sessionId, "user", question);
    await saveMessage(sessionId, "assistant", answer, extra);
  }, [saveMessage]);

  // Load messages from a past session
  const loadSession = useCallback(async (sessionId: number) => {
    setLoadingSession(true);
    lastFrame.current = null;
    try {
      const res = await msalFetch(`/api/v1/chat/history/${sessionId}`);
      if (!res.ok) return;
      const body = await res.json();
      const withFrame = [...(body.messages || [])].reverse().find((m: Record<string, unknown>) => (m.data as Record<string, unknown> | null)?.frame);
      lastFrame.current = withFrame ? ((withFrame.data as Record<string, unknown>).frame as Record<string, unknown>) : null;
      setMessages((body.messages || []).map(restoreMessage));
      setActiveSessionId(sessionId);
    } catch {
      // Failed to load session
    } finally {
      setLoadingSession(false);
    }
  }, []);

  // Resume a chat deep-linked from Recents (?session=<id>). Reacts to URL changes.
  const searchParams = useSearchParams();
  const sessionParam = searchParams.get("session");
  useEffect(() => {
    if (!sessionParam) return;
    const n = parseInt(sessionParam, 10);
    if (!Number.isNaN(n)) void loadSession(n);
  }, [sessionParam, loadSession]);

  const deleteSession = useCallback(async (sessionId: number) => {
    try {
      await msalFetch(`/api/v1/chat/history/${sessionId}`, { method: "DELETE" });
      setSessions(prev => prev.filter(s => s.id !== sessionId));
      if (activeSessionId === sessionId) {
        setActiveSessionId(null);
        setMessages([]);
        lastFrame.current = null;
      }
    } catch {
      // Best-effort
    }
  }, [activeSessionId]);

  const startNewChat = useCallback(() => {
    // The conversation is already recorded in Recents at session-create time,
    // so just reset the view here.
    setMessages([]);
    setActiveSessionId(null);
    lastFrame.current = null;
  }, []);

  // Listen for "new-chat" event from sidebar nav
  useEffect(() => {
    const handler = () => startNewChat();
    window.addEventListener("kaveon-new-chat", handler);
    return () => window.removeEventListener("kaveon-new-chat", handler);
  }, [startNewChat]);

  const email = account?.email ?? "";
  // A dataset over an Engine table is askable with no registered data source,
  // so the platform is empty only when it holds neither — and only once both
  // reads have answered: a catalogue still in flight is not an absence of data.
  const catalogueSettled = catalogueState !== "loading";
  const isEmpty = data !== null && catalogueSettled
    && data.sourceCount === 0 && datasets.length === 0;
  const inConversation = messages.length > 0;

  // ── Data fetching ────────────────────────────────────────────────────────────

  useEffect(() => {
    if (!isSetupOk || !email) return;
    const headers = { "x-user-email": email };

    // The connection facts only. The dataset catalogue is read once by
    // useDatasetSchemas, in parallel with this, and is the authority for which
    // datasets this user can see and what each one can be asked.
    async function load() {
      try {
        const [listRes, activeRes] = await Promise.all([
          msalFetch("/api/v1/data-sources/list", { headers }),
          msalFetch("/api/v1/data-sources/active", { headers }),
        ]);

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

        setData({ sourceCount: list.length, tableCount });
        setSources(list.map((s: any) => ({ id: s.id, name: s.name, database_name: s.database_name })));
      } catch {
        setData({ sourceCount: 0, tableCount: 0 });
      }
    }

    load();
  }, [isSetupOk, email]);

  // Auto-select the dataset named in the URL when it is visible to this user,
  // else the first — and the source it sits in, once both have arrived.
  useEffect(() => {
    if (datasets.length === 0 || selectedDataset) return;
    const requested = Number(new URLSearchParams(window.location.search).get("dataset"));
    const initial = datasets.find((d) => d.id === requested) ?? datasets[0];
    setSelectedDataset(initial.id);
    const matchSource = sources.find((s) => s.database_name === initial.database_name);
    if (matchSource) setSelectedSource(matchSource);
  }, [datasets, sources, selectedDataset]);

  // The schema of the dataset in the picker, taken from the catalogue that is
  // already in hand rather than re-read one dataset at a time.
  const datasetSchema: DatasetSchema | null =
    catalogue.find(entry => entry.id === selectedDataset)?.schema ?? null;

  // Auto-scroll to the end of the conversation.
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [messages]);

  // Each catalogue entry bound to the source it is queried through. The
  // in-browser fallback parser needs the source to execute what it writes.
  type SchemaEntry = { id: number; name: string; sourceId?: number; sourceName?: string; schema: DatasetSchema };
  const allSchemas: SchemaEntry[] = useMemo(
    () => catalogue.map(entry => {
      const src = sources.find(s => s.database_name === entry.databaseName);
      return {
        id: entry.id,
        name: entry.name,
        sourceId: src?.id,
        sourceName: src?.database_name || entry.databaseName || undefined,
        schema: entry.schema,
      };
    }),
    [catalogue, sources],
  );
  const allSchemasRef = useRef<SchemaEntry[]>([]);
  allSchemasRef.current = allSchemas;
  // Read inside sendMessage without making it depend on a render.
  const catalogueStateRef = useRef(catalogueState);
  catalogueStateRef.current = catalogueState;
  const catalogueSettledRef = useRef(catalogueHandle.settled);
  catalogueSettledRef.current = catalogueHandle.settled;

  // ── Send message ─────────────────────────────────────────────────────────────

  function findBestSchema(text: string) {
    const lower = text.toLowerCase();
    const words = lower.split(/\s+/).filter(w => w.length >= 3);
    let best: { schema: DatasetSchema; datasetId: number; sourceId?: number; sourceName?: string; confidence: number; name: string; parsed: any } | null = null;

    const schemas = allSchemasRef.current;
    for (const ds of schemas) {
      // Score: how well does this dataset match the query?
      let score = 0;

      // Dataset name overlap — weighted heavily so the right dataset wins
      const nameWords = ds.name.toLowerCase().split(/[\s_\-·:]+/).filter(w => w.length >= 3);
      for (const nw of nameWords) {
        if (words.some(w => w === nw || w.includes(nw) || nw.includes(w))) score += 0.5;
      }

      // Column name overlap — require ≥4 char overlap to avoid spurious substring matches
      for (const col of ds.schema.columns) {
        const colWords = col.name.toLowerCase().replace(/_/g, " ").split(/\s+/).filter(w => w.length >= 3);
        for (const cw of colWords) {
          if (words.some(w => (cw.length >= 4 || w.length >= 4) && (w.includes(cw) || cw.includes(w)))) score += 0.15;
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
        best = { schema: ds.schema, datasetId: ds.id, sourceId: ds.sourceId, sourceName: ds.sourceName, confidence: score, name: ds.name, parsed };
      }
    }

    return best;
  }

  // The composer is usable as soon as the platform is known to hold data.
  // The primary path, POST /dlm/ask, routes the question server-side and
  // needs nothing from the browser's catalogue; only the fallback parser
  // below does, and sendMessage awaits the catalogue if it ever gets there
  // first. Blocking every keystroke on the fallback's data was backwards.
  const canSend = !sending && !isEmpty;

  function generateInsight(
    rows: (string | number | null)[][],
    columns: string[],
    parsed: { chartType: string; xAxis: string | null; yAxis: string | null; title: string },
    userQuery: string,
  ): string {
    // Match column by name — SQL may return aliased names (e.g. "avg" for AVG(...))
    const findCol = (name: string | null): number => {
      if (!name) return -1;
      const lower = name.toLowerCase();
      let idx = columns.findIndex(c => c.toLowerCase() === lower);
      if (idx >= 0) return idx;
      // Fuzzy: column starts with or contains the name
      idx = columns.findIndex(c => c.toLowerCase().includes(lower) || lower.includes(c.toLowerCase()));
      if (idx >= 0) return idx;
      return -1;
    };
    const xIdx = findCol(parsed.xAxis) >= 0 ? findCol(parsed.xAxis) : 0;
    const yIdx = findCol(parsed.yAxis) >= 0 ? findCol(parsed.yAxis) : (columns.length > 1 ? 1 : 0);

    const fmt = (v: number): string => {
      if (Math.abs(v) >= 1_000_000_000) return (v / 1_000_000_000).toFixed(1) + "B";
      if (Math.abs(v) >= 1_000_000) return (v / 1_000_000).toFixed(1) + "M";
      if (Math.abs(v) >= 1_000) return (v / 1_000).toFixed(1) + "K";
      return v.toLocaleString();
    };

    // KPI — single value
    if (parsed.chartType === "kpi" && rows.length === 1) {
      const val = Number(rows[0][yIdx >= 0 ? yIdx : 0]);
      const metric = parsed.yAxis || columns[0];
      // Extract context from user query (e.g. "India" from "India energy usage")
      const queryWords = userQuery.toLowerCase().split(/\s+/);
      const metricWords = (metric || "").toLowerCase().replace(/[_()]/g, " ").split(/\s+/);
      const contextWords = queryWords.filter(w => w.length > 2 && !metricWords.some(m => m.includes(w) || w.includes(m)));
      const context = contextWords.length > 0 ? contextWords.map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(" ") : "";
      // Prefer a clean assembled title (the DLM provides one, e.g.
      // "Total Cases — India") over stitching the raw question into the label.
      const label = parsed.title || (context ? `${metric} of ${context}` : metric);
      return `**${label}** is **${fmt(val)}**`;
    }

    // Grouped data — smart listing based on result count
    if (rows.length > 1 && xIdx >= 0 && yIdx >= 0) {
      const sorted = [...rows].sort((a, b) => Number(b[yIdx] || 0) - Number(a[yIdx] || 0));
      const total = rows.length;
      const xLabel = parsed.xAxis || columns[xIdx] || "item";
      const yLabel = parsed.yAxis || columns[yIdx] || "value";

      let insight = "";

      // If <= 20 results (e.g. Top 10, Top 15), list them all
      if (total <= 20) {
        insight = `**${parsed.title || `${yLabel} by ${xLabel}`}** (${total} results)\n\n`;
        sorted.forEach((r, i) => {
          insight += `${i + 1}. **${r[xIdx]}** — ${fmt(Number(r[yIdx] || 0))}\n`;
        });
      } else {
        // Large result set — show top 5 with context
        const top5 = sorted.slice(0, 5).map((r, i) => `${i + 1}. **${r[xIdx]}** — ${fmt(Number(r[yIdx] || 0))}`);
        insight = `Found **${total}** results for ${yLabel} by ${xLabel}.\n\n${top5.join("\n")}\n`;
        insight += `\n*...and ${total - 5} more.* Ask for "top 10" or "top 20" to narrow down.`;
      }

      return insight;
    }

    // Table / fallback
    return `Here are **${rows.length}** results from your data. ${rows.length > 20 ? "Showing first 50 rows." : ""}`;
  }

  // A single row whose only values are null/blank (e.g. SUM over a year with no
  // data) is NOT a real answer — used to decide whether to keep the DLM answer.
  function resultHasData(rows: (string | number | null)[][] | Record<string, unknown>[]): boolean {
    if (!rows || rows.length === 0) return false;
    if (rows.length > 1) return true;
    const row = rows[0] as unknown;
    const vals = Array.isArray(row) ? row : Object.values((row as Record<string, unknown>) ?? {});
    return vals.some(v => v !== null && v !== undefined && v !== "");
  }

  // `resume` re-posts an earlier question with one ambiguous slot pinned; the
  // bubble then shows the option the user picked rather than the question.
  async function sendMessage(text: string, resume?: Clarification["resume"]) {
    if (!text.trim() || !canSend) return;
    const chatAbort = new AbortController();
    chatAbortRef.current?.abort();
    chatAbortRef.current = chatAbort;
    const chatCancelToken = (globalThis.crypto?.randomUUID?.() ?? `${Date.now()}${Math.random()}`)
      .replace(/[^A-Za-z0-9_-]/g, "").slice(0, 64);
    chatCancelTokenRef.current = chatCancelToken;
    const question = resume?.question ?? text.trim();
    const userMsg: Message = { role: "user", content: text.trim() };
    const loadingMsg: Message = { role: "assistant", content: "", loading: true };
    setMessages(prev => [...prev, userMsg, loadingMsg]);
    setQuery("");
    setSending(true);

    // Ensure we have a session — create on first message
    let sid = activeSessionId;
    if (!sid) {
      sid = await ensureSession(text.trim());
    }

    try {
      // Only show charts when user explicitly asks for visualization
      const chartKeywords = /\b(chart|graph|plot|visuali[sz]e|draw|map|heatmap|scatter|bar chart|pie chart|line chart|show me a)\b/i;
      const wantsChart = chartKeywords.test(text.trim());

      // Conversation context: if the user asks a follow-up like "What about India",
      // reuse the previous query's dataset/SQL context
      let queryText = text.trim();
      const followUpPattern = /^(?:what about|how about|and |show me |now |same for )/i;
      // Detect short entity-only follow-ups: 1-3 words, no verb, likely a country/name
      const isShortEntity = /^[A-Za-z]+(?:\s+[A-Za-z]+){0,2}\s*[?.!]?\s*$/i.test(queryText)
        && queryText.replace(/[?.!]/g, "").trim().split(/\s+/).length <= 3
        && !/\b(show|get|what|how|total|top|trend|compare|list|count|average)\b/i.test(queryText)
        && messages.length >= 2;
      const isFollowUp = followUpPattern.test(queryText) || isShortEntity;
      // Find the last user message that was a real query (not a follow-up)
      const prevUser = [...messages].reverse().find(m => m.role === "user" && !followUpPattern.test(m.content) && m.content.split(/\s+/).length > 3);

      if (isFollowUp && prevUser) {
        const entityMatch = followUpPattern.test(queryText)
          ? queryText.match(/(?:what about|how about|and|show me|now|same for)\s+(.+)/i)
          : null;
        const rawEntity = entityMatch
          ? entityMatch[1].replace(/[?.!]$/, "").trim()
          : queryText.replace(/[?.!]$/, "").trim();
        const entity = rawEntity.replace(/^(?:in|for|of)\s+/i, "").trim();
        if (entity) {
          const prev = prevUser.content;
          // Remove any existing entity from previous query (capitalized words that aren't keywords)
          const cleaned = prev.replace(/\b[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\b/g, match => {
            const lower = match.toLowerCase();
            const isKeyword = /^(energy|carbon|total|show|top|get|what|how|consumption|emissions|temperature|renewable|global|trend|arena|benchmark|model)$/i.test(lower);
            return isKeyword ? match : "";
          }).replace(/\s+/g, " ").replace(/\b(in|for|of)\s*$/i, "").trim()
            .replace(/\b(?:by|per|across)\s+\w+/i, "").replace(/\s+/g, " ").trim();
          queryText = cleaned ? `${cleaned} in ${entity}` : `${entity} energy`;
        }
      }

      // ── DLM first — the compiled Data Language Model is the PRIMARY NL→SQL
      // engine: metric-aware, resolves entities from the value index, and handles
      // out-of-range years (answers with the latest available). The in-browser
      // template parser below is only a fallback for shapes the DLM can't cover
      // yet (e.g. time-series trends). A null/empty result doesn't count as a hit.
      try {
        const dlmT0 = performance.now();
        const dlmRes = await msalFetch("/api/v1/dlm/ask", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          signal: chatAbort.signal,
          body: JSON.stringify({ question, choices: resume?.choices, frame: lastFrame.current }),
        });
        if (dlmRes.ok) {
          const dlm = await dlmRes.json();
          if (!dlm?.ok && dlm?.reason === "clarify" && dlm.clarification) {
            const prompt: string = dlm.clarification.prompt;
            if (sid) {
              void saveTurn(sid, text.trim(), prompt, { route: "clarify" });
            }
            setMessages(prev => [...prev.slice(0, -1), {
              role: "assistant",
              content: prompt,
              clarification: { ...dlm.clarification, resume: dlm.resume },
            }]);
            return;
          }
          if (!dlm?.ok && dlm?.reason === "query_failed") {
            const failMsg = `The query could not be completed: ${String(dlm.message || "the Engine refused the statement")}.`;
            if (sid) {
              void saveTurn(sid, text.trim(), failMsg, { sql_query: dlm.sql, route: "error" });
            }
            setMessages(prev => [...prev.slice(0, -1), {
              role: "assistant",
              content: failMsg,
              ...(dlm.evidence ? { routeMeta: { route: "dlm", lane: "live", evidence: dlm.evidence, datasetName: dlm.dataset_name } } : {}),
            }]);
            return;
          }
          // The dataset genuinely cannot answer this. Say so, and offer the
          // closest questions it can answer as one-click follow-ups — the same
          // selectable shape a clarification uses.
          if (!dlm?.ok && dlm?.reason === "unanswerable") {
            const closest: string[] = dlm.closest || [];
            const why = String(dlm.answer || dlm.why || "That question cannot be answered from this dataset.");
            if (sid) {
              void saveTurn(sid, text.trim(), why, { route: "unanswerable" });
            }
            setMessages(prev => [...prev.slice(0, -1), {
              role: "assistant",
              content: why,
              ...(closest.length ? {
                clarification: {
                  kind: "question" as const,
                  prompt: why,
                  options: closest.map(q => ({ id: q, label: q })),
                  resume: { question: "", choices: {} },
                },
              } : {}),
            }]);
            return;
          }
          if (!dlm?.ok && dlm?.reason === "out_of_scope") {
            const names: string[] = dlm.datasets || [];
            const scopeMsg = dlm.hint
              ? String(dlm.hint)
              : names.length === 0
                ? "No datasets are registered yet. Create a dataset in the Library, then return here to ask questions about it."
                : `That question is outside the data Kaveon holds. Ask about one of these datasets: ${names.map(n => `**${n}**`).join(", ")}.`;
            if (sid) {
              void saveTurn(sid, text.trim(), scopeMsg, { route: "out_of_scope" });
            }
            setMessages(prev => [...prev.slice(0, -1), { role: "assistant", content: scopeMsg }]);
            return;
          }
          if (dlm?.ok && (dlm.from_context || dlm.executed || dlm.sql)) {
            if (dlm.frame) lastFrame.current = dlm.frame;
            // Answered from context (no scan), executed on the Engine by the
            // DLM itself (rows and evidence in hand), or a warehouse statement
            // the browser runs now.
            let rows: (string | number | null)[][] = [];
            let columns: string[] = [];
            let got = false;
            const evidence: Evidence | undefined = dlm.evidence;
            if (dlm.from_context || dlm.executed) {
              rows = dlm.rows || [];
              columns = dlm.columns || [];
              got = true;
            } else {
              // Going live — surface it immediately with a running timer AND what we
              // already know from context (single-dim slices), so the user isn't
              // staring at a blank spinner while the exact combo is fetched.
              setMessages(prev => {
                const copy = [...prev];
                const last = copy[copy.length - 1];
                if (last && last.loading) copy[copy.length - 1] = { ...last, liveSince: Date.now(), contextHints: dlm.context_hints || [] };
                return copy;
              });
              // A KaveonDB catalog executes through the Engine plane, scoped to
              // the dataset's schema by the server; external sources keep the pool.
              const execRes = await msalFetch(dlm.engine ? "/api/v1/sql/engine" : "/api/v1/sql/execute", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                signal: chatAbort.signal,
                body: JSON.stringify(dlm.engine
                  ? { sql_text: dlm.sql, database: dlm.database, dataset_id: Number(dlm.dataset_id), source: "chat", cancel_token: chatCancelToken }
                  : { sql_text: dlm.sql, database: dlm.database || "kaveon", source: "chat" }),
              });
              if (execRes.ok) {
                const execData = await execRes.json();
                rows = execData.rows || execData.data || [];
                columns = execData.columns || execData.column_names || [];
                got = true;
                // The warehouse live lane runs here: complete its evidence.
                if (evidence) {
                  evidence.elapsed_ms = typeof execData.duration_ms === "number" ? execData.duration_ms : Math.round(performance.now() - dlmT0);
                  evidence.rows = rows.length;
                  if (execData.query_id) evidence.query_id = String(execData.query_id);
                }
              } else {
                const notice = rateLimitNotice(execRes.status, await execRes.json().catch(() => null));
                if (notice) {
                  demoQuota.refresh();
                  setMessages(prev => [...prev.slice(0, -1), { role: "assistant", content: quotaNotice(notice), notice: true }]);
                  return;
                }
              }
              demoQuota.refresh();
            }
            if (got && (resultHasData(rows) || dlm.note)) {
              const lane: RouteMeta["lane"] = evidence?.lane ?? (dlm.from_context ? "context" : "live");
              const route = lane === "context" ? "context" : "dlm";
              const parsedLike = { sql: dlm.sql, chartType: dlm.chartType, xAxis: dlm.xAxis, yAxis: dlm.yAxis, title: dlm.title, confidence: dlm.confidence ?? 0.5 };
              const insight = generateInsight(rows, columns, parsedLike, question);
              const summary = dlm.note ? `${dlm.note}\n\n${insight}` : insight;
              const answerMeta: RouteMeta = {
                route, lane, evidence, headline: headlineOf(rows),
                durationMs: Math.round(performance.now() - dlmT0),
                approx: !!dlm.approx, datasetName: dlm.dataset_name,
                questionClass: dlm.question_class,
              };
              const answerChart: ChartData | null = wantsChart
                ? { rows, columns, chartType: dlm.chartType, xAxis: dlm.xAxis, yAxis: dlm.yAxis, title: dlm.title, sql: dlm.sql }
                : null;
              if (sid) {
                // The frame rides along in the message's data blob so a reopened
                // session resumes with the same context the DLM last answered in;
                // `meta` carries the rest of the answer, so reopening restores
                // its lane, its evidence and its chart rather than the text alone.
                void saveTurn(sid, text.trim(), summary, {
                  sql_query: dlm.sql,
                  chart_type: wantsChart ? dlm.chartType : undefined,
                  data: {
                    columns, rows: rows.slice(0, 100), row_count: rows.length,
                    ...(dlm.frame ? { frame: dlm.frame } : {}),
                    meta: persistedMeta(answerMeta, answerChart),
                  },
                  route,
                });
              }
              setMessages(prev => [...prev.slice(0, -1), {
                role: "assistant",
                content: summary,
                ...(answerChart ? { chart: answerChart } : {}),
                routeMeta: answerMeta,
              }]);
              return;
            }
          }
        }
      } catch {
        // DLM unavailable — fall through to the in-browser parser
      }

      // The in-browser parser is the only path that needs the dataset
      // catalogue. Reaching it before the one read at mount has settled is
      // possible but rare, so wait on that read here rather than holding every
      // keystroke on the whole page behind it.
      if (catalogueStateRef.current === "loading") await catalogueSettledRef.current();

      // Auto-find best matching dataset
      const match = findBestSchema(queryText);
      const schema = match?.schema || datasetSchema;
      const srcId = match?.sourceId || selectedSource?.id;
      const srcDb = match?.sourceName || selectedSource?.database_name;
      let parsed = match?.parsed || (schema ? nlToSql(queryText, schema) : null);

      // Fallback: if we matched a dataset by name but parser returned null,
      // build a simple SELECT query showing the data
      if (schema && !parsed && match && match.confidence >= 0.2) {
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
        const dbName = srcDb || "kaveon";

        const t0 = performance.now();

        // ── Direct SQL execution ─────────────────────────────────────────────
        try {
          // After the PostgreSQL retirement `/sql/execute` routes a statement
          // naming an Engine catalog to the Engine plane, which resolves the
          // schema from an authorized dataset rather than trusting one the
          // browser sends. Carry the dataset the question was asked against,
          // or this fallback answers "an Engine chart query requires a
          // dataset" for every question.
          const execRes = await msalFetch("/api/v1/sql/execute", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              sql_text: parsed.sql,
              database: dbName,
              // The dataset of the table this statement actually reads, which
              // is the one the question matched — not whatever is selected in
              // the sidebar. The Engine resolves the schema from the dataset,
              // so sending the selected one asked for a table in the wrong
              // schema: "table 'OpenSource.ai_benchmarks.kaveon_events_enriched'
              // not found".
              ...(match?.datasetId ?? selectedDataset
                ? { dataset_id: Number(match?.datasetId ?? selectedDataset) }
                : {}),
              source: "chat",
            }),
          });

          if (execRes.ok) {
            const execData = await execRes.json();
            const rows = execData.rows || execData.data || [];
            const columns = execData.columns || execData.column_names || [];

            if (rows.length > 0) {
              const summary = generateInsight(rows, columns, parsed, text.trim());
              const directMeta: RouteMeta = {
                route: "direct", durationMs: Math.round(performance.now() - t0),
                headline: headlineOf(rows),
              };
              const directChart: ChartData | null = wantsChart
                ? { rows, columns, chartType: parsed.chartType, xAxis: parsed.xAxis, yAxis: parsed.yAxis, title: parsed.title, sql: parsed.sql }
                : null;
              if (sid) {
                void saveTurn(sid, text.trim(), summary, {
                  sql_query: parsed.sql,
                  chart_type: wantsChart ? parsed.chartType : undefined,
                  data: {
                    columns, rows: rows.slice(0, 100), row_count: rows.length,
                    meta: persistedMeta(directMeta, directChart),
                  },
                  route: "direct",
                });
              }
              setMessages(prev => [...prev.slice(0, -1), {
                role: "assistant",
                content: summary,
                ...(directChart ? { chart: directChart } : {}),
                routeMeta: directMeta,
              }]);
              return;
            }

            const noResultMsg = `The query completed successfully but returned no rows.\n\nSQL: \`${parsed.sql}\``;
            if (sid) {
              void saveTurn(sid, text.trim(), noResultMsg, { sql_query: parsed.sql, route: "direct" });
            }
            setMessages(prev => [...prev.slice(0, -1), {
              role: "assistant",
              content: noResultMsg,
            }]);
            return;
          } else {
            const errText = await execRes.text().catch(() => "");
            const notice = rateLimitNotice(execRes.status, (() => { try { return JSON.parse(errText); } catch { return null; } })());
            if (notice) {
              demoQuota.refresh();
              setMessages(prev => [...prev.slice(0, -1), { role: "assistant", content: quotaNotice(notice), notice: true }]);
              return;
            }
            const errMsg = `The query could not be completed (status ${execRes.status}).\n\nSQL: \`${parsed.sql}\`\n\n${errText.substring(0, 200)}`;
            if (sid) {
              void saveTurn(sid, text.trim(), errMsg, { sql_query: parsed.sql, route: "error" });
            }
            setMessages(prev => [...prev.slice(0, -1), {
              role: "assistant",
              content: errMsg,
            }]);
            return;
          }
        } catch (execErr) {
          const errMsg = `The query could not be executed: ${execErr instanceof Error ? execErr.message : "an unexpected error occurred"}.\n\nSQL: \`${parsed.sql}\``;
          if (sid) {
            void saveTurn(sid, text.trim(), errMsg, { route: "error" });
          }
          setMessages(prev => [...prev.slice(0, -1), {
            role: "assistant",
            content: errMsg,
          }]);
          return;
        }
      }

      // Neither the DLM (tried first) nor the in-browser parser could answer —
      // show helpful suggestions.
      const schemasNow = allSchemasRef.current;
      const availableDatasets = schemasNow.map(s => s.name).join(", ");
      const fallbackMsg = schemasNow.length === 0
        ? "No datasets are available yet. Please create a dataset in the Workspace, then return here to ask questions about your data."
        : `I wasn't able to match that request to your data. The following datasets are available: **${availableDatasets}**\n\nYou might try, for example:\n• "Show [metric] by [column]"\n• "Top 10 [column] by [metric]"\n• "Total [metric]"\n• "Trend of [metric] over time"`;
      if (sid) {
        void saveTurn(sid, text.trim(), fallbackMsg, { route: "no_match" });
      }
      setMessages(prev => [...prev.slice(0, -1), {
        role: "assistant",
        content: fallbackMsg,
      }]);
    } catch (e) {
      const errMsg = `Something went wrong. ${e instanceof Error ? e.message : "Please try again."}`;
      if (sid) {
        void saveTurn(sid, text.trim(), errMsg, { route: "error" });
      }
      setMessages(prev => [...prev.slice(0, -1), {
        role: "assistant",
        content: errMsg,
      }]);
    } finally {
      if (chatAbortRef.current === chatAbort) chatAbortRef.current = null;
      if (chatCancelTokenRef.current === chatCancelToken) chatCancelTokenRef.current = null;
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
    : "Talk to your data...";

  // Build meta line — only show positive counts
  let metaParts: string[] = [];
  if (data && !isEmpty) {
    if (data.sourceCount > 0) metaParts.push(`${data.sourceCount} source${data.sourceCount !== 1 ? "s" : ""}`);
    if (data.tableCount > 0) metaParts.push(`${data.tableCount} table${data.tableCount !== 1 ? "s" : ""}`);
    const dsCount = datasets.length;
    if (dsCount > 0) metaParts.push(`${dsCount} dataset${dsCount !== 1 ? "s" : ""}`);
  }
  const metaLine = metaParts.length > 0 ? metaParts.join(" · ") : null;

  return (
    <div
      style={{
        position: "relative",
        height: "100dvh",
        display: "flex",
        background: "var(--bg-primary)",
      }}
    >
      {/* Main content — chat history lives in Recents now, no separate sidebar.
          The column is exactly as tall as the viewport so the transcript below
          is the element that scrolls: the app shell's main column is an
          `overflow: auto` box whose height is its own content, which makes it a
          scroll container that never scrolls. Without a definite height here
          the window scrolled instead and carried the composer off screen. */}
      <div style={{ flex: 1, display: "flex", flexDirection: "column", minWidth: 0, height: "100%" }}>

        {/* Available-context banner — what's compiled & testable (date ranges, values) */}
        <ContextBanner />

        {/* Hero section — collapses when in conversation */}
        {!inConversation && !loadingSession && (
          <div
            style={{
              flex: 1,
              minHeight: 0,
              overflowY: "auto",
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              justifyContent: "center",
              position: "relative",
              paddingBottom: "8vh",
            }}
          >
            <div style={{ position: "relative", zIndex: 1, display: "flex", flexDirection: "column", alignItems: "center", gap: "1rem", width: "100%", maxWidth: 680, padding: "0 1.5rem" }}>
              {/* Greeting — with Guardian O inline like Claude's */}
              <h1 style={{ margin: "0 0 8px", fontSize: "clamp(22px, 5vw, 32px)", fontWeight: 500, color: "var(--text-primary)", textAlign: "center", letterSpacing: "-0.5px", display: "flex", alignItems: "center", justifyContent: "center", gap: 8 }}>
                <KaveonMark size={52} useDirectColor />
                {isEmpty ? heroText : `${new Date().getHours() < 12 ? "Morning" : new Date().getHours() < 17 ? "Afternoon" : "Evening"}, ${(account?.name || "there").replace(/\w\S*/g, w => w[0].toUpperCase() + w.slice(1).toLowerCase())}`}
              </h1>


              {/* Input */}
              <div style={{ width: "100%", maxWidth: 640, background: "var(--bg-surface)", border: "1px solid var(--border)", borderRadius: 16, padding: "16px", boxShadow: "0 0 0 1px rgba(var(--accent-rgb), 0.06), var(--shadow-lg)", display: "flex", flexDirection: "column", gap: 10 }}>
                <textarea
                  ref={inputRef as any}
                  value={query}
                  onChange={e => setQuery(e.target.value)}
                  onKeyDown={e => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }}
                  placeholder={placeholder}
                  rows={2}
                  style={{ width: "100%", border: "none", outline: "none", background: "transparent", color: "var(--text-primary)", fontSize: 15, lineHeight: 1.5, resize: "none", fontFamily: "inherit" }}
                />
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
                  <span style={{ fontSize: 11.5, color: demoQuota.quota?.remaining === 0 ? "var(--warning)" : "var(--text-muted)", minHeight: 16 }}
                    title={demoQuota.label ? "This demo allows a fixed number of live reads per rolling window; answers from precomputed context and the result cache do not count." : undefined}>
                    {demoQuota.label ?? ""}
                  </span>
                  <button onClick={submit} disabled={!query.trim() || !canSend} style={{ width: 34, height: 34, borderRadius: 10, border: "none", background: query.trim() && canSend ? "var(--accent)" : "var(--bg-hover)", color: query.trim() && canSend ? "#fff" : "var(--text-muted)", cursor: query.trim() && canSend ? "pointer" : "default", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 16, transition: "all 0.15s" }}>
                    ↑
                  </button>
                </div>
              </div>

              {/* Suggestions */}
              <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem", justifyContent: "center" }}>
                {isEmpty
                  ? EMPTY_SUGGESTIONS.map(s => (
                      <a key={s.label} href={s.href} style={{ padding: "6px 14px", borderRadius: 999, border: "1px solid var(--border)", background: "var(--bg-surface)", color: "var(--text-secondary)", fontSize: 13, textDecoration: "none", boxShadow: "var(--shadow-sm)" }}>{s.label}</a>
                    ))
                  : DEFAULT_SUGGESTIONS.map(s => (
                      <button key={s} onClick={() => { if (canSend) { setQuery(s); setTimeout(() => void sendMessage(s), 50); } }}
                        style={{ padding: "9px 18px", borderRadius: 999, border: "1.5px solid var(--border)", background: "var(--bg-surface)", color: "var(--text-secondary)", fontSize: 13.5, fontWeight: 500, cursor: "pointer", boxShadow: "var(--shadow-md)", transition: "all 0.15s" }}
                        onMouseEnter={(e) => { e.currentTarget.style.borderColor = "rgba(var(--accent-rgb), 0.4)"; e.currentTarget.style.color = "var(--text-primary)"; e.currentTarget.style.background = "var(--bg-elevated)"; }}
                        onMouseLeave={(e) => { e.currentTarget.style.borderColor = "var(--border)"; e.currentTarget.style.color = "var(--text-secondary)"; e.currentTarget.style.background = "var(--bg-surface)"; }}>
                        {s}
                      </button>
                    ))}
              </div>
            </div>
          </div>
        )}

        {/* Conversation view — appears after first message */}
        {loadingSession && <KaveonLoading message="Loading conversation" />}

        {inConversation && !loadingSession && (
          <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>

            {/* The transcript scrolls under a soft fade; the composer below it
                never moves. The fade is painted over the foot of the scroller
                rather than inside it, so a message passing beneath the composer
                dissolves into the page ground instead of being cut at a hard
                edge, and it takes no pointer events so the wheel still reaches
                the transcript under it. */}
            <div style={{ position: "relative", flex: 1, minHeight: 0, display: "flex" }}>
              <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "32px 24px", display: "flex", flexDirection: "column", gap: 16 }}>
                {messages.map((m, i) => (
                  <div key={i} style={{ display: "flex", flexDirection: m.role === "user" ? "row-reverse" : "row", gap: 10, alignItems: "flex-start" }}>
                    {/* Avatar */}
                    <div style={{
                      width: 28, height: 28, borderRadius: "50%", flexShrink: 0,
                      display: "flex", alignItems: "center", justifyContent: "center",
                      background: m.role === "user" ? "var(--accent)" : "transparent",
                      fontSize: 12, fontWeight: 600, color: m.role === "user" ? "#fff" : "var(--text-secondary)",
                    }}>
                      {m.role === "user" ? (account?.name?.[0] ?? "U") : <KaveonMark size={22} useDirectColor />}
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
                        m.liveSince ? (
                          <div style={{ padding: "8px 14px", fontSize: 12.5, color: "var(--text-secondary)" }}>
                            {m.contextHints && m.contextHints.length > 0 && (
                              <div style={{ marginBottom: 8 }}>
                                <div style={{ display: "inline-flex", alignItems: "center", gap: 5, padding: "2px 9px", borderRadius: 999, background: "rgba(16,185,129,0.1)", color: "#10b981", fontSize: 11, fontWeight: 600, marginBottom: 6 }}>
                                  <i className="fas fa-bolt" style={{ fontSize: 8 }} /> From context
                                </div>
                                <div style={{ color: "var(--text-muted)", marginBottom: 4 }}>What we already know, instantly:</div>
                                {m.contextHints.map((h, i) => (
                                  <div key={i} style={{ display: "flex", justifyContent: "space-between", gap: 16, padding: "2px 0", maxWidth: 320 }}>
                                    <span>{h.label}</span><strong style={{ color: "var(--text-primary)" }}>{fmtNum(h.value)}</strong>
                                  </div>
                                ))}
                              </div>
                            )}
                            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                              <span style={{ display: "inline-flex", alignItems: "center", gap: 5, padding: "2px 9px", borderRadius: 999, background: "rgba(74,158,232,0.12)", color: "#4A9EE8", fontSize: 11, fontWeight: 600 }}>
                                <i className="fas fa-database" style={{ fontSize: 8 }} /> Live query
                              </span>
                              <span>fetching the exact figure&hellip; <LiveTimer since={m.liveSince} /></span>
                            </div>
                          </div>
                        ) : (
                          <ThinkingBubble />
                        )
                      ) : (
                        <>
                          {m.notice && (
                            <div style={{ display: "inline-flex", alignItems: "center", gap: 5, padding: "2px 9px", borderRadius: 999, background: "rgba(217,119,6,0.12)", color: "#d97706", fontSize: 11, fontWeight: 600, marginBottom: 6 }}>
                              <i className="fas fa-hourglass-half" style={{ fontSize: 8 }} /> Live-query quota
                            </div>
                          )}
                          {m.content && (
                            <div style={{ padding: m.chart ? "0 0 8px" : 0, whiteSpace: "pre-wrap", lineHeight: 1.6, color: m.notice ? "var(--text-secondary)" : undefined }}
                              dangerouslySetInnerHTML={{ __html: renderAssistantHtml(m.content) }}
                            />
                          )}
                          {m.clarification && (
                            <div style={{ display: "flex", flexWrap: "wrap", gap: 8, marginTop: 10 }}>
                              {m.clarification.options.map(opt => {
                                const picked = m.chosen === opt.id;
                                const settled = m.chosen != null;
                                return (
                                  <button
                                    key={opt.id}
                                    type="button"
                                    disabled={settled || sending}
                                    title={opt.description || undefined}
                                    onClick={() => {
                                      const c = m.clarification!;
                                      setMessages(prev => prev.map((x, j) => (j === i ? { ...x, chosen: opt.id } : x)));
                                      if (c.kind === "question") {
                                        void sendMessage(opt.label);   // a whole question, asked afresh
                                      } else {
                                        void sendMessage(opt.label, { question: c.resume.question, choices: { ...c.resume.choices, [c.kind]: opt.id } });
                                      }
                                    }}
                                    style={{
                                      padding: "6px 12px", borderRadius: 8, fontSize: 12.5, fontWeight: 500,
                                      cursor: settled ? "default" : "pointer",
                                      border: `1px solid ${picked ? "var(--accent)" : "var(--border)"}`,
                                      background: picked ? "rgba(var(--accent-rgb), 0.12)" : "var(--bg-surface)",
                                      color: picked ? "var(--accent)" : settled ? "var(--text-faint)" : "var(--text-primary)",
                                    }}
                                  >
                                    {opt.label}
                                  </button>
                                );
                              })}
                            </div>
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
                          {m.routeMeta && (
                            <div style={{ display: "flex", alignItems: "center", gap: 6, marginTop: 6, fontSize: 10.5, color: "var(--text-faint)" }}>
                              <span style={{
                                display: "inline-flex", alignItems: "center", gap: 4,
                                padding: "2px 8px", borderRadius: 10,
                                background: m.routeMeta.route === "context" ? "rgba(16,185,129,0.1)" : m.routeMeta.route === "direct" ? "rgba(255,255,255,0.04)" : "rgba(74,158,232,0.1)",
                                color: m.routeMeta.route === "context" ? "#10b981" : m.routeMeta.route === "direct" ? "var(--text-faint)" : "#4A9EE8",
                                fontWeight: 600,
                              }}>
                                <i className={`fas ${m.routeMeta.route === "context" ? "fa-bolt" : m.routeMeta.route === "direct" ? "fa-database" : "fa-route"}`} style={{ fontSize: 8 }} />
                                {m.routeMeta.route === "context" ? (m.routeMeta.approx ? "From sketch" : "From context") : m.routeMeta.route === "direct" ? "Live query" : m.routeMeta.route === "hybrid" ? "Hybrid" : m.routeMeta.lane === "cache" ? "From cache" : "Live query"}
                              </span>
                              {m.routeMeta.durationMs != null && <span>{m.routeMeta.durationMs >= 1000 ? (m.routeMeta.durationMs / 1000).toFixed(1) + "s" : m.routeMeta.durationMs + "ms"}</span>}
                              {m.routeMeta.route === "context" && !m.routeMeta.approx && <span style={{ color: "#10b981" }}>&middot; no scan</span>}
                              {m.routeMeta.route === "context" && m.routeMeta.approx && <span style={{ color: "#10b981" }} title="Sketch estimate with its error stated in the evidence; no scan">&middot; ≈ estimate &middot; no scan</span>}
                              {m.routeMeta.datasetName && <span>&middot; {m.routeMeta.datasetName}</span>}
                              {m.routeMeta.questionClass && (
                                <span title="The question class the DLM answered as">
                                  &middot; {m.routeMeta.questionClass.replace(/_/g, " ")}
                                </span>
                              )}
                            </div>
                          )}
                          {m.routeMeta?.evidence && (
                            <EvidencePanel evidence={m.routeMeta.evidence} headline={m.routeMeta.headline ?? null} canRunLive={canRunLive} />
                          )}
                        </>
                      )}
                    </div>
                  </div>
                ))}
                <div ref={bottomRef} />
              </div>
              <div
                aria-hidden
                style={{
                  position: "absolute", left: 0, right: 0, bottom: 0, height: 28,
                  pointerEvents: "none",
                  background: "linear-gradient(to bottom, transparent, var(--bg-primary))",
                }}
              />
            </div>

            {/* Composer — on the page ground below the transcript, so it holds
                its place however far the conversation is scrolled and however
                short the window is. */}
            <div style={{ flexShrink: 0, padding: "0 24px 16px" }}>
              <div style={{ maxWidth: 700, margin: "0 auto", background: "var(--bg-surface)", border: "1px solid var(--border)", borderRadius: 16, padding: "16px", boxShadow: "0 0 0 1px rgba(var(--accent-rgb), 0.06), var(--shadow-lg)", display: "flex", flexDirection: "column", gap: 10 }}>
                <textarea
                  ref={inputRef as any}
                  value={query}
                  onChange={e => setQuery(e.target.value)}
                  onKeyDown={e => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }}
                  placeholder="Ask anything..."
                  rows={2}
                  style={{ width: "100%", border: "none", outline: "none", background: "transparent", color: "var(--text-primary)", fontSize: 15, lineHeight: 1.5, resize: "none", fontFamily: "inherit" }}
                />
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
                  <span style={{ fontSize: 11.5, color: demoQuota.quota?.remaining === 0 ? "var(--warning)" : "var(--text-muted)", minHeight: 16 }}
                    title={demoQuota.label ? "This demo allows a fixed number of live reads per rolling window; answers from precomputed context and the result cache do not count." : undefined}>
                    {demoQuota.label ?? ""}
                  </span>
                  <button onClick={submit} disabled={!query.trim() || !canSend} style={{ width: 34, height: 34, borderRadius: 10, border: "none", background: query.trim() && canSend ? "var(--accent)" : "var(--bg-hover)", color: query.trim() && canSend ? "#fff" : "var(--text-muted)", cursor: query.trim() && canSend ? "pointer" : "default", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 16, transition: "all 0.15s" }}>
                    ↑
                  </button>
                </div>
              </div>
              <p style={{ textAlign: "center", fontSize: 11, color: "var(--text-faint)", margin: "6px 0 0" }}>Kaveon generates SQL from your questions. Always verify queries before running in production.</p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
