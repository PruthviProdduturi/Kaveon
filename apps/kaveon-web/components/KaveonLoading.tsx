"use client";

import React, { useState, useEffect } from "react";

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
        overflow: "hidden",
      }}
    >
      {/* Background ambient glow */}
      <div
        style={{
          position: "absolute",
          width: 500,
          height: 500,
          borderRadius: "50%",
          background: "radial-gradient(circle, rgba(74,158,232,0.08) 0%, transparent 70%)",
          pointerEvents: "none",
        }}
      />

      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 40,
          position: "relative",
        }}
      >
        {/* Spinning rings container */}
        <div
          style={{
            position: "relative",
            width: 140,
            height: 140,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          {/* Outer ring — slow spin */}
          <div
            style={{
              position: "absolute",
              inset: 0,
              borderRadius: "50%",
              border: "1px solid rgba(74,158,232,0.1)",
              animation: "loadSpin 8s linear infinite",
            }}
          >
            {/* Orbital dot */}
            <div
              style={{
                position: "absolute",
                top: -3,
                left: "50%",
                marginLeft: -3,
                width: 6,
                height: 6,
                borderRadius: "50%",
                background: "#4A9EE8",
                boxShadow: "0 0 12px rgba(74,158,232,0.6)",
              }}
            />
          </div>

          {/* Middle ring — medium spin reverse */}
          <div
            style={{
              position: "absolute",
              inset: 18,
              borderRadius: "50%",
              border: "1px solid rgba(74,158,232,0.06)",
              animation: "loadSpinReverse 6s linear infinite",
            }}
          >
            <div
              style={{
                position: "absolute",
                bottom: -2,
                left: "50%",
                marginLeft: -2,
                width: 4,
                height: 4,
                borderRadius: "50%",
                background: "#4A9EE8",
                opacity: 0.6,
                boxShadow: "0 0 8px rgba(74,158,232,0.4)",
              }}
            />
          </div>

          {/* Guardian O — spinning at center */}
          <div
            style={{
              animation: "loadSpin 3s linear infinite",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
            }}
          >
            <svg
              width="64"
              height="64"
              viewBox="0 0 512 512"
              fill="none"
            >
              <path
                d="M 343.68 407.88 A 124 124 0 1 0 168.32 407.88"
                stroke="#4A9EE8"
                strokeWidth="52"
                fill="none"
                strokeLinecap="butt"
              />
            </svg>
          </div>

          {/* Glow pulse behind the O */}
          <div
            style={{
              position: "absolute",
              inset: 30,
              borderRadius: "50%",
              background: "radial-gradient(circle, rgba(74,158,232,0.12) 0%, transparent 70%)",
              animation: "loadPulse 2s ease-in-out infinite",
              pointerEvents: "none",
            }}
          />
        </div>

        {/* Text + progress */}
        <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 16 }}>
          <p
            style={{
              fontSize: 13,
              fontWeight: 400,
              letterSpacing: "0.15em",
              color: "#475569",
              textTransform: "uppercase",
              margin: 0,
              minWidth: 120,
              textAlign: "center",
            }}
          >
            {message}{dots}
          </p>

          {/* Thin progress bar */}
          <div
            style={{
              width: 160,
              height: 1.5,
              background: "rgba(255,255,255,0.04)",
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
      </div>

      <style>{`
        @keyframes loadSpin {
          from { transform: rotate(0deg); }
          to { transform: rotate(360deg); }
        }
        @keyframes loadSpinReverse {
          from { transform: rotate(360deg); }
          to { transform: rotate(0deg); }
        }
        @keyframes loadPulse {
          0%, 100% { opacity: 0.3; transform: scale(1); }
          50% { opacity: 0.8; transform: scale(1.1); }
        }
        @keyframes loadSlide {
          0% { transform: translateX(-300%); }
          100% { transform: translateX(600%); }
        }
      `}</style>
    </div>
  );
}

export default KaveonLoading;
