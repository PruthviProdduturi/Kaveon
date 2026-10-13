"use client";

import { useEffect, useState } from "react";
import { msalFetch } from "../utils/msalFetch";
import { UnknownLine } from "./Reading";

/**
 * Fixed "available context" banner — shows which datasets have a compiled DLM,
 * the date range each covers, row counts, and how many filter values are
 * indexed. Lets users see what they can ask about before they start testing.
 *
 * Reads GET /api/v1/dlm/coverage (no-LLM compiled artifacts). While that read
 * is in flight the strip holds its place and says so, rather than appearing
 * from nowhere and pushing the page down under the reader's cursor; it shows a
 * hint when no DLM has been generated yet, and is dismissible either way.
 *
 * A read that did not answer is not a hint. Both failure paths here used to
 * end in `setItems([])`, and an empty list is what draws "No context compiled
 * yet — open a dataset and Generate DLM to make it queryable here": with the
 * Engine down and eight of nine datasets compiled, this strip told every
 * reader to go and compile what they already had. The two outcomes are now
 * held apart — the strip reports an unknown as an unknown, and only a server
 * that answered with no compiled context draws the hint (D-26).
 */

interface DateRange {
  column?: string | null;
  min?: string | null;
  max?: string | null;
  approx?: boolean;
}

interface DimInfo { column?: string | null; values?: string[]; }

interface CoverageItem {
  dataset_id: string;
  name?: string | null;
  date_column?: string | null;
  date_range?: DateRange | null;
  row_count?: number | null;
  values_indexed?: number;
  columns_count?: number;
  dimensions?: DimInfo[];
  metrics?: string[];
  status?: string;
  built_at?: string;
}

function compact(n: number | null | undefined): string {
  if (n == null) return "—";
  const a = Math.abs(n);
  if (a >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + "B";
  if (a >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (a >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
}

function shortDate(s?: string | null): string | null {
  if (!s) return null;
  // trust the first 10 chars for ISO-ish dates; otherwise show as-is (trimmed)
  const m = /^(\d{4}-\d{2}-\d{2})/.exec(String(s).trim());
  return m ? m[1] : String(s).trim().slice(0, 12);
}

const PREF_KEY = "kaveon-context-banner";

export function ContextBanner() {
  const [items, setItems] = useState<CoverageItem[] | null>(null);
  /** Set only when the read itself did not answer; never when it answered with nothing. */
  const [unread, setUnread] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState(() => {
    if (typeof window === "undefined") return false;
    return localStorage.getItem(PREF_KEY) === "hidden";
  });
  const [hover, setHover] = useState<{ item: CoverageItem; x: number; y: number } | null>(null);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const res = await msalFetch("/api/v1/dlm/coverage");
        if (!res.ok) {
          // A refusal and a restart are different facts, and the status is the
          // only thing that separates them here. Neither is an empty list.
          if (alive) {
            setUnread(res.status === 403
              ? "Your role does not include reading compiled context."
              : res.status >= 500
                ? `The server answered ${res.status}.`
                : `The request was answered with ${res.status}.`);
          }
          return;
        }
        const data = await res.json();
        // A body that is not a dataset list is not an empty dataset list.
        if (!alive) return;
        if (Array.isArray(data?.datasets)) setItems(data.datasets);
        else setUnread("The server answered with something other than a coverage listing.");
      } catch {
        if (alive) setUnread("The Kaveon API could not be reached from this browser.");
      }
    })();
    return () => { alive = false; };
  }, []);

  if (dismissed) return null;

  const ready = (items ?? []).filter((d) => d.status === "ready" || (d.values_indexed ?? 0) > 0 || d.date_range);

  const wrap: React.CSSProperties = {
    position: "sticky", top: 0, zIndex: 20,
    display: "flex", flexWrap: "wrap", alignItems: "center", justifyContent: "center",
    gap: 8,
    padding: "8px 44px",
    width: "100%", maxWidth: "100%", boxSizing: "border-box",
    background: "var(--bg-surface)",
    borderBottom: "1px solid var(--border)",
    fontSize: 12.5, color: "var(--text-secondary)",
  };
  const label: React.CSSProperties = {
    display: "flex", alignItems: "center", gap: 6,
    fontWeight: 600, color: "var(--text-primary)", flexShrink: 0,
  };
  const pill: React.CSSProperties = {
    display: "inline-flex", alignItems: "center", gap: 8,
    padding: "3px 10px", borderRadius: 999,
    border: "1px solid var(--border)", background: "rgba(var(--accent-rgb),0.06)",
    flexShrink: 0, whiteSpace: "nowrap", maxWidth: "100%",
  };
  const dot = (ok: boolean): React.CSSProperties => ({
    width: 6, height: 6, borderRadius: 999,
    background: ok ? "#22c55e" : "#f59e0b", flexShrink: 0,
  });
  const closeBtn: React.CSSProperties = {
    position: "absolute", right: 10, top: 8,
    flexShrink: 0,
    border: "none", background: "transparent", cursor: "pointer",
    color: "var(--text-secondary)", fontSize: 13, padding: 4,
  };

  // The read did not answer. What is compiled is unknown, and an unknown is
  // reported as one: no count, no claim that nothing exists, and no
  // instruction to compile context that may well already be there.
  if (unread) {
    return (
      <div style={wrap}>
        <span style={label}><i className="fas fa-database" /> Context</span>
        <UnknownLine
          what="Compiled context" says="which datasets can be asked about here"
          detail={unread}
        />
        <button style={closeBtn} title="Dismiss" onClick={() => { setDismissed(true); localStorage.setItem(PREF_KEY, "hidden"); }}>
          <i className="fas fa-times" />
        </button>
      </div>
    );
  }

  if (items === null) {
    return (
      <div style={wrap} aria-busy="true">
        <span style={label}><i className="fas fa-database" /> Context</span>
        <span style={{ ...pill, color: "var(--text-secondary)" }}>
          <span style={{ ...dot(false), background: "var(--text-muted)" }} />
          Reading compiled context
        </span>
      </div>
    );
  }

  // Reached only when the server answered and reported nothing compiled.
  if (ready.length === 0) {
    return (
      <div style={wrap}>
        <span style={label}><i className="fas fa-database" /> Context</span>
        <span>No context compiled yet — open a dataset and <b>Generate DLM</b> to make it queryable here.</span>
        <button style={closeBtn} title="Dismiss" onClick={() => { setDismissed(true); localStorage.setItem(PREF_KEY, "hidden"); }}>
          <i className="fas fa-times" />
        </button>
      </div>
    );
  }

  return (
    <div style={wrap}>
      <span style={label}>
        <i className="fas fa-database" /> Context ready
      </span>
      {ready.map((d) => {
        return (
          <span
            key={d.dataset_id}
            style={{ ...pill, cursor: "help" }}
            onMouseEnter={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              setHover({ item: d, x: r.left, y: r.bottom + 6 });
            }}
            onMouseLeave={() => setHover(null)}
          >
            <span style={dot(d.status === "ready")} />
            {/* The strip says what can be asked about, in as few words as it
                takes: the dataset, how much of it there is, and how much of it
                is answerable by value. A date range is a property of the data
                rather than of what you can ask, and it pushed the two numbers
                that do matter off the end of the row. It is still on the
                hover card for anyone who wants it. */}
            <b style={{ color: "var(--text-primary)" }}>{d.name ?? `#${d.dataset_id}`}</b>
            <span>· {compact(d.row_count)} rows</span>
            {(d.values_indexed ?? 0) > 0 && <span>· {compact(d.values_indexed)} values</span>}
          </span>
        );
      })}
      <button style={closeBtn} title="Dismiss" onClick={() => { setDismissed(true); localStorage.setItem(PREF_KEY, "hidden"); }}>
        <i className="fas fa-times" />
      </button>

      {hover && <HoverCard item={hover.item} x={hover.x} y={hover.y} />}
    </div>
  );
}

