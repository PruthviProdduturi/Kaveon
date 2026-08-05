"use client";

import React, { useCallback, useId, useState } from "react";

/**
 * LensLogo — part of the shared Kaveon letterform kit.
 *
 * Mark  — a six-blade APERTURE. Where Kaveon's crosshair-O (⊙) has a pupil at its
 *         centre — the guardian that *watches* — Lens has an open centre: the aperture
 *         you look *through* to bring the governed data into focus. Same circular
 *         family, opposite centre. Top-lit gradient, matching the Forge F.
 * Word   — "LENS" in Inter 800, same face/cap-height as the Forge wordmark.
 *
 * Colour comes from the Lens theme CSS vars (set synchronously in layout.tsx before
 * hydration), so there's no flash and no hydration mismatch.
 */

interface LensLogoProps {
  size?: number;
  /** "iris" gently tightens/opens the blades; "revolve" spins the aperture once. */
  animate?: "iris" | "revolve" | "none";
  onClick?: () => void;
  className?: string;
}

const BLADES = 6;
const CX = 12;
const CY = 12;
const R_OUTER = 10.2; // blade tips ride just inside the ring
const R_INNER = 3.9; // radius of the hexagonal opening
const TWIST = (42 * Math.PI) / 180; // swirl between a blade's outer tip and inner heel

// Precompute the six blades once (geometry is symmetric, so this never changes).
const BLADE_LINES = Array.from({ length: BLADES }, (_, k) => {
  const a = Math.PI / 2 + (k * 2 * Math.PI) / BLADES; // start at top, go around
  const ox = CX + R_OUTER * Math.cos(a);
  const oy = CY - R_OUTER * Math.sin(a);
  const ix = CX + R_INNER * Math.cos(a - TWIST);
  const iy = CY - R_INNER * Math.sin(a - TWIST);
  return { ox, oy, ix, iy };
});

// The inner heels, connected, form the hexagonal opening.
const OPENING = BLADE_LINES.map((b, i) => `${i === 0 ? "M" : "L"} ${b.ix.toFixed(2)} ${b.iy.toFixed(2)}`).join(" ") + " Z";

