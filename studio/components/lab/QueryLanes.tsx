"use client";

import React from "react";
import { KaveonLoading } from "../KaveonLoading";

/**
 * What the Studio shows while a statement is in flight.
 *
 * The signals are the coordinator's own: its statement state, the placement
 * it reports once the statement settles, and the task and row counts it
 * publishes as it goes. Nothing here guesses — a figure is shown only when
 * the record carries it.
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

export interface QueryProgressProps {
  /** Completed fraction when the coordinator has reported one, else null. */
  progress: number | null;
  onCancel?: () => void;
  /** Hold the middle of the pane rather than sitting along its top edge. */
  centered?: boolean;
  /**
   * Draw only the bar, pinned over the top edge of the pane and taking no
   * space in the layout. For a statement whose rows have already started
   * landing: the grid is on screen and must not move when the bar goes.
   */
  overlay?: boolean;
}

/**
 * A statement in flight.
 *
 * This used to be a four-step ladder naming admission, the result cache,
 * the statistics and the read. Those are the engine's own stages, written
 * in the engine's words, and with the cube answering a breakdown in about
 * 150ms there is no wait long enough to read them in. Where the answer came
 * from is worth saying, and the results bar says it once the rows land.
 *
 * What a reader needs while waiting is that it is moving, and a way out.
 * The bar fills to whatever the coordinator reports and otherwise travels,
 * and it holds still for anyone who asked for less motion.
 */
export function QueryProgress({ progress, onCancel, centered = false, overlay = false }: QueryProgressProps) {
  const determinate = progress != null && progress > 0;
  // Rows are already on screen. Anything that occupies height here pushes the
  // grid down and then lets it jump back when the statement finishes, so this
  // takes none: it is painted over the pane's top edge and nothing reflows
  // when it goes. Cancelling is still on the toolbar, where it always is.
  if (overlay) {
    return (
      <div className="query-progress-overlay" role="status" aria-live="polite">
        <div className={`query-progress__track${determinate ? "" : " query-progress__track--roving"}`}>
          <span
            className="query-progress__fill"
            style={determinate ? { width: `${Math.min(100, Math.round(progress * 100))}%` } : undefined}
          />
        </div>
        <span className="sr-only">Rows are still arriving.</span>
      </div>
    );
  }
  // Holding the pane is the same moment as opening a dashboard or a page, so
  // it is the same component rather than a second thing that merely resembles
  // it — the breathing mark, the uppercase line and the travelling bar. The
  // slim inline bar below stays for the cases that sit along an edge.
  if (centered) {
    return (
      <KaveonLoading
        message="Running query"
        fullScreen={false}
        mark={false}
        action={onCancel ? (
          <button type="button" className="query-progress__cancel" onClick={onCancel}>
            Cancel
          </button>
        ) : undefined}
      />
    );
  }
  return (
    <div
      className={`query-progress${centered ? " query-progress--centered" : ""}`}
      role="status"
      aria-live="polite"
    >
      <div className={`query-progress__track${determinate ? "" : " query-progress__track--roving"}`}>
        <span
          className="query-progress__fill"
          style={determinate ? { width: `${Math.min(100, Math.round(progress * 100))}%` } : undefined}
        />
      </div>
      {onCancel && (
        <button type="button" className="query-progress__cancel" onClick={onCancel}>
          Cancel
        </button>
      )}
      <span className="sr-only">Running your query.</span>
    </div>
  );
}

export default QueryProgress;