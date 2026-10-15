"use client";

import { useEffect, useRef, useState } from "react";

/**
 * The copy control on a code block. The clipboard is not always available —
 * no `navigator.clipboard` outside a secure context, a rejected permission, a
 * document that is not focused — and a write that fails must say so rather
 * than leave the button reading "Copy" as though nothing had been asked of
 * it. The three states are distinct, and the failure is announced as well as
 * shown so a reader who cannot see the chip is not told a lie by silence.
 */
type CopyState = "idle" | "copied" | "failed";

const LABELS: Record<CopyState, { text: string; aria: string }> = {
  idle: { text: "Copy", aria: "Copy code to clipboard" },
  copied: { text: "Copied", aria: "Code copied to clipboard" },
  failed: { text: "Copy failed", aria: "Code could not be copied to the clipboard" },
};

export function CopyCode({ value }: { value: string }) {
  const [state, setState] = useState<CopyState>("idle");
  const timer = useRef<number | null>(null);

  useEffect(() => () => { if (timer.current !== null) window.clearTimeout(timer.current); }, []);

  async function copy() {
    if (timer.current !== null) window.clearTimeout(timer.current);
    let copied = false;
    try {
      await navigator.clipboard.writeText(value);
      copied = true;
    } catch {
      copied = false;
    }
    setState(copied ? "copied" : "failed");
    // A failure needs longer on screen than a confirmation does.
    timer.current = window.setTimeout(() => { setState("idle"); timer.current = null; }, copied ? 1600 : 4000);
  }

  return <>
    <button className="docs-copy" type="button" onClick={copy} aria-label={LABELS[state].aria}>
      {LABELS[state].text}
    </button>
    <span className="docs-visually-hidden" role="status">
      {state === "idle" ? "" : LABELS[state].aria}
    </span>
  </>;
}
