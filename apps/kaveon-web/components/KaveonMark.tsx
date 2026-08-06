"use client";

import React from "react";

interface KaveonMarkProps {
  size?: number;
  className?: string;
  opacity?: number;
  /** When true uses "#4A9EE8" directly; when false uses "var(--accent, #4A9EE8)" */
  useDirectColor?: boolean;
}

/**
 * KaveonMark — the Guardian O.
 * Open blue halo with gap at the bottom.
 * Geometry from the canonical kaveon-icon.svg (512×512 viewBox).
 */
export function KaveonMark({
  size = 28,
  className,
  opacity = 1,
  useDirectColor = false,
}: KaveonMarkProps) {
  const color = useDirectColor ? "#4A9EE8" : "var(--accent, #4A9EE8)";

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 512 512"
      fill="none"
      className={className}
      role="img"
      aria-label="Kaveon"
      style={{ opacity }}
    >
      <path
        d="M 343.68 407.88 A 124 124 0 1 0 168.32 407.88"
        stroke={color}
        strokeWidth="52"
        fill="none"
        strokeLinecap="butt"
      />
    </svg>
  );
}

interface KaveonWordmarkProps {
  height?: number;
  className?: string;
}

/**
 * KaveonWordmark — the full KAVE[O]N wordmark rendered inline.
 * KAVE and N in Inter 600, the O replaced by the Guardian O mark.
 */
export function KaveonWordmark({ height = 15, className }: KaveonWordmarkProps) {
  const textColor = "var(--text-primary, #111318)";

  return (
    <span
      className={className}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 0,
        height,
        lineHeight: 1,
      }}
    >
      <span
        style={{
          fontSize: height,
          fontWeight: 600,
          fontFamily: "'Inter', 'Segoe UI', system-ui, sans-serif",
          letterSpacing: "2px",
          color: textColor,
          lineHeight: 1,
          whiteSpace: "nowrap",
          textTransform: "uppercase",
        }}
      >
        KAVE
      </span>
      <KaveonMark size={height * 1.1} useDirectColor />
      <span
        style={{
          fontSize: height,
          fontWeight: 600,
          fontFamily: "'Inter', 'Segoe UI', system-ui, sans-serif",
          letterSpacing: "2px",
          color: textColor,
          lineHeight: 1,
          whiteSpace: "nowrap",
          textTransform: "uppercase",
        }}
      >
        N
      </span>
    </span>
  );
}

export default KaveonMark;
