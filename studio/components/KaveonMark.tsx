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
        d="M 318 363.39 A 124 124 0 1 0 194 363.39"
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
 * KaveonWordmark — the canonical vector KAVE[O]N wordmark.
 * Fixed geometry keeps the brand consistent and crisp at every pixel ratio.
 */
export function KaveonWordmark({ height = 24, className }: KaveonWordmarkProps) {
  return (
    <svg
      className={className}
      width={height * 5.84}
      height={height}
      viewBox="70 55 1168 200"
      fill="none"
      role="img"
      aria-label="Kaveon"
      style={{ display: "block", flexShrink: 0, shapeRendering: "geometricPrecision" }}
    >
      <g fill="var(--text-primary, #111318)">
        <rect x="90" y="70" width="20" height="165" />
        <polygon points="108.73,161.20 215.73,86.39 204.27,70 97.27,144.80" />
        <polygon points="97.51,161.36 209.51,235 220.49,218.29 108.49,144.64" />
        <path d="M260 235 L330 70 L350 70 L420 235 L397 235 L340 104 L283 235 Z" />
        <path d="M465 70 L488 70 L545 201 L602 70 L625 70 L555 235 L535 235 Z" />
        <rect x="675" y="70" width="20" height="165" />
        <rect x="675" y="70" width="130" height="20" />
        <rect x="675" y="142.5" width="108" height="20" />
        <rect x="675" y="215" width="130" height="20" />
        <rect x="1060" y="70" width="20" height="165" />
        <rect x="1195" y="70" width="20" height="165" />
        <polygon points="1062.53,83.30 1197.53,235 1212.47,221.70 1077.47,70" />
      </g>
      <path d="M966.25 215.29 A72.5 72.5 0 1 0 893.75 215.29" stroke="#4A9EE8" strokeWidth="20" />
    </svg>
  );
}

export default KaveonMark;
