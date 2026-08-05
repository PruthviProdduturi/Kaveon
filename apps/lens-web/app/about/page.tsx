"use client";

import React, { useState, useEffect, useCallback } from "react";
import Link from "next/link";
import { LensLogo, LensMark } from "../../components/LensLogo";

// ── Lens landing — dark/light, dense, polished ─────────────────────────────────

const CYAN = "#46c7d9";
const CYAN_LIGHT = "#7be0ef";
const CYAN_DARK = "#2aa6bb";

interface Theme {
  bg: string; bg2: string; surface: string; surfaceBorder: string;
  text: string; textMuted: string; textFaint: string;
  navBg: string; navBorder: string;
  cardBg: string; cardBorder: string;
  tableBg: string; tableRowHover: string;
  shotBg: string; shotBorder: string; shotSurface: string; shotSurfaceBorder: string;
  shotText: string; shotTextMuted: string;
}

const DARK: Theme = {
  bg: "#0a101e", bg2: "#0e1c33", surface: "#101d33", surfaceBorder: "rgba(255,255,255,0.08)",
  text: "#f4f8fc", textMuted: "#a9bace", textFaint: "#6b7d94",
  navBg: "rgba(10,16,30,0.82)", navBorder: "rgba(255,255,255,0.08)",
  cardBg: "linear-gradient(180deg, #101d33, #0c1524)", cardBorder: "rgba(255,255,255,0.07)",
  tableBg: "#0b1524", tableRowHover: "rgba(70,199,217,0.05)",
  shotBg: "linear-gradient(180deg, #101d33, #0b1526)", shotBorder: "rgba(124,224,239,0.18)",
  shotSurface: "rgba(255,255,255,0.025)", shotSurfaceBorder: "rgba(255,255,255,0.06)",
  shotText: CYAN_LIGHT, shotTextMuted: "#8496ad",
};
const LIGHT: Theme = {
  bg: "#ffffff", bg2: "#f8fafc", surface: "#f1f5f9", surfaceBorder: "#e2e8f0",
  text: "#0f172a", textMuted: "#475569", textFaint: "#94a3b8",
  navBg: "rgba(255,255,255,0.88)", navBorder: "#e2e8f0",
  cardBg: "#ffffff", cardBorder: "#e2e8f0",
  tableBg: "#ffffff", tableRowHover: "rgba(70,199,217,0.06)",
  shotBg: "linear-gradient(180deg, #f8fafc, #f1f5f9)", shotBorder: "#d1d9e6",
  shotSurface: "#ffffff", shotSurfaceBorder: "#e2e8f0",
  shotText: CYAN_DARK, shotTextMuted: "#64748b",
};

// ── Theme toggle button ──────────────────────────────────────────────────────
function ThemeToggle({ dark, onToggle }: { dark: boolean; onToggle: () => void }) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-label={dark ? "Switch to light mode" : "Switch to dark mode"}
      style={{
        background: "none", border: "none", cursor: "pointer", padding: 4,
        color: dark ? "#a9bace" : "#64748b", fontSize: 16, display: "flex",
        alignItems: "center", justifyContent: "center", width: 32, height: 32,
        borderRadius: 8, transition: "background 0.2s",
      }}
      onMouseEnter={e => (e.currentTarget.style.background = dark ? "rgba(255,255,255,0.08)" : "rgba(0,0,0,0.05)")}
      onMouseLeave={e => (e.currentTarget.style.background = "none")}
    >
      {dark ? (
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <circle cx="12" cy="12" r="5"/><path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/>
        </svg>
      ) : (
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>
        </svg>
      )}
    </button>
  );
}

