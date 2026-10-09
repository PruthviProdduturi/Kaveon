"use client";

import React, { useState, useEffect } from "react";
import { KaveonMark } from "./KaveonMark";

interface KaveonLoadingProps {
  message?: string;
  fullScreen?: boolean;
  /**
   * Something the reader can do while they wait — a running statement's
   * Cancel, say. Sits under the bar so the wait and the way out of it read
   * as one thing.
   */
  action?: React.ReactNode;
  /**
   * Whether to show the Kaveon mark. A page opening is the product
   * appearing, and the mark belongs there. A statement running inside a pane
   * that is already the product is not, and the mark only adds a second
   * thing to look at.
   */
  mark?: boolean;
}

export function KaveonLoading({
  message = "Loading",
  fullScreen = true,
  action,
  mark = true,
}: KaveonLoadingProps) {
  const [dots, setDots] = useState("");

  useEffect(() => {
    const interval = setInterval(() => {
      setDots((prev) => (prev === "..." ? "" : prev + "."));
    }, 500);
    return () => clearInterval(interval);
  }, []);

  return (
    <div
      style={{
        ...(fullScreen
          ? { position: "fixed", inset: 0, zIndex: 9999 }
          : {
              width: "100%",
              // A floor so the mark is not cramped, and nothing else. No
              // border, no radius: inline, this is a page waiting for its
              // own content, not a card of its own. Drawing one put a box
              // around every section that loads — the Library, the Lab's
              // results — where there had never been one.
              minHeight: 240,
            }),
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        // Full screen, this covers the app and needs its own ground. Inline,
        // it sits inside a pane that already has one, and painting a second
        // drew a rectangle a shade off its host — visible as a dark block in
        // the Lab's results pane while a statement ran.
        background: fullScreen ? "var(--bg-primary)" : "transparent",
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 32,
        }}
      >
        {mark && (
          <div style={{ animation: "kaveon-breathe 3s ease-in-out infinite" }}>
            <KaveonMark size={56} useDirectColor />
          </div>
        )}

        <p
          style={{
            fontSize: 13,
            fontWeight: 400,
            letterSpacing: "0.1em",
            color: "var(--text-muted)",
            textTransform: "uppercase",
            minWidth: 120,
            textAlign: "center",
            margin: 0,
          }}
        >
          {message}{dots}
        </p>

        <div
          style={{
            width: 180,
            height: 2,
            background: "var(--border)",
            borderRadius: 1,
            overflow: "hidden",
          }}
        >
          <div
            style={{
              width: "35%",
              height: "100%",
              background: "linear-gradient(90deg, transparent, #4A9EE8, transparent)",
              animation: "loadSlide 1.8s ease-in-out infinite",
            }}
          />
        </div>

        {action}
      </div>

      <style>{`
        @keyframes loadSlide {
          0% { transform: translateX(-300%); }
          100% { transform: translateX(600%); }
        }
      `}</style>
    </div>
  );
}

export default KaveonLoading;
