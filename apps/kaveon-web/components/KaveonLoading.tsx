"use client";

import React, { useState, useEffect } from "react";
import { KaveonMark } from "./KaveonMark";

interface KaveonLoadingProps {
  message?: string;
  fullScreen?: boolean;
}

export function KaveonLoading({ message = "Loading" }: KaveonLoadingProps) {
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
        position: "fixed",
        inset: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: "var(--bg-primary)",
        zIndex: 9999,
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
        <div style={{ animation: "kaveon-breathe 3s ease-in-out infinite" }}>
          <KaveonMark size={56} useDirectColor />
        </div>

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