// ── Nav ──────────────────────────────────────────────────────────────────────
function Nav({ t, dark, onToggle }: { t: Theme; dark: boolean; onToggle: () => void }) {
  return (
    <nav style={{
      position: "sticky", top: 0, zIndex: 50,
      display: "flex", alignItems: "center", justifyContent: "space-between",
      padding: "10px 28px", background: t.navBg,
      backdropFilter: "blur(14px)", borderBottom: `1px solid ${t.navBorder}`,
      transition: "background 0.3s, border-color 0.3s",
    }}>
      <LensLogo size={30} />
      <div style={{ display: "flex", alignItems: "center", gap: 20 }}>
        {["Features", "Compare", "Databases"].map(s => (
          <a key={s} href={`#${s.toLowerCase()}`} style={{ color: t.textMuted, textDecoration: "none", fontSize: 13, fontWeight: 500, transition: "color 0.2s" }}>{s}</a>
        ))}
        <ThemeToggle dark={dark} onToggle={onToggle} />
        <Link href="/login" style={{
          padding: "7px 16px", borderRadius: 8,
          background: `linear-gradient(135deg, ${CYAN_LIGHT}, ${CYAN})`,
          color: "#0a101e", fontWeight: 700, fontSize: 13, textDecoration: "none",
        }}>Sign in</Link>
      </div>
    </nav>
  );
}

