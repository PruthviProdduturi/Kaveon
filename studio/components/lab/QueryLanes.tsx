"use client";

import React from "react";

/**
 * How a statement is being answered, shown while it runs.
 *
 * KaveonDB resolves a statement in a fixed order: it takes a slot under its
 * resource group, then takes two chances to answer with no read at all — the
 * coordinator's result cache, then the table's statistics or cube — and only
 * if neither holds the answer does it read the data. That order is the
 * product's claim: Kaveon knows a table rather than re-reading it. A wait
 * that renders the order, and says which lane answered and how little it had
 * to read, reports something; a spinner reports nothing.
 *
 * Nothing here guesses. The record names its placement (`execution.mode`)
 * only once the statement has settled — until then it reads `pending` — so a
 * step is marked "deciding" only while the live signals put the statement
 * there, and a lane is named only when the record names it.
 */

/** The live signals the Studio holds while a KaveonDB statement runs. */
export interface LaneSignals {
  /** The coordinator's statement state: QUEUED, RUNNING, FINISHED, FAILED, CANCELED. */
  state: string;
  /** The record's placement: `pending` while it runs, then cache / context / distributed / coordinator. */
  mode: string | null;
  tasksDone: number;
  tasksTotal: number;
  rowsScanned: number;
  workers: number;
}

interface Lanes {
  /** Where the answer came from, in one phrase.  */
  headline: string;
  /** One sentence for assistive technology; it changes only when the phrase does. */
  announcement: string;
}

/**
 * The state the Studio uses for a statement it has submitted but holds no
 * record for yet. The coordinator never reports it; it exists so the ladder
 * says what the Studio actually knows at that moment.
 */
export const SUBMITTED = "SUBMITTED";

/** The placements the coordinator reports once a statement has settled. */
const KNOWN_ANSWER_MODES = new Set(["cache", "context"]);
const READ_MODES = new Set(["distributed", "coordinator"]);

/**
 * The Guardian O, as a path: a 300-degree arc with its gap at the bottom,
 * the geometry of the wordmark's O and of `docs/reference/kaveon-icon.svg`,
 * scaled into a 48-unit box. `pathLength` is declared as 100 wherever this
 * is drawn, so every dash and offset below is a percentage of the arc.
 */
const HALO_ARC = "M 32.5 38.7224 A 17 17 0 1 0 15.5 38.7224";

export interface KaveonHaloProps {
  /** The fraction of the reported work complete, 0–1, or null when the coordinator has reported none. */
  progress?: number | null;
  /** Rendered size in pixels. */
  size?: number;
  /** `inline` weights the stroke so the mark still reads at text size. */
  variant?: "panel" | "inline";
}

/**
 * The running mark: the Guardian O in motion.
 *
 * Four strokes share the one brand arc, and only one of them loops:
 *
 *  - the track, the halo at rest, so the mark is the brand even when still;
 *  - the read, drawn from the start of the arc to as far as the coordinator
 *    says the statement has got — completed work, never a timer, and absent
 *    while the coordinator has reported none;
 *  - a bloom and the light above it, one dash travelling the arc, out
 *    through the gap at the bottom and back in on the other side. It is the
 *    only looping motion in the Lab, and it says the statement is alive.
 *
 * `prefers-reduced-motion` drops the travelling light altogether; the track
 * and the read stay, so the mark still reports where the statement has got.
 */
export function KaveonHalo({ progress = null, size = 48, variant = "panel" }: KaveonHaloProps) {
  const read = progress == null ? null : Math.min(1, Math.max(0, progress)) * 100;
  return (
    <svg
      className={`kv-halo kv-halo--${variant}`}
      width={size}
      height={size}
      viewBox="0 0 48 48"
      aria-hidden="true"
      focusable="false"
    >
      <path className="kv-halo__track" d={HALO_ARC} pathLength={100} />
      {read != null && (
        <path
          className="kv-halo__read"
          d={HALO_ARC}
          pathLength={100}
          style={{ strokeDashoffset: 100 - read }}
        />
      )}
      <path className="kv-halo__bloom" d={HALO_ARC} pathLength={100} />
      <path className="kv-halo__light" d={HALO_ARC} pathLength={100} />
    </svg>
  );
}

/**
 * How far the statement has got, as a fraction, or null while there is
 * nothing measured to report. Only completed tasks count: they are work the
 * cluster has finished, so the arc never claims progress the coordinator has
 * not reported.
 */
