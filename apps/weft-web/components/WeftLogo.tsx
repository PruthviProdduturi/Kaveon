"use client";

import React, { useCallback, useId, useState } from "react";

/**
 * WeftLogo — part of the shared Kaveon letterform kit.
 *
 * WE·T — Inter, 700, matching cap-height (same face as the Forge wordmark).
 * F    — Forge's custom chamfered letterform (45° cuts on the free bar ends,
 *        top-lit gradient). Reused verbatim so the suite reads as one family.
 *
 * Colour comes from the Weft theme CSS vars (set synchronously in layout.tsx
 * before hydration), so there's no flash and no hydration mismatch.
 */

interface WeftLogoProps {
  size?: number;
  animate?: "pulse" | "none" | "revolve";
  onClick?: () => void;
  className?: string;
}

export function WeftLogo({ size = 48, animate = "none", onClick, className = "" }: WeftLogoProps) {
  const uid = useId();
  const gradId = `weft-f-grad-${uid.replace(/:/g, "")}`;
  const [animating, setAnimating] = useState(false);

  const handleClick = useCallback(() => {
    if (!animating) {
      setAnimating(true);
      setTimeout(() => setAnimating(false), 420);
    }
    onClick?.();
  }, [animating, onClick]);

  // Match text cap-height to the F glyph height exactly (Inter cap-height ≈ 0.728em).
  const capH = size * 0.62;
  const fontSize = capH / 0.728;

  const letterStyle: React.CSSProperties = {
    fontSize,
    fontWeight: 700,
    fontFamily: "'Inter', 'Segoe UI', system-ui, sans-serif",
    letterSpacing: capH * 0.06,
    lineHeight: 1,
    color: "currentColor",
  };

  return (
    <div
      onClick={handleClick}
      className={`weft-logo ${className} ${onClick ? "cursor-pointer" : ""} ${
        animate === "pulse" ? "animate-pulse-logo" : ""
      } ${animate === "revolve" || animating ? "animate-revolve" : ""}`}
      role={onClick ? "button" : undefined}
      aria-label="Weft"
      style={{
        height: size,
        display: "inline-flex",
        alignItems: "center",
        userSelect: "none",
        color: "var(--weft-primary, #6366f1)",
      }}
    >
      <span style={{ display: "inline-flex", alignItems: "center", gap: capH * 0.08, lineHeight: 1 }}>
        {/* WE */}
        <span style={letterStyle}>WE</span>

        {/* ── F — Forge's chamfered letterform ───────────────────────────── */}
        <svg
          width={capH * 0.66}
          height={capH}
          viewBox="0 0 18 28"
          fill="none"
          aria-hidden="true"
          overflow="visible"
          style={{ marginLeft: capH * 0.04, marginRight: capH * 0.02 }}
        >
          <defs>
            {/* Top-lit gradient — polished forged steel */}
            <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="currentColor" stopOpacity="1" />
              <stop offset="100%" stopColor="currentColor" stopOpacity="0.65" />
            </linearGradient>
          </defs>
          <path
            d="M 0 0 L 16 0 L 18 2.5 L 18 7 L 6 7 L 6 12 L 12 12 L 14 14.5 L 14 19 L 6 19 L 6 28 L 0 28 Z"
            fill={`url(#${gradId})`}
          />
          {/* Top-face highlight — catches the light */}
          <path d="M 0 0 L 16 0 L 18 2.5 L 18 3.5 L 0 3.5 Z" fill="currentColor" opacity="0.18" />
        </svg>

        {/* T */}
        <span style={letterStyle}>T</span>
      </span>

      <style jsx>{`
        .weft-logo {
          transition: transform 280ms cubic-bezier(0.2, 0, 0, 1), filter 280ms;
          will-change: transform, filter;
          filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.08));
        }
        .weft-logo:hover:not(.animate-revolve) {
          transform: translateY(-2px);
          filter: drop-shadow(0 6px 12px rgba(var(--weft-primary-rgb), 0.19));
        }
        @keyframes pulse-logo {
          0% { transform: scale(1); }
          50% { transform: scale(1.05); }
          100% { transform: scale(1); }
        }
        @keyframes revolve {
          0% { transform: rotate(0deg); }
          100% { transform: rotate(360deg); }
        }
        .animate-pulse-logo { animation: pulse-logo 2.4s ease-in-out infinite; }
        .animate-revolve { animation: revolve 0.42s ease-in-out; }
        .cursor-pointer { cursor: pointer; user-select: none; }
      `}</style>
    </div>
  );
}

export default WeftLogo;