// ── Product shot — realistic dashboard mockup ────────────────────────────────
function ProductShot({ t }: { t: Theme }) {
  const linePoints = [
    [0,72],[25,68],[50,60],[75,55],[100,58],[125,42],[150,38],[175,44],[200,30],
    [225,26],[250,32],[275,20],[300,24],[325,16],[350,12],[375,18],[400,14],
  ];
  const linePath = linePoints.map((p, i) => `${i === 0 ? "M" : "L"}${p[0]},${p[1]}`).join(" ");
  const areaPath = linePath + " L400,80 L0,80 Z";

  const bars = [
    { label: "US", h: 92 }, { label: "IN", h: 78 }, { label: "BR", h: 65 },
    { label: "FR", h: 52 }, { label: "DE", h: 48 }, { label: "UK", h: 44 },
    { label: "RU", h: 40 }, { label: "TR", h: 34 },
  ];

  const donutSegments = [
    { pct: 35, color: CYAN_LIGHT }, { pct: 25, color: CYAN },
    { pct: 20, color: CYAN_DARK }, { pct: 12, color: "#1d8a9e" },
    { pct: 8, color: "#156a7a" },
  ];
  let donutOffset = 0;

  const kpiStyle: React.CSSProperties = {
    background: t.shotSurface, border: `1px solid ${t.shotSurfaceBorder}`,
    borderRadius: 10, padding: "10px 12px", transition: "background 0.3s, border-color 0.3s",
  };
  const chartCard: React.CSSProperties = {
    ...kpiStyle, padding: "12px 14px",
  };
  const chartLabel: React.CSSProperties = { fontSize: 11, color: t.shotTextMuted, fontWeight: 600 };
  const badge: React.CSSProperties = {
    fontSize: 9, color: t.textFaint, padding: "2px 6px", borderRadius: 4,
    background: t === DARK ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.04)",
  };

  return (
    <div style={{
      borderRadius: 14, background: t.shotBg,
      border: `1px solid ${t.shotBorder}`,
      boxShadow: t === DARK
        ? "0 32px 80px rgba(0,0,0,0.5), 0 0 40px rgba(70,199,217,0.08)"
        : "0 24px 60px rgba(0,0,0,0.1), 0 0 0 1px rgba(0,0,0,0.03)",
      padding: 14, display: "grid", gap: 10,
      gridTemplateColumns: "1fr 1fr 1fr",
      transition: "background 0.3s, border-color 0.3s, box-shadow 0.3s",
    }}>
      {/* KPIs */}
      {([
        ["676.6M", "Total Cases", "+2.1%"],
        ["6.9M", "Total Deaths", "-0.4%"],
        ["13.5B", "Doses Given", "+1.8%"],
        ["201", "Countries", ""],
        ["4.2%", "Fatality Rate", "-0.2%"],
        ["68.3%", "Vaccinated", "+0.6%"],
      ] as const).map(([v, l, d]) => (
        <div key={l} style={kpiStyle}>
          <div style={{ display: "flex", alignItems: "baseline", gap: 6 }}>
            <span style={{ fontSize: 18, fontWeight: 800, color: t.shotText }}>{v}</span>
            {d && <span style={{ fontSize: 10, color: d.startsWith("-") ? "#e05252" : "#4ade80", fontWeight: 600 }}>{d}</span>}
          </div>
          <div style={{ fontSize: 10, color: t.textFaint, marginTop: 2 }}>{l}</div>
        </div>
      ))}

      {/* Line chart */}
      <div style={{ ...chartCard, gridColumn: "1 / 3" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
          <span style={chartLabel}>New Cases Over Time</span>
          <span style={badge}>Monthly</span>
        </div>
        <svg viewBox="0 0 400 85" width="100%" height="100" preserveAspectRatio="none" style={{ display: "block" }}>
          <defs>
            <linearGradient id="aGr" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={CYAN} stopOpacity="0.3" />
              <stop offset="100%" stopColor={CYAN} stopOpacity="0" />
            </linearGradient>
          </defs>
          {[20, 40, 60].map(y => <line key={y} x1="0" y1={y} x2="400" y2={y} stroke={t === DARK ? "rgba(255,255,255,0.04)" : "rgba(0,0,0,0.06)"} strokeWidth="0.5" />)}
          <path d={areaPath} fill="url(#aGr)" />
          <path d={linePath} fill="none" stroke={t.shotText} strokeWidth="2" strokeLinejoin="round" />
          {linePoints.filter((_, i) => i % 3 === 0).map((p, i) => (
            <circle key={i} cx={p[0]} cy={p[1]} r="2.5" fill={t.shotText} stroke={t === DARK ? "#0a101e" : "#fff"} strokeWidth="1" />
          ))}
        </svg>
      </div>

      {/* Donut */}
      <div style={{ ...chartCard, display: "flex", flexDirection: "column", alignItems: "center" }}>
        <span style={{ ...chartLabel, alignSelf: "flex-start", marginBottom: 6 }}>By Region</span>
        <svg viewBox="0 0 100 100" width="90" height="90">
          {donutSegments.map((s) => {
            const circ = Math.PI * 70;
            const dash = (s.pct / 100) * circ;
            const gap = circ - dash;
            const off = donutOffset;
            donutOffset += dash;
            return <circle key={s.color} cx="50" cy="50" r="35" fill="none" stroke={s.color} strokeWidth="10" strokeDasharray={`${dash} ${gap}`} strokeDashoffset={-off} transform="rotate(-90 50 50)" />;
          })}
          <text x="50" y="50" textAnchor="middle" dominantBaseline="middle" fill={t.text} fontSize="12" fontWeight="800">201</text>
          <text x="50" y="62" textAnchor="middle" fill={t.textFaint} fontSize="6">countries</text>
        </svg>
      </div>

      {/* Bar chart */}
      <div style={{ ...chartCard, gridColumn: "1 / 3" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <span style={chartLabel}>Top Countries by Cases</span>
          <span style={badge}>Cumulative</span>
        </div>
        <div style={{ display: "flex", alignItems: "flex-end", gap: 6, height: 80 }}>
          {bars.map((b) => (
            <div key={b.label} style={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", gap: 3 }}>
              <div style={{
                width: "100%", height: `${b.h}%`, borderRadius: "4px 4px 0 0",
                background: `linear-gradient(180deg, ${CYAN_LIGHT}, ${CYAN})`,
                opacity: 0.5 + (b.h / 200),
              }} />
              <span style={{ fontSize: 8, color: t.textFaint }}>{b.label}</span>
            </div>
          ))}
        </div>
      </div>

      {/* Severity bars */}
      <div style={chartCard}>
        <span style={{ ...chartLabel, display: "block", marginBottom: 6 }}>Severity Index</span>
        {([["Americas", 0.85], ["Europe", 0.72], ["SE Asia", 0.58], ["E. Med", 0.45], ["Africa", 0.28], ["W. Pac", 0.22]] as const).map(([region, val]) => (
          <div key={region} style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 3 }}>
            <span style={{ fontSize: 9, color: t.shotTextMuted, width: 48, flexShrink: 0 }}>{region}</span>
            <div style={{ flex: 1, height: 6, borderRadius: 3, background: t === DARK ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.06)", overflow: "hidden" }}>
              <div style={{
                width: `${val * 100}%`, height: "100%", borderRadius: 3,
                background: `linear-gradient(90deg, ${CYAN}, ${val > 0.6 ? "#e05252" : CYAN_LIGHT})`,
              }} />
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Hero ──────────────────────────────────────────────────────────────────────
function Hero({ t }: { t: Theme }) {
  return (
    <header style={{
      position: "relative", overflow: "hidden",
      background: t === DARK
        ? `radial-gradient(1100px 520px at 50% -5%, #103240 0%, ${t.bg} 55%)`
        : `radial-gradient(1100px 520px at 50% -5%, #e0f4f8 0%, ${t.bg} 55%)`,
      padding: "44px 24px 48px",
      transition: "background 0.3s",
    }}>
      <div style={{ maxWidth: 1080, margin: "0 auto", textAlign: "center" }}>
        <div style={{ display: "flex", justifyContent: "center", marginBottom: 14 }}>
          <div style={{ display: "inline-flex", filter: t === DARK ? "drop-shadow(0 0 28px rgba(70,199,217,0.4))" : "drop-shadow(0 0 20px rgba(70,199,217,0.2))" }}>
            <LensMark size={56} />
          </div>
        </div>
        <div style={{
          display: "inline-flex", alignItems: "center", gap: 8, padding: "4px 12px",
          borderRadius: 999,
          border: t === DARK ? "1px solid rgba(124,224,239,0.3)" : "1px solid rgba(42,166,187,0.25)",
          background: t === DARK ? "rgba(124,224,239,0.08)" : "rgba(42,166,187,0.06)",
          color: t === DARK ? CYAN_LIGHT : CYAN_DARK, fontSize: 11.5, fontWeight: 600, marginBottom: 16,
        }}>
          ✦ AI-native · Self-hosted · Open source
        </div>
        <h1 style={{
          fontSize: "clamp(36px, 6vw, 64px)", fontWeight: 850, lineHeight: 1.05, letterSpacing: "-0.02em",
          margin: 0, color: t.text, transition: "color 0.3s",
        }}>
          See the pattern.
        </h1>
        <p style={{
          fontSize: "clamp(14px, 2vw, 18px)", color: t.textMuted, maxWidth: 580,
          margin: "14px auto 0", lineHeight: 1.5, transition: "color 0.3s",
        }}>
          The analytics platform for <strong style={{ color: t.text }}>Microsoft Fabric</strong> and{" "}
          <strong style={{ color: t.text }}>Trino</strong>.
          Query live data, build 20+ chart types, assemble dashboards, and ask questions in plain English.
        </p>
        <style>{`@keyframes ctaPulse { 0%, 100% { box-shadow: 0 8px 24px rgba(70,199,217,0.3); } 50% { box-shadow: 0 8px 36px rgba(70,199,217,0.5), 0 0 16px rgba(70,199,217,0.2); } }`}</style>
        <div style={{ display: "flex", gap: 12, justifyContent: "center", marginTop: 22, flexWrap: "wrap" }}>
          <Link href="/" style={{
            padding: "13px 32px", borderRadius: 10, fontSize: 15, fontWeight: 700, textDecoration: "none",
            background: `linear-gradient(135deg, ${CYAN_LIGHT}, ${CYAN})`, color: "#0a101e",
            animation: "ctaPulse 2.5s ease-in-out infinite",
          }}>Get started →</Link>
          <a href="https://github.com/PruthviProdduturi/Lens" target="_blank" rel="noreferrer" style={{
            padding: "13px 24px", borderRadius: 10, fontSize: 15, fontWeight: 700, textDecoration: "none",
            background: t === DARK ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.04)",
            color: t === DARK ? "#e6eef7" : "#1e293b",
            border: `1px solid ${t.surfaceBorder}`,
          }}>GitHub ▸</a>
        </div>
        <div style={{ marginTop: 32, maxWidth: 940, marginInline: "auto" }}>
          <ProductShot t={t} />
        </div>
      </div>
    </header>
  );
}

// ── Features ─────────────────────────────────────────────────────────────────
const FEATURES = [
  { icon: "fa-flask", title: "SQL Lab", body: "Monaco editor, multi-tab, query history, async execution, result caching." },
  { icon: "fa-wand-magic-sparkles", title: "AI, built in", body: "Natural-language → SQL. Anthropic, OpenAI, or GitHub Models — your keys." },
  { icon: "fa-chart-column", title: "20+ chart types", body: "Lines, bars, pies, heatmaps, treemaps, 3D WebGL globe, cross-filtering." },
  { icon: "fa-table-columns", title: "Dashboards", body: "Drag-and-drop, rows, tabs, text, shared filters that slice every tile." },
  { icon: "fa-database", title: "Multi-source", body: "Fabric SQL, Azure SQL, PostgreSQL, MySQL — Trino & StarRocks coming." },
  { icon: "fa-shield-halved", title: "Enterprise auth", body: "GitHub, Google, Microsoft sign-in with 4-tier RBAC and per-object visibility." },
];

function Features({ t }: { t: Theme }) {
  return (
    <section id="features" style={{ background: t.bg, padding: "48px 24px", transition: "background 0.3s" }}>
      <div style={{ maxWidth: 1120, margin: "0 auto" }}>
        <h2 style={{ textAlign: "center", fontSize: 28, fontWeight: 800, color: t.text, margin: 0, transition: "color 0.3s" }}>
          Everything to explore, visualise, and share
        </h2>
        <p style={{ textAlign: "center", color: t.textMuted, fontSize: 14, marginTop: 8, transition: "color 0.3s" }}>
          A modern command centre for your data — not a dashboard bolted onto a warehouse.
        </p>
        <div style={{
          display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(280px, 1fr))",
          gap: 12, marginTop: 28,
        }}>
          {FEATURES.map((f) => (
            <div key={f.title} style={{
              background: t.cardBg, border: `1px solid ${t.cardBorder}`,
              borderRadius: 12, padding: "18px 18px", transition: "background 0.3s, border-color 0.3s",
            }}>
              <div style={{
                width: 36, height: 36, borderRadius: 9, display: "flex", alignItems: "center", justifyContent: "center",
                background: t === DARK ? "rgba(70,199,217,0.12)" : "rgba(70,199,217,0.08)",
                border: t === DARK ? "1px solid rgba(70,199,217,0.25)" : "1px solid rgba(70,199,217,0.2)",
                marginBottom: 10,
              }}>
                <i className={`fas ${f.icon}`} style={{ color: t === DARK ? CYAN_LIGHT : CYAN_DARK, fontSize: 15 }} />
              </div>
              <div style={{ fontSize: 15, fontWeight: 700, color: t.text, transition: "color 0.3s" }}>{f.title}</div>
              <div style={{ fontSize: 13, color: t.textMuted, lineHeight: 1.5, marginTop: 4, transition: "color 0.3s" }}>{f.body}</div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

// ── Database logos ────────────────────────────────────────────────────────────
const DBS = [
  { name: "Microsoft Fabric", status: "live" },
  { name: "Azure SQL", status: "live" },
  { name: "PostgreSQL", status: "live" },
  { name: "MySQL", status: "live" },
  { name: "Trino", status: "soon" },
  { name: "StarRocks", status: "soon" },
  { name: "Snowflake", status: "planned" },
  { name: "BigQuery", status: "planned" },
];

function Databases({ t }: { t: Theme }) {
  return (
    <section id="databases" style={{ background: t.bg2, padding: "48px 24px", transition: "background 0.3s" }}>
      <div style={{ maxWidth: 900, margin: "0 auto" }}>
        <h2 style={{ textAlign: "center", fontSize: 28, fontWeight: 800, color: t.text, margin: 0, transition: "color 0.3s" }}>
          Connect to anything
        </h2>
        <p style={{ textAlign: "center", color: t.textMuted, fontSize: 14, marginTop: 8, marginBottom: 28, transition: "color 0.3s" }}>
          One platform, every database. More connectors shipping fast.
        </p>
        <div style={{
          display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(180px, 1fr))",
          gap: 10,
        }}>
          {DBS.map((db) => (
            <div key={db.name} style={{
              display: "flex", alignItems: "center", gap: 10,
              padding: "12px 14px", borderRadius: 10,
              background: t === DARK ? "rgba(255,255,255,0.03)" : "#ffffff",
              border: `1px solid ${t.cardBorder}`,
              transition: "background 0.3s, border-color 0.3s",
            }}>
              <i className="fas fa-database" style={{ color: db.status === "live" ? CYAN : t.textFaint, fontSize: 14 }} />
              <span style={{ fontSize: 13, fontWeight: 600, color: t.text, flex: 1, transition: "color 0.3s" }}>{db.name}</span>
              <span style={{
                fontSize: 9, fontWeight: 700, padding: "2px 6px", borderRadius: 6, textTransform: "uppercase",
                ...(db.status === "live" ? {
                  color: "#4ade80", background: "rgba(74,222,128,0.1)", border: "1px solid rgba(74,222,128,0.2)",
                } : db.status === "soon" ? {
                  color: "#fbbf24", background: "rgba(251,191,36,0.1)", border: "1px solid rgba(251,191,36,0.2)",
                } : {
                  color: t.textFaint, background: t === DARK ? "rgba(255,255,255,0.04)" : "rgba(0,0,0,0.03)",
                  border: `1px solid ${t.surfaceBorder}`,
                }),
              }}>
                {db.status === "live" ? "Live" : db.status === "soon" ? "Soon" : "Planned"}
              </span>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

// ── Compare ──────────────────────────────────────────────────────────────────
const ROWS: [string, boolean | "~", boolean | "~", boolean | "~", boolean | "~"][] = [
  ["Modern, fast UI", true, "~", true, "~"],
  ["AI natural-language → SQL", true, false, true, false],
  ["Built-in SQL Lab", true, "~", false, true],
  ["Self-hosted + open source (MIT)", true, true, false, true],
  ["20+ chart types incl. 3D globe", true, "~", true, "~"],
  ["Microsoft Fabric-native", true, "~", true, false],
];

function Cell({ v, t }: { v: boolean | "~"; t: Theme }) {
  if (v === "~") return <span style={{ color: "#c79a3a" }}>~</span>;
  return v
    ? <i className="fas fa-check" style={{ color: CYAN }} />
    : <i className="fas fa-minus" style={{ color: t.textFaint }} />;
}

function Compare({ t }: { t: Theme }) {
  const th: React.CSSProperties = { padding: "10px 10px", fontSize: 12.5, color: t.textMuted, fontWeight: 700, textAlign: "center", transition: "color 0.3s" };
  const td: React.CSSProperties = { padding: "9px 10px", textAlign: "center", borderTop: `1px solid ${t.surfaceBorder}`, transition: "border-color 0.3s" };
  return (
    <section id="compare" style={{ background: t.bg, padding: "48px 24px", transition: "background 0.3s" }}>
      <div style={{ maxWidth: 860, margin: "0 auto" }}>
        <h2 style={{ textAlign: "center", fontSize: 28, fontWeight: 800, color: t.text, margin: 0, transition: "color 0.3s" }}>
          How Lens compares
        </h2>
        <p style={{ textAlign: "center", color: t.textMuted, fontSize: 14, marginTop: 8, marginBottom: 24, transition: "color 0.3s" }}>
          An honest look next to Superset, Power BI, and Redash.
        </p>
        <div style={{ overflowX: "auto", borderRadius: 12, border: `1px solid ${t.cardBorder}`, background: t.tableBg, transition: "background 0.3s, border-color 0.3s" }}>
          <table style={{ width: "100%", borderCollapse: "collapse", minWidth: 600 }}>
            <thead>
              <tr style={{ background: t === DARK ? "rgba(255,255,255,0.03)" : "rgba(0,0,0,0.02)" }}>
                <th style={{ ...th, textAlign: "left", paddingLeft: 16 }}>Capability</th>
                <th style={{ ...th, color: t === DARK ? CYAN_LIGHT : CYAN_DARK }}>Lens</th>
                <th style={th}>Superset</th>
                <th style={th}>Power BI</th>
                <th style={th}>Redash</th>
              </tr>
            </thead>
            <tbody>
              {ROWS.map((r) => (
                <tr key={r[0]}>
                  <td style={{ ...td, textAlign: "left", paddingLeft: 16, color: t.text, fontSize: 13, fontWeight: 600 }}>{r[0]}</td>
                  <td style={{ ...td, background: t.tableRowHover }}><Cell v={r[1]} t={t} /></td>
                  <td style={td}><Cell v={r[2]} t={t} /></td>
                  <td style={td}><Cell v={r[3]} t={t} /></td>
                  <td style={td}><Cell v={r[4]} t={t} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </section>
  );
}

// ── CTA + footer ─────────────────────────────────────────────────────────────
function CTA({ t }: { t: Theme }) {
  return (
    <section style={{
      background: t === DARK
        ? `radial-gradient(700px 350px at 50% 120%, #10404f, ${t.bg})`
        : `radial-gradient(700px 350px at 50% 120%, #e0f4f8, ${t.bg})`,
      padding: "48px 24px 40px", textAlign: "center", transition: "background 0.3s",
    }}>
      <h2 style={{ fontSize: 32, fontWeight: 850, color: t.text, margin: 0, letterSpacing: "-0.02em", transition: "color 0.3s" }}>
        Bring your data into focus.
      </h2>
      <p style={{ color: t.textMuted, fontSize: 15, marginTop: 8, transition: "color 0.3s" }}>
        Self-hosted, open source, and ready in minutes.
      </p>
      <div style={{ display: "flex", gap: 12, justifyContent: "center", marginTop: 22, flexWrap: "wrap" }}>
        <Link href="/login" style={{
          padding: "11px 28px", borderRadius: 10, fontSize: 14, fontWeight: 700, textDecoration: "none",
          background: `linear-gradient(135deg, ${CYAN_LIGHT}, ${CYAN})`, color: "#0a101e",
          boxShadow: "0 8px 24px rgba(70,199,217,0.3)",
        }}>Get started free</Link>
        <a href="https://github.com/PruthviProdduturi/Lens" target="_blank" rel="noreferrer" style={{
          padding: "11px 24px", borderRadius: 10, fontSize: 14, fontWeight: 700, textDecoration: "none",
          background: t === DARK ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.04)",
          color: t === DARK ? "#e6eef7" : "#1e293b",
          border: `1px solid ${t.surfaceBorder}`,
        }}>View on GitHub</a>
      </div>
      <div style={{ marginTop: 32, color: t.textFaint, fontSize: 12, transition: "color 0.3s" }}>
        © {new Date().getFullYear()} Lens — a Kaveon platform module. See the pattern.
      </div>
    </section>
  );
}

// ── Page ─────────────────────────────────────────────────────────────────────
export default function AboutPage() {
  const [dark, setDark] = useState(false);
  const t = dark ? DARK : LIGHT;

  // Persist preference
  useEffect(() => {
    const saved = localStorage.getItem("lens-about-theme");
    if (saved === "dark") setDark(true);
  }, []);
  const toggle = useCallback(() => {
    setDark(prev => {
      localStorage.setItem("lens-about-theme", prev ? "light" : "dark");
      return !prev;
    });
  }, []);

  return (
    <div style={{
      background: t.bg, minHeight: "100vh",
      fontFamily: "'Inter', 'Segoe UI', system-ui, sans-serif",
      transition: "background 0.3s",
    }}>
      <Nav t={t} dark={dark} onToggle={toggle} />
      <Hero t={t} />
      <Features t={t} />
      <Databases t={t} />
      <Compare t={t} />
      <CTA t={t} />
    </div>
  );
}