export function LensLogo({ size = 48, animate = "none", onClick, className = "" }: LensLogoProps) {
  const uid = useId();
  const gradId = `lens-ap-grad-${uid.replace(/:/g, "")}`;
  const [spinning, setSpinning] = useState(false);

  const handleClick = useCallback(() => {
    if (!spinning) {
      setSpinning(true);
      setTimeout(() => setSpinning(false), 520);
    }
    onClick?.();
  }, [spinning, onClick]);

  // capH is the visual reference height — matches Forge exactly.
  const capH     = size * 0.62;
  const fontSize = capH / 0.728;

  return (
    <div
      onClick={handleClick}
      className={`lens-logo ${className} ${onClick ? "cursor-pointer" : ""} ${
        animate === "iris" ? "anim-iris" : ""
      } ${animate === "revolve" || spinning ? "anim-revolve" : ""}`}
      role={onClick ? "button" : undefined}
      aria-label="Lens"
      style={{
        height: size,
        display: "inline-flex",
        alignItems: "center",
        gap: 0,
        userSelect: "none",
        color: "var(--lens-primary, #46c7d9)",
      }}
    >
      <span style={{ display: "inline-flex", alignItems: "center", gap: capH * 0.12 }}>

        {/* ── ⊙ — hexagonal crosshair mark ────────────────────────────── */}
        <svg
          className="lens-aperture"
          width={capH}
          height={capH}
          viewBox="0 0 22 22"
          fill="none"
          aria-hidden="true"
        >
          <circle cx="11" cy="11" r="9.5" stroke="currentColor" strokeWidth="1.9" />
          <g className="lens-blades" style={{ transformOrigin: "11px 11px" }}>
            <path
              d="M 13.35 7.62 L 9.42 6.78 L 6.52 10.07 L 7.93 14.88 L 11.86 15.72 L 14.76 12.43 Z"
              opacity="0.12" fill="currentColor"
            />
            <line x1="13.35" y1="7.62" x2="11" y2="1.5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
            <line x1="9.42" y1="6.78" x2="3" y2="5.5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
            <line x1="6.52" y1="10.07" x2="3" y2="16.5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
            <line x1="7.93" y1="14.88" x2="11" y2="20.5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
            <line x1="11.86" y1="15.72" x2="19" y2="16.5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
            <line x1="14.76" y1="12.43" x2="19" y2="5.5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
          </g>
        </svg>

        {/* ── Chamfered L + ENS wordmark ─────────────────────────────────── */}
        <svg
          width={capH * 0.5}
          height={capH}
          viewBox="0 0 14 28"
          fill="none"
          aria-hidden="true"
        >
          <defs>
            <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="currentColor" stopOpacity="1" />
              <stop offset="100%" stopColor="currentColor" stopOpacity="0.65" />
            </linearGradient>
          </defs>
          <path
            d="M 0 0 L 6 0 L 6 21 L 12 21 L 14 23.5 L 14 28 L 0 28 Z"
            fill={`url(#${gradId})`}
          />
          <path d="M 0 0 L 6 0 L 6 3.5 L 0 3.5 Z" fill="currentColor" opacity="0.18" />
        </svg>
        <span
          style={{
            fontSize: fontSize * 0.88,
            fontWeight: 700,
            fontFamily: "'Inter', 'Segoe UI', system-ui, sans-serif",
            color: "currentColor",
            lineHeight: 1,
          }}
        >
          ENS
        </span>

      </span>

      <style jsx>{`
        .lens-logo {
          transition: transform 280ms cubic-bezier(0.2, 0, 0, 1), filter 280ms;
          will-change: transform, filter;
          filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.08));
        }
        .lens-logo:hover:not(.anim-revolve) {
          filter: drop-shadow(0 6px 12px rgba(var(--lens-primary-rgb), 0.19));
        }
        /* Hover spins the aperture blades — like Forge's crosshair */
        .lens-aperture .lens-blades {
          transition: transform 420ms cubic-bezier(0.5, 0, 0.2, 1);
        }
        .lens-logo:hover .lens-blades {
          transform: rotate(360deg);
        }
        @keyframes lens-iris {
          0%, 100% { transform: rotate(0deg) scale(1); }
          50% { transform: rotate(-16deg) scale(0.94); }
        }
        @keyframes lens-revolve {
          from { transform: rotate(0deg); }
          to { transform: rotate(360deg); }
        }
        .anim-iris .lens-blades { animation: lens-iris 3s ease-in-out infinite; }
        .anim-revolve .lens-blades { animation: lens-revolve 0.52s cubic-bezier(0.5, 0, 0.2, 1); }
        .cursor-pointer { cursor: pointer; user-select: none; }
      `}</style>
    </div>
  );
}

/** LensMark — the aperture alone (no wordmark). For nav-collapsed, favicons, avatars. */
export function LensMark({ size = 24, className = "" }: { size?: number; className?: string }) {
  const uid = useId();
  const gradId = `lens-mark-grad-${uid.replace(/:/g, "")}`;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      className={className}
      role="img"
      aria-label="Lens"
      style={{ color: "var(--lens-primary, #46c7d9)" }}
    >
      <defs>
        <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="currentColor" stopOpacity="1" />
          <stop offset="100%" stopColor="currentColor" stopOpacity="0.62" />
        </linearGradient>
      </defs>
      <circle cx={CX} cy={CY} r={10.6} fill="none" stroke={`url(#${gradId})`} strokeWidth={1.5} />
      <path d={OPENING} fill="currentColor" opacity={0.1} />
      {BLADE_LINES.map((b, i) => (
        <line
          key={i}
          x1={b.ix.toFixed(2)}
          y1={b.iy.toFixed(2)}
          x2={b.ox.toFixed(2)}
          y2={b.oy.toFixed(2)}
          stroke={`url(#${gradId})`}
          strokeWidth={1.6}
          strokeLinecap="round"
        />
      ))}
    </svg>
  );
}

export default LensLogo;
