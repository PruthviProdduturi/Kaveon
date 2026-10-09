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

/**
 * The rhythm being stood in for: a short question, a long answer, twice over.
 * `bubble` is the bubble's share of the transcript's width and `lines` each
 * text line's share of that bubble, so the block has a real transcript's
 * proportions rather than a uniform block's.
 */
const ROWS: { role: "user" | "assistant"; bubble: number; lines: number[] }[] = [
  { role: "user", bubble: 26, lines: [84] },
  { role: "assistant", bubble: 62, lines: [96, 89, 54] },
  { role: "user", bubble: 19, lines: [72] },
  { role: "assistant", bubble: 57, lines: [93, 68] },
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
          <span className={s.bubble} style={{ width: `${row.bubble}%` }}>
            {row.lines.map((width, line) => (
              <span key={line} className={s.bar} style={{ width: `${width}%` }} />
            ))}
          </span>
        </div>
      ))}
    </div>
  );
}
