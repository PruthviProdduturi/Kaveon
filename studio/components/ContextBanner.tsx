"use client";

import { useEffect, useState } from "react";
import { msalFetch } from "../utils/msalFetch";
import { refusalMessage } from "../utils/demoQuota";
import { UnknownLine, reasonFromStatus, type ReadReason } from "./Reading";

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

/**
 * Where this preference lives, exported so there is one spelling of it.
 *
 * The same string was written out again in the preferences page, which is the
 * only other thing that reads or writes it — two literals for one key, either
 * of which could be corrected without the other and silently decouple the
 * switch from the strip it controls. Importing it is now the only way to name
 * it; see the note filed with this change for the duplicate still standing in
 * `app/settings/preferences/page.tsx`, which is outside this sweep's files.
 */
export const CONTEXT_BANNER_PREF_KEY = "kaveon-context-banner";

/** A read that did not answer, kept apart from a read that answered with nothing. */
interface Unread { reason: ReadReason | null; detail: string }

function readDismissed(): boolean {
  if (typeof window === "undefined") return false;
  try { return localStorage.getItem(CONTEXT_BANNER_PREF_KEY) === "hidden"; } catch { return false; }
}

export function ContextBanner() {
  const [items, setItems] = useState<CoverageItem[] | null>(null);
  /** Set only when the read itself did not answer; never when it answered with nothing. */
  const [unread, setUnread] = useState<Unread | null>(null);
  const [dismissed, setDismissed] = useState(readDismissed);
  const [hover, setHover] = useState<{ item: CoverageItem; x: number; y: number } | null>(null);

  // The preference was read once, at mount, and never again — so a browser
  // with this page open in one tab and Preferences in another kept showing a
  // strip the reader had just switched off, until that tab was reloaded.
  // `storage` is the event the other document's write raises; a window
  // regaining focus re-reads as well, which covers a write this document did
  // not hear about.
  useEffect(() => {
    const reread = () => setDismissed(readDismissed());
    const onStorage = (event: StorageEvent) => {
      if (event.key === null || event.key === CONTEXT_BANNER_PREF_KEY) reread();
    };
    window.addEventListener("storage", onStorage);
    window.addEventListener("focus", reread);
    return () => {
      window.removeEventListener("storage", onStorage);
      window.removeEventListener("focus", reread);
    };
  }, []);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const res = await msalFetch("/api/v1/dlm/coverage");
        const body: unknown = await res.json().catch(() => null);
        if (!alive) return;
        if (!res.ok) {
          // A refusal and a restart are different facts, and neither is an
          // empty list. What the server itself said is preferred over a
          // sentence written here: this used to tell a reader their role did
          // not permit the read on any 403, including the ones the API
          // explains, and threw that explanation away.
          setUnread({
            reason: reasonFromStatus(res.status),
            detail: refusalMessage(body, `The server answered ${res.status}.`),
          });
          return;
        }
        // A body that is not a dataset list is not an empty dataset list.
        const datasets = (body as { datasets?: unknown } | null)?.datasets;
        if (Array.isArray(datasets)) setItems(datasets as CoverageItem[]);
        else setUnread({ reason: "invalid_listing", detail: "" });
      } catch {
        if (alive) setUnread({ reason: null, detail: "The Kaveon API could not be reached from this browser." });
      }
    })();
    return () => { alive = false; };
  }, []);

  const dismiss = () => {
    setDismissed(true);
    try { localStorage.setItem(CONTEXT_BANNER_PREF_KEY, "hidden"); } catch { /* preference is best effort */ }
  };

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
  // The literals were the light theme's green and amber, so the only part of
  // this strip that carries a state did not follow the theme. The status tokens
  // are defined for both grounds; the word beside each dot, on the pill's
  // hover card, is what actually reports the state, since a 6px dot cannot.
  const dot = (ok: boolean): React.CSSProperties => ({
    width: 6, height: 6, borderRadius: 999,
    background: ok ? "var(--success)" : "var(--warning)", flexShrink: 0,
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
          reason={unread.reason}
          detail={unread.detail || null}
        />
        <button type="button" style={closeBtn} title="Hide this strip" aria-label="Hide the context strip" onClick={dismiss}>
          <i className="fas fa-times" aria-hidden="true" />
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
        <button type="button" style={closeBtn} title="Hide this strip" aria-label="Hide the context strip" onClick={dismiss}>
          <i className="fas fa-times" aria-hidden="true" />
        </button>
      </div>
    );
  }

  return (
    <div style={wrap}>
      <span style={label}>
        <i className="fas fa-database" /> Context ready
      </span>
      {/* These were spans with mouse handlers, so everything the hover card
          holds — the date range, the filter values, the metrics, and the only
          statement of whether a dataset is ready — was reachable by pointer
          hover alone. A button carries the same handlers plus focus and blur,
          so the keyboard and the tab order reach it; on a touch device the tap
          that focuses it opens the card. */}
      {ready.map((d) => {
        const open = (element: HTMLElement) => {
          const r = element.getBoundingClientRect();
          setHover({ item: d, x: r.left, y: r.bottom + 6 });
        };
        return (
          <button
            type="button"
            key={d.dataset_id}
            style={{ ...pill, cursor: "help", font: "inherit", color: "inherit", textAlign: "left" }}
            aria-expanded={hover?.item.dataset_id === d.dataset_id}
            onMouseEnter={(e) => open(e.currentTarget)}
            onMouseLeave={() => setHover(null)}
            onFocus={(e) => open(e.currentTarget)}
            onBlur={() => setHover(null)}
          >
            <span style={dot(d.status === "ready")} />
            {/* The strip says what can be asked about, in as few words as it
                takes: the dataset, how much of it there is, and how much of it
                is answerable by value. A date range is a property of the data
                rather than of what you can ask, and it pushed the two numbers
                that do matter off the end of the row. It is still on the
                hover card for anyone who wants it. */}
            <b style={{ color: "var(--text-primary)" }}>{d.name ?? `#${d.dataset_id}`}</b>
            <span>· {compact(d.row_count)} {d.row_count === 1 ? "row" : "rows"}</span>
            {(d.values_indexed ?? 0) > 0 && <span>· {compact(d.values_indexed)} values</span>}
          </button>
        );
      })}
      <button type="button" style={closeBtn} title="Hide this strip" aria-label="Hide the context strip" onClick={dismiss}>
        <i className="fas fa-times" aria-hidden="true" />
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
    // `--bg-elevated` and `--shadow-lg` are both defined in both themes, so
    // the literal fallback and the literal shadow were dark-theme values that
    // could only ever be wrong on the light one.
    background: "var(--bg-elevated)", border: "1px solid var(--border)",
    borderRadius: 10, boxShadow: "var(--shadow-lg)",
    padding: 12, fontSize: 12, color: "var(--text-secondary)",
    textAlign: "left", whiteSpace: "normal", cursor: "default",
  };
  const chip: React.CSSProperties = {
    display: "inline-block", padding: "1px 7px", borderRadius: 999, marginRight: 4, marginBottom: 4,
    background: "rgba(var(--accent-rgb),0.1)", color: "var(--text-primary)", fontSize: 11,
  };
  // 10.5px section headings at faint measured 3.02:1 light and 3.01:1 dark;
  // they name what follows, so they are read, not skimmed past.
  const head: React.CSSProperties = { fontSize: 10.5, fontWeight: 700, textTransform: "uppercase", letterSpacing: 0.4, color: "var(--text-muted)", margin: "10px 0 4px" };
  return (
    <div style={card}>
      <div style={{ fontWeight: 700, color: "var(--text-primary)", fontSize: 13 }}>{item.name ?? `#${item.dataset_id}`}</div>
      {/* `--text-faint` is set to a deliberate 3:1 for a figure the source has
          moved past; these are current figures a reader is being asked to act
          on, and at 12px they measured 3.02:1 light and 3.01:1 dark. Muted is
          the tone for a value that still counts: 4.52:1 and 4.56:1. */}
      <div style={{ marginTop: 3, color: "var(--text-muted)" }}>
        {compact(item.row_count)} {item.row_count === 1 ? "row" : "rows"} · {item.columns_count ?? "—"} columns · {compact(item.values_indexed)} indexed values
        {dr?.min && dr?.max ? ` · ${shortDate(dr.min)} → ${shortDate(dr.max)}` : ""}
      </div>
      {/* The dot on the pill was the only place a dataset's compile state was
          reported, and it reported it in colour alone. It is stated here in
          words, and a state the server did not report is not read as ready. */}
      <div style={{ marginTop: 3, color: "var(--text-muted)" }}>
        {item.status === "ready"
          ? "Compiled and ready to be asked about."
          : item.status
            ? `Compile state: ${item.status}. Some answers may not be available yet.`
            : "The server did not report a compile state for this dataset."}
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
