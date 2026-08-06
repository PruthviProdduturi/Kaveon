"use client";

import React from "react";

interface KaveonMarkProps {
  size?: number;
  className?: string;
  opacity?: number;
  /** When true uses "#4A9EE8" directly; when false uses "var(--accent, #4A9EE8)" */
  useDirectColor?: boolean;
}

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
      viewBox="0 0 100 100"
      fill="none"
      className={className}
      role="img"
      aria-label="Kaveon"
      style={{ opacity }}
    >
      {/* Open arc ~280° */}
      <path
        d="M 30 72 A 34 34 0 1 1 42 80"
        stroke={color}
        strokeWidth="8"
        fill="none"
        strokeLinecap="round"
      />
      {/* Chat bubble tail */}
      <path
        d="M 30 72 L 24 84 L 42 80"
        stroke={color}
        strokeWidth="8"
        fill="none"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      {/* Center dot */}
      <circle cx="50" cy="48" r="7" fill={color} />
    </svg>
  );
}

interface KaveonWordmarkProps {
  height?: number;
  className?: string;
}

/** KaveonWordmark — "KAVE" + inline O-mark + "N". For sidebar expanded state. */
export function KaveonWordmark({ height = 18, className }: KaveonWordmarkProps) {
  const color = "var(--accent, #4A9EE8)";
  const markSize = height;

  return (
    <span
      className={className}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: height * 0.2,
        height,
        lineHeight: 1,
      }}
    >
      <span
        style={{
          fontSize: height,
          fontWeight: 800,
          fontFamily: "'Inter', 'Segoe UI', system-ui, sans-serif",
          letterSpacing: "-0.01em",
          color,
          lineHeight: 1,
          whiteSpace: "nowrap",
        }}
      >
        KAVE
      </span>
      <KaveonMark size={markSize} useDirectColor={false} />
      <span
        style={{
          fontSize: height,
          fontWeight: 800,
          fontFamily: "'Inter', 'Segoe UI', system-ui, sans-serif",
          letterSpacing: "-0.01em",
          color,
          lineHeight: 1,
          whiteSpace: "nowrap",
        }}
      >
        N
      </span>
    </span>
  );
}