export function reportedProgress(signals: LaneSignals | null): number | null {
  if (!signals || signals.tasksTotal <= 0) return null;
  return signals.tasksDone / signals.tasksTotal;
}

/**
 * The Guardian O — the open ring of the Kaveon wordmark, its gap at the
 * bottom, in a 24-unit box. It carries all four step states, and it is
 * still in every one of them: the halo at the head of the panel is the only
 * moving mark, so the ladder reads as a list of facts rather than as four
 * things competing for the eye.
 */
function workerPhrase(workers: number): string {
  if (workers <= 0) return "across the cluster";
  return `across ${workers} ${workers === 1 ? "worker" : "workers"}`;
}

/** The ladder for a statement running on KaveonDB. */
function engineLanes(signals: LaneSignals): Lanes {
  const mode = signals.mode && signals.mode !== "pending" ? signals.mode : null;
  const answeredWithoutReading = mode != null && KNOWN_ANSWER_MODES.has(mode);
  const answeredByReading = mode != null && READ_MODES.has(mode);
  // Before the first record read the Studio knows only that it submitted;
  // saying so is more honest than claiming a place in the admission queue.
  const submitting = signals.state === SUBMITTED;
  const queued = submitting || signals.state === "QUEUED";
  // The read has started the moment the coordinator reports tasks, workers
  // or scanned rows; before that the statement is still being placed.
  const reading = signals.tasksTotal > 0 || signals.workers > 0 || signals.rowsScanned > 0;

  let headline: string;
  if (mode === "cache") headline = "Served from the result cache";
  else if (mode === "context") headline = "Answered without reading data";
  else if (mode === "distributed") headline = `Scanned ${workerPhrase(signals.workers)}`;
  else if (mode === "coordinator") headline = "Read on the coordinator";
  // Before the coordinator has placed the statement there is nothing to
  // report but that it is running. Narrating the steps it is about to take
  // describes the engine rather than the reader's query.
  else if (queued && !submitting) headline = "Queued";
  else headline = "Running";

  return {
    headline,
    announcement: headline + ".",
  };
}

/**
 * A federated source holds no Kaveon statistics and no cached result for a
 * table it does not own, so the statement is always a live read there and
 * there is nothing to choose between.
 */
function federatedLanes(sourceLabel: string | null): Lanes {
  const target = sourceLabel ? `Running on ${sourceLabel}` : "Running on the source";
  return { headline: target, announcement: target + "." };
}

export interface QueryLanePanelProps {
  /** The coordinator's live signals, or null when the statement runs on a federated source. */
  signals: LaneSignals | null;
  /** The federated source the statement is running on, when it is not on KaveonDB. */
  sourceLabel?: string | null;
  /** Elapsed time, already formatted by the caller so the Lab has one time format. */
  elapsedLabel: string;
  onCancel?: () => void;
}

/**
 * The Lab's running state: the lane ladder, the facts the coordinator has
 * reported so far, and the cancel the operator needs. It reads state the
 * query path already publishes and adds no work to it.
 */
export function QueryLanePanel({ signals, sourceLabel = null, elapsedLabel, onCancel }: QueryLanePanelProps) {
  const lanes = signals ? engineLanes(signals) : federatedLanes(sourceLabel);

  const facts: string[] = [elapsedLabel];
  if (signals) {
    if (signals.rowsScanned > 0) facts.push(`${signals.rowsScanned.toLocaleString()} rows scanned`);
    else if (signals.mode && KNOWN_ANSWER_MODES.has(signals.mode)) facts.push("no rows scanned");
    if (signals.workers > 0) facts.push(`${signals.workers} ${signals.workers === 1 ? "worker" : "workers"}`);
  }

  return (
    <div className="lane-panel">
      <div className="lane-panel__inner">
        <div className="lane-panel__head">
          <KaveonHalo progress={reportedProgress(signals)} size={46} />
          <div className="lane-panel__headings">
            <h3 className="lane-panel__headline">{lanes.headline}</h3>
          </div>
        </div>

        <div className="lane-panel__foot">
          <span className="lane-facts" aria-hidden="true">
            {facts.map((fact) => (
              <span key={fact}>{fact}</span>
            ))}
          </span>
          {onCancel && (
            <button type="button" className="lane-panel__cancel" onClick={onCancel} title="Cancel the statement">
              Cancel
            </button>
          )}
        </div>
      </div>

      <p className="sr-only" role="status" aria-live="polite">
        {lanes.announcement}
      </p>
    </div>
  );
}

export default QueryLanePanel;
