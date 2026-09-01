"use client";

import { KaveonMark } from "./KaveonMark";

/**
 * LoadingOverlay — inline loading state.
 * Renders inside the content area (sidebar stays visible).
 * No longer a full-screen takeover.
 */
export function LoadingOverlay() {
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        minHeight: "60vh",
        gap: 24,
      }}
    >
      <div style={{ animation: "kaveon-breathe 3s ease-in-out infinite" }}>
        <KaveonMark size={48} useDirectColor />
      </div>
      <p
        style={{
          fontSize: 13,
          color: "var(--text-muted)",
          letterSpacing: "0.1em",
          textTransform: "uppercase",
        }}
      >
        Loading...
      </p>
    </div>
  );
}
