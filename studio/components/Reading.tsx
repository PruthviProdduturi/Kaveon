"use client";

/**
 * A read that did not answer, rendered as an unknown.
 *
 * Three things were being collapsed into one across this product, and the
 * collapse was always in the same direction: **absent** (there is genuinely
 * nothing), **unknown** (we could not ask) and **stale** (we asked a while
 * ago, and this is what it said) all came out as absent. The catalog page
 * reported "0 tables" and "No tables found" over a catalog holding eighteen,
 * because the Engine was scaled to zero. The context banner said "No context
 * compiled yet" with eight of nine datasets compiled. Settings told an
 * administrator to run a setup wizard against a database that was already
 * correct and merely restarting (D-10, fixed in `e7f8b453`).
 *
 * Every one of those is a failed read rendered as a verdict, and every one of
 * them told the reader to fix something that was not broken. So the rule is
 * written once, here, rather than in each page's conditionals:
 *
 * 1. A read that failed renders as unknown, never as absent.
 * 2. It names what could not be reached, and says that nothing is known to be
 *    wrong — because nothing is.
 * 3. It instructs a change in configuration only where the server itself said
 *    the thing is unconfigured. `reason` carries that distinction where an
 *    endpoint reports it (`catalog-sources/engine-status` answers
 *    `not_configured` | `unreachable` | `invalid_listing`); where no endpoint
 *    reports it, the absence of a reason reads as a plain unknown and no
 *    configuration advice is offered.
 * 4. Where a previous good answer exists, it is served and labelled as the
 *    last reading rather than blanked — `LastKnown` is that label. A reader
 *    looking at slightly old figures is better served than one told their
 *    data is gone.
 *
 * The pages that read share this state model, not their markup: one is a
 * sticky strip, one an inventory band inside a table, one a console masthead.
 * So what is shared is the sentence and the three presentations of it —
 * `unknownText` for prose a page composes itself, `UnknownPanel` where an
 * empty state would have gone, `UnknownLine` inside a strip or a row, and
 * `LastKnown` above figures that are no longer being refreshed. Each page
 * keeps its own layout and cannot drift from the rule.
 */

import s from "./Reading.module.css";

/**
 * The server's own account of why a read did not answer, in the vocabulary
 * `e7f8b453` established on the API side. Anything else is a plain unknown:
 * no signal is invented here to fill the gap.
 */
export type ReadReason = "not_configured" | "unreachable" | "invalid_listing";

/**
 * What an HTTP status says about reachability, where the route reports it that
 * way. The API answers 503 for an Engine this deployment was never told
 * about, and 502/504 for one that was and did not answer — the same
 * distinction `engine-status` reports by name. Anything else is a plain
 * unknown: a 403 stays a 403 and is never translated into a reachability
 * claim.
 */
export function reasonFromStatus(status: number | null | undefined): ReadReason | null {
  if (status === 503) return "not_configured";
  if (status === 502 || status === 504) return "unreachable";
  return null;
}

export interface ReadFailure {
  /** What could not be reached, as a sentence subject: "The data catalog", "KaveonDB". */
  what: string;
  /** What therefore cannot be stated: "which tables it holds". */
  says?: string;
  /** The server's account of why, where the endpoint reports one. */
  reason?: ReadReason | null;
  /** The message the server or the transport gave, kept verbatim. */
  detail?: string | null;
}

/**
 * The sentence. One subject, one consequence, and the assurance that the
 * failure is not a finding — never an instruction to reconfigure anything,
 * unless the server said the thing is unconfigured.
 */
export function unknownText({ what, says, reason, detail }: ReadFailure): string {
  const cannot = says ? `so this page cannot say ${says}` : "so this page cannot report on it";
  const body =
    reason === "not_configured"
      ? `${what} is not configured on this server, ${cannot}.`
      : reason === "unreachable"
        ? `${what} did not answer, ${cannot}. It may be restarting; the connection is configured and nothing is known to be wrong with it.`
        : reason === "invalid_listing"
          ? `${what} answered with something this page could not read, ${cannot}. The connection is configured and is reaching a server that disagrees with this version.`
          : `${what} could not be read, ${cannot}. Nothing is known to be missing.`;
  return detail ? `${body} ${detail}` : body;
}

/**
 * Where an empty state would have gone. Deliberately the same shape as an
 * empty state and deliberately not the same words: a reader who has seen
 * "No tables found" over a full catalog needs the difference to be visible
 * at a glance, so it carries the unknown mark rather than an invitation.
 */
export function UnknownPanel({
  title, onRetry, retryLabel = "Try again", className, ...failure
}: ReadFailure & { title?: string; onRetry?: () => void; retryLabel?: string; className?: string }) {
  return (
    <div className={`${s.panel} ${className ?? ""}`} role="status">
      <span className={s.mark} aria-hidden="true"><i className="fas fa-circle-question" /></span>
      <h2 className={s.panelTitle}>{title ?? `${failure.what} could not be read`}</h2>
      <p className={s.panelBody}>{unknownText(failure)}</p>
      {onRetry && (
        <button type="button" className={s.retry} onClick={onRetry}>{retryLabel}</button>
      )}
    </div>
  );
}

/** The same sentence inline — a strip, a prose paragraph, a row inside a table. */
export function UnknownLine({ className, ...failure }: ReadFailure & { className?: string }) {
  return (
    <span className={`${s.line} ${className ?? ""}`} role="status">
      <i className={`fas fa-circle-question ${s.lineMark}`} aria-hidden="true" />
      {unknownText(failure)}
    </span>
  );
}

/**
 * Figures that are still true of a moment that has passed.
 *
 * Shown above a reading that is being kept rather than cleared, so nobody
 * reads it as current and nobody is told their data is gone. `taken` is
 * written by the caller, in whichever elapsed form that surface already uses.
 */
export function LastKnown({ what, taken, detail, onRetry, retryLabel = "Try again", className }: {
  what: string; taken?: string | null; detail?: string | null;
  onRetry?: () => void; retryLabel?: string; className?: string;
}) {
  return (
    <div className={`${s.stale} ${className ?? ""}`} role="status">
      <i className={`fas fa-clock-rotate-left ${s.staleMark}`} aria-hidden="true" />
      <p className={s.staleText}>
        {taken
          ? `${what} is not answering, so these figures are the last reading, taken ${taken}.`
          : `${what} is not answering, so these figures are the last reading rather than the current one.`}
        {detail ? ` ${detail}` : ""}
      </p>
      {onRetry && (
        <button type="button" className={s.staleRetry} onClick={onRetry}>{retryLabel}</button>
      )}
    </div>
  );
}
