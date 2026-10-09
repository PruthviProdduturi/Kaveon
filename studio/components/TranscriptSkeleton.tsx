"use client";

import React from "react";
import s from "./TranscriptSkeleton.module.css";

/**
 * The transcript's own shape while a reopened conversation is being read.
 *
 * Resuming a chat from Recents is a section change, not a page load: the
 * sidebar, the context banner and the composer all stay where they are and
 * only the transcript area waits. This is what waits there — the alternating
 * question-and-answer rhythm of the conversation that is about to replace it,
 * at the widths real messages have.
 */

/** Bar widths per row, as a share of the bubble — question short, answer long. */
const ROWS: { role: "user" | "assistant"; lines: number[] }[] = [
  { role: "user", lines: [62] },
  { role: "assistant", lines: [94, 88, 54] },
  { role: "user", lines: [46] },
  { role: "assistant", lines: [90, 71] },
];

export function TranscriptSkeleton() {
  return (
    <div className={s.list} role="status" aria-label="Loading conversation">
      {ROWS.map((row, i) => (
        <div
          key={i}
          className={`${s.row} ${row.role === "user" ? s.user : ""}`}
          aria-hidden="true"
        >
          <span className={s.avatar} />
          <span
            className={s.bubble}
            style={{ width: `${row.role === "user" ? 42 : 68}%`, maxWidth: row.role === "user" ? "75%" : "90%" }}
          >
            {row.lines.map((width, line) => (
              <span key={line} className={s.bar} style={{ width: `${width}%` }} />
            ))}
          </span>
        </div>
      ))}
    </div>
  );
}