function HoverCard({ item, x, y }: { item: CoverageItem; x: number; y: number }) {
  const dr = item.date_range;
  const dims = (item.dimensions ?? []).filter((d) => (d.values?.length ?? 0) > 0);
  const card: React.CSSProperties = {
    position: "fixed", left: Math.min(x, (typeof window !== "undefined" ? window.innerWidth : 1200) - 340), top: y,
    zIndex: 60, width: 320, maxWidth: "92vw",
    background: "var(--bg-elevated, #1a1a1a)", border: "1px solid var(--border)",
    borderRadius: 10, boxShadow: "0 12px 32px rgba(0,0,0,0.4)",
    padding: 12, fontSize: 12, color: "var(--text-secondary)",
    textAlign: "left", whiteSpace: "normal", cursor: "default",
  };
  const chip: React.CSSProperties = {
    display: "inline-block", padding: "1px 7px", borderRadius: 999, marginRight: 4, marginBottom: 4,
    background: "rgba(var(--accent-rgb),0.1)", color: "var(--text-primary)", fontSize: 11,
  };
  const head: React.CSSProperties = { fontSize: 10.5, fontWeight: 700, textTransform: "uppercase", letterSpacing: 0.4, color: "var(--text-faint)", margin: "10px 0 4px" };
  return (
    <div style={card}>
      <div style={{ fontWeight: 700, color: "var(--text-primary)", fontSize: 13 }}>{item.name}</div>
      <div style={{ marginTop: 3, color: "var(--text-faint)" }}>
        {compact(item.row_count)} rows · {item.columns_count ?? "—"} columns · {compact(item.values_indexed)} indexed values
        {dr?.min && dr?.max ? ` · ${shortDate(dr.min)} → ${shortDate(dr.max)}` : ""}
      </div>

      {dims.length > 0 && (
        <>
          <div style={head}>Filter by</div>
          {dims.map((d) => (
            <div key={d.column} style={{ marginBottom: 5 }}>
              <span style={{ color: "var(--text-primary)", fontWeight: 600 }}>{d.column}: </span>
              <span>{(d.values ?? []).slice(0, 6).join(", ")}{(d.values?.length ?? 0) >= 6 ? "…" : ""}</span>
            </div>
          ))}
        </>
      )}

      {(item.metrics?.length ?? 0) > 0 && (
        <>
          <div style={head}>Metrics</div>
          <div>{item.metrics!.map((m) => <span key={m} style={chip}>{m}</span>)}</div>
        </>
      )}
    </div>
  );
}
