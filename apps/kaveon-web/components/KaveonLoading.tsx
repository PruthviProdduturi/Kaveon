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
        background: "#09090b",
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
            color: "#475569",
            textTransform: "uppercase",
            minWidth: 120,
            textAlign: "center",
            margin: 0,
          }}
        >
          {message}{dots}
        </p>
      </div>
    </div>
  );
}

export default KaveonLoading;
