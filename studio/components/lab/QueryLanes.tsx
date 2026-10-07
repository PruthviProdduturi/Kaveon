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

/** Where a statement stands relative to one step of the ladder. */
export type LaneStepState = "answered" | "passed" | "deciding" | "unreached";

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

interface LaneStep {
  id: string;
  label: string;
  /** The one-line consequence of this step for this statement, or null when it has nothing to say yet. */
  note: string | null;
  /** What the step is doing, in words, so the state never rests on colour. */
  status: string;
  state: LaneStepState;
}

interface Lanes {
  headline: string;
  /** The claim the ladder is making, shown only while no lane has answered. */
  sub: string | null;
  steps: LaneStep[];
  /** One sentence for assistive technology; it changes only when the ladder moves. */
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
function LaneGlyph({ state }: { state: LaneStepState }) {
  if (state === "passed") {
    return (
      <svg className="lane-glyph" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
        <path
          d="M 7.4 9.8 L 12 14.4 L 16.6 9.8"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.7"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (state === "unreached") {
    return (
      <svg className="lane-glyph" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
        <circle cx="12" cy="12" r="2.4" fill="none" stroke="currentColor" strokeWidth="1.3" />
      </svg>
    );
  }

  return (
    <svg className="lane-glyph" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path
        d="M 16.075 19.058 A 8.15 8.15 0 1 0 7.925 19.058"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.1"
      />
      {state === "answered" && <circle cx="12" cy="12" r="2.7" fill="currentColor" />}
    </svg>
  );
}

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

  const admitted: LaneStep = queued
    ? {
        id: "admitted",
        label: "Admission",
        note: submitting
          ? "Submitted to the coordinator."
          : "Waiting for a slot in the resource group.",
        status: submitting ? "submitting" : "queued",
        state: "deciding",
      }
    : {
        id: "admitted",
        label: "Admission",
        note: "Running under its resource group.",
        status: "passed through",
        state: "passed",
      };

  let known: LaneStep;
  if (mode === "cache") {
    known = {
      id: "known",
      label: "Known answers",
      note: "The result cache already held this statement. No data was read.",
      status: "answered here",
      state: "answered",
    };
  } else if (mode === "context") {
    known = {
      id: "known",
      label: "Known answers",
      note: "The table statistics describe exactly this version. No data was read.",
      status: "answered here",
      state: "answered",
    };
  } else if (answeredByReading || reading) {
    known = {
      id: "known",
      label: "Known answers",
      note: "Neither the result cache nor the statistics held this answer.",
      status: "passed through",
      state: "passed",
    };
  } else if (queued) {
    known = { id: "known", label: "Known answers", note: null, status: "not reached", state: "unreached" };
  } else {
    known = {
      id: "known",
      label: "Known answers",
      note: "Checking the result cache, then the table statistics and cube.",
      status: "checking",
      state: "deciding",
    };
  }

  let read: LaneStep;
  if (mode === "distributed") {
    read = {
      id: "read",
      label: "Data read",
      note: `Scanned ${workerPhrase(signals.workers)}.`,
      status: "answered here",
      state: "answered",
    };
  } else if (mode === "coordinator") {
    read = {
      id: "read",
      label: "Data read",
      note: "This shape has no distributed plan, so the coordinator read it.",
      status: "answered here",
      state: "answered",
    };
  } else if (answeredWithoutReading) {
    read = { id: "read", label: "Data read", note: "Not needed.", status: "not reached", state: "unreached" };
  } else if (reading) {
    read = {
      id: "read",
      label: "Data read",
      note:
        signals.tasksTotal > 0
          ? `${signals.tasksDone.toLocaleString()} of ${signals.tasksTotal.toLocaleString()} tasks complete ${workerPhrase(signals.workers)}.`
          : `Reading ${workerPhrase(signals.workers)}.`,
      status: "reading",
      state: "deciding",
    };
  } else {
    read = {
      id: "read",
      label: "Data read",
      note: null,
      status: "not reached",
      state: "unreached",
    };
  }

  const steps = [admitted, known, read];

  let headline: string;
  if (mode === "cache") headline = "Served from the result cache";
  else if (mode === "context") headline = "Answered from table statistics";
  else if (mode === "distributed") headline = `Scanned ${workerPhrase(signals.workers)}`;
  else if (mode === "coordinator") headline = "Read on the coordinator";
  else if (submitting) headline = "Submitted to KaveonDB";
  else if (queued) headline = "Queued for admission";
  else headline = "Choosing how to answer";

  const sub = mode == null ? "Kaveon answers without reading data whenever it already knows the answer." : null;
  const current = steps.find((step) => step.state === "answered")
    ?? steps.find((step) => step.state === "deciding")
    ?? steps[steps.length - 1];

  return {
    headline,
    sub,
    steps,
    announcement: `${headline}. ${current.label}: ${current.status}.`,
  };
}

/**
 * The ladder for a statement on a federated source. There are no lanes
 * there: Kaveon holds no statistics and no cached result for a table it does
 * not own, so the statement runs on the source. Saying so is honest; showing
 * three steps it never takes would not be.
 */
function federatedLanes(sourceLabel: string | null): Lanes {
  const target = sourceLabel ? `Running on ${sourceLabel}` : "Running on the source";
  return {
    headline: target,
    sub: "A federated source holds no Kaveon statistics or cached result, so every statement is a live read.",
    steps: [
      {
        id: "read",
        label: "Live read",
        note: "The statement is running on the source and its rows are on the way.",
        status: "reading",
        state: "deciding",
      },
    ],
    announcement: `${target}. Live read: reading.`,
  };
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
            {lanes.sub && <p className="lane-panel__sub">{lanes.sub}</p>}
          </div>
        </div>

        <ol className="lane-steps" aria-hidden="true">
          {lanes.steps.map((step) => (
            <li key={step.id} className={`lane-step lane-step--${step.state}`}>
              <span className="lane-step__rail" />
              <span className="lane-step__glyph">
                <LaneGlyph state={step.state} />
              </span>
              <span className="lane-step__body">
                <span className="lane-step__label">{step.label}</span>
                {step.note && <span className="lane-step__note">{step.note}</span>}
              </span>
              <span className="lane-step__status">{step.status}</span>
            </li>
          ))}
        </ol>

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
