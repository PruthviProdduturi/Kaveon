"use client";

import React from "react";
import Link from "next/link";
import { LensLogo, LensMark } from "../../components/LensLogo";

// ── Lens landing / about — dense dark hero + realistic product shot ───────────
// Brand: cyan aperture, gradient #7be0ef → #2aa6bb, accent #46c7d9.

const CYAN = "#46c7d9";
const CYAN_LIGHT = "#7be0ef";
const INK = "#0a101e";
const INK2 = "#0e1c33";

function Nav() {
  return (
    <nav style={{
      position: "sticky", top: 0, zIndex: 50,
      display: "flex", alignItems: "center", justifyContent: "space-between",
      padding: "10px 28px", background: "rgba(10,16,30,0.82)",
      backdropFilter: "blur(12px)", borderBottom: "1px solid rgba(255,255,255,0.08)",
    }}>
      <LensLogo size={30} />
      <div style={{ display: "flex", alignItems: "center", gap: 24 }}>
        <a href="#features" style={{ color: "#c7d2e0", textDecoration: "none", fontSize: 13, fontWeight: 500 }}>Features</a>
        <a href="#compare" style={{ color: "#c7d2e0", textDecoration: "none", fontSize: 13, fontWeight: 500 }}>Compare</a>
        <a href="#stack" style={{ color: "#c7d2e0", textDecoration: "none", fontSize: 13, fontWeight: 500 }}>Platform</a>
        <Link href="/login" style={{
          padding: "7px 16px", borderRadius: 8,
          background: `linear-gradient(135deg, ${CYAN_LIGHT}, ${CYAN})`,
          color: INK, fontWeight: 700, fontSize: 13, textDecoration: "none",
        }}>Sign in</Link>
      </div>
    </nav>
  );
}

// ── Realistic product shot — mimics a real Lens dashboard ────────────────────
function ProductShot() {
  // Line chart data points (monthly trend, normalized to SVG viewBox)
  const linePoints = [
    [0,72],[25,68],[50,60],[75,55],[100,58],[125,42],[150,38],[175,44],[200,30],
    [225,26],[250,32],[275,20],[300,24],[325,16],[350,12],[375,18],[400,14],
  ];
  const linePath = linePoints.map((p, i) => `${i === 0 ? "M" : "L"}${p[0]},${p[1]}`).join(" ");
  const areaPath = linePath + ` L400,80 L0,80 Z`;

  // Bar chart data (top 8 countries)
  const bars = [
    { label: "US", h: 92 }, { label: "IN", h: 78 }, { label: "BR", h: 65 },
    { label: "FR", h: 52 }, { label: "DE", h: 48 }, { label: "UK", h: 44 },
    { label: "RU", h: 40 }, { label: "TR", h: 34 },
  ];

  // Donut chart segments
  const donutSegments = [
    { pct: 35, color: CYAN_LIGHT }, { pct: 25, color: CYAN },
    { pct: 20, color: "#2aa6bb" }, { pct: 12, color: "#1d8a9e" },
    { pct: 8, color: "#156a7a" },
  ];
  let donutOffset = 0;

  return (
    <div style={{
      borderRadius: 14, background: "linear-gradient(180deg, #101d33, #0b1526)",
      border: "1px solid rgba(124,224,239,0.18)",
      boxShadow: "0 32px 80px rgba(0,0,0,0.5), 0 0 40px rgba(70,199,217,0.08)",
      padding: 14, display: "grid", gap: 10,
      gridTemplateColumns: "1fr 1fr 1fr",
    }}>
      {/* KPI row */}
      {[
        ["676.6M", "Total Cases", "+2.1%"],
        ["6.9M", "Total Deaths", "-0.4%"],
        ["13.5B", "Doses Given", "+1.8%"],
        ["201", "Countries", ""],
        ["4.2%", "Fatality Rate", "-0.2%"],
        ["68.3%", "Vaccinated", "+0.6%"],
      ].map(([v, l, d]) => (
        <div key={l} style={{
          background: "rgba(255,255,255,0.025)", border: "1px solid rgba(255,255,255,0.06)",
          borderRadius: 10, padding: "10px 12px",
        }}>
          <div style={{ display: "flex", alignItems: "baseline", gap: 6 }}>
            <span style={{ fontSize: 18, fontWeight: 800, color: CYAN_LIGHT }}>{v}</span>
            {d && <span style={{ fontSize: 10, color: (d as string).startsWith("-") ? "#e05252" : "#4ade80", fontWeight: 600 }}>{d}</span>}
          </div>
          <div style={{ fontSize: 10, color: "#6b7d94", marginTop: 2 }}>{l}</div>
        </div>
      ))}

      {/* Line chart — spans 2 columns */}
      <div style={{
        gridColumn: "1 / 3", background: "rgba(255,255,255,0.025)",
        border: "1px solid rgba(255,255,255,0.06)", borderRadius: 10, padding: "10px 12px",
      }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
          <span style={{ fontSize: 11, color: "#8496ad", fontWeight: 600 }}>New Cases Over Time</span>
          <span style={{ fontSize: 9, color: "#5a6b7d", padding: "2px 6px", borderRadius: 4, background: "rgba(255,255,255,0.05)" }}>Monthly</span>
        </div>
        <svg viewBox="0 0 400 85" width="100%" height="100" preserveAspectRatio="none" style={{ display: "block" }}>
          <defs>
            <linearGradient id="areaGrad" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={CYAN} stopOpacity="0.3" />
              <stop offset="100%" stopColor={CYAN} stopOpacity="0" />
            </linearGradient>
          </defs>
          {/* Grid lines */}
          {[20, 40, 60].map(y => <line key={y} x1="0" y1={y} x2="400" y2={y} stroke="rgba(255,255,255,0.04)" strokeWidth="0.5" />)}
          <path d={areaPath} fill="url(#areaGrad)" />
          <path d={linePath} fill="none" stroke={CYAN_LIGHT} strokeWidth="2" strokeLinejoin="round" />
          {/* Data point dots */}
          {linePoints.filter((_, i) => i % 3 === 0).map((p, i) => (
            <circle key={i} cx={p[0]} cy={p[1]} r="2.5" fill={CYAN_LIGHT} stroke={INK} strokeWidth="1" />
          ))}
        </svg>
      </div>

      {/* Donut chart */}
      <div style={{
        background: "rgba(255,255,255,0.025)", border: "1px solid rgba(255,255,255,0.06)",
        borderRadius: 10, padding: "10px 12px", display: "flex", flexDirection: "column", alignItems: "center",
      }}>
        <span style={{ fontSize: 11, color: "#8496ad", fontWeight: 600, alignSelf: "flex-start", marginBottom: 6 }}>By Region</span>
        <svg viewBox="0 0 100 100" width="90" height="90">
          {donutSegments.map((s) => {
            const circumference = Math.PI * 70;
            const dashLen = (s.pct / 100) * circumference;
            const dashGap = circumference - dashLen;
            const offset = donutOffset;
            donutOffset += dashLen;
            return (
              <circle key={s.color} cx="50" cy="50" r="35" fill="none" stroke={s.color} strokeWidth="10"
                strokeDasharray={`${dashLen} ${dashGap}`} strokeDashoffset={-offset}
                transform="rotate(-90 50 50)" />
            );
          })}
          <text x="50" y="50" textAnchor="middle" dominantBaseline="middle" fill="#eaf1f8" fontSize="12" fontWeight="800">201</text>
          <text x="50" y="62" textAnchor="middle" fill="#6b7d94" fontSize="6">countries</text>
        </svg>
      </div>

      {/* Bar chart — spans 2 columns */}
      <div style={{
        gridColumn: "1 / 3", background: "rgba(255,255,255,0.025)",
        border: "1px solid rgba(255,255,255,0.06)", borderRadius: 10, padding: "10px 12px",
      }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <span style={{ fontSize: 11, color: "#8496ad", fontWeight: 600 }}>Top Countries by Cases</span>
          <span style={{ fontSize: 9, color: "#5a6b7d", padding: "2px 6px", borderRadius: 4, background: "rgba(255,255,255,0.05)" }}>Cumulative</span>
        </div>
        <div style={{ display: "flex", alignItems: "flex-end", gap: 6, height: 80 }}>
          {bars.map((b) => (
            <div key={b.label} style={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", gap: 3 }}>
              <div style={{
                width: "100%", height: `${b.h}%`, borderRadius: "4px 4px 0 0",
                background: `linear-gradient(180deg, ${CYAN_LIGHT}, ${CYAN})`,
                opacity: 0.5 + (b.h / 200),
              }} />
              <span style={{ fontSize: 8, color: "#6b7d94" }}>{b.label}</span>
            </div>
          ))}
        </div>
      </div>

      {/* Heatmap / mini table — 1 column */}
      <div style={{
        background: "rgba(255,255,255,0.025)", border: "1px solid rgba(255,255,255,0.06)",
        borderRadius: 10, padding: "10px 12px",
      }}>
        <span style={{ fontSize: 11, color: "#8496ad", fontWeight: 600, display: "block", marginBottom: 6 }}>Severity Index</span>
        {[
          ["Americas", 0.85], ["Europe", 0.72], ["SE Asia", 0.58],
          ["E. Med", 0.45], ["Africa", 0.28], ["W. Pac", 0.22],
        ].map(([region, val]) => (
          <div key={region as string} style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 3 }}>
            <span style={{ fontSize: 9, color: "#8496ad", width: 48, flexShrink: 0 }}>{region}</span>
            <div style={{ flex: 1, height: 6, borderRadius: 3, background: "rgba(255,255,255,0.05)", overflow: "hidden" }}>
              <div style={{
                width: `${(val as number) * 100}%`, height: "100%", borderRadius: 3,
                background: `linear-gradient(90deg, ${CYAN}, ${(val as number) > 0.6 ? "#e05252" : CYAN_LIGHT})`,
              }} />
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function Hero() {
  return (
    <header style={{
      position: "relative", overflow: "hidden",
      background: `radial-gradient(1100px 520px at 50% -5%, #103240 0%, ${INK} 55%)`,
      padding: "44px 24px 48px",
    }}>
      <div style={{ maxWidth: 1080, margin: "0 auto", textAlign: "center" }}>
        <div style={{ display: "flex", justifyContent: "center", marginBottom: 14 }}>
          <div style={{
            display: "inline-flex",
            filter: "drop-shadow(0 0 28px rgba(70,199,217,0.4))",
          }}>
            <LensMark size={56} />
          </div>
        </div>
        <div style={{
          display: "inline-flex", alignItems: "center", gap: 8, padding: "4px 12px",
          borderRadius: 999, border: "1px solid rgba(124,224,239,0.3)",
          background: "rgba(124,224,239,0.08)", color: CYAN_LIGHT, fontSize: 11.5, fontWeight: 600, marginBottom: 16,
        }}>
          ✦ AI-native · Self-hosted · Open source
        </div>
        <h1 style={{
          fontSize: "clamp(36px, 6vw, 64px)", fontWeight: 850, lineHeight: 1.05, letterSpacing: "-0.02em",
          margin: 0, color: "#f4f8fc",
        }}>
          See the pattern.
        </h1>
        <p style={{
          fontSize: "clamp(14px, 2vw, 18px)", color: "#a9bace", maxWidth: 580,
          margin: "14px auto 0", lineHeight: 1.5,
        }}>
          The analytics platform for <strong style={{ color: "#e6eef7" }}>Microsoft Fabric</strong> and{" "}
          <strong style={{ color: "#e6eef7" }}>Trino</strong>.
          Query live data, build 20+ chart types, assemble dashboards, and ask questions in plain English.
        </p>
        <div style={{ display: "flex", gap: 12, justifyContent: "center", marginTop: 22, flexWrap: "wrap" }}>
          <Link href="/login" style={{
            padding: "11px 26px", borderRadius: 10, fontSize: 14, fontWeight: 700, textDecoration: "none",
            background: `linear-gradient(135deg, ${CYAN_LIGHT}, ${CYAN})`, color: INK,
            boxShadow: "0 8px 24px rgba(70,199,217,0.3)",
          }}>Get started</Link>
          <a href="https://github.com/PruthviProdduturi/Lens" target="_blank" rel="noreferrer" style={{
            padding: "11px 24px", borderRadius: 10, fontSize: 14, fontWeight: 700, textDecoration: "none",
            background: "rgba(255,255,255,0.06)", color: "#e6eef7", border: "1px solid rgba(255,255,255,0.14)",
          }}>GitHub ▸</a>
        </div>
        <div style={{ marginTop: 32, maxWidth: 940, marginInline: "auto" }}>
          <ProductShot />
        </div>
      </div>
    </header>
  );
}

const FEATURES = [
  { icon: "fa-flask", title: "SQL Lab", body: "Monaco editor, multi-tab, query history, async execution, result caching." },
  { icon: "fa-wand-magic-sparkles", title: "AI, built in", body: "Natural-language → SQL. Anthropic, OpenAI, or GitHub Models — your keys." },
  { icon: "fa-chart-column", title: "20+ chart types", body: "Lines, bars, pies, heatmaps, treemaps, 3D WebGL globe, cross-filtering." },
  { icon: "fa-table-columns", title: "Dashboards", body: "Drag-and-drop, rows, tabs, text, shared filters that slice every tile." },
  { icon: "fa-database", title: "Multi-source", body: "Fabric SQL, Azure SQL, PostgreSQL, MySQL — Trino & StarRocks coming." },
  { icon: "fa-shield-halved", title: "Enterprise auth", body: "GitHub, Google, Microsoft sign-in with 4-tier RBAC and per-object visibility." },
];

function Features() {
  return (
    <section id="features" style={{ background: INK, padding: "48px 24px" }}>
      <div style={{ maxWidth: 1120, margin: "0 auto" }}>
        <h2 style={{ textAlign: "center", fontSize: 28, fontWeight: 800, color: "#f4f8fc", margin: 0 }}>
          Everything to explore, visualise, and share
        </h2>
        <p style={{ textAlign: "center", color: "#8fa2ba", fontSize: 14, marginTop: 8 }}>
          A modern command centre for your data — not a dashboard bolted onto a warehouse.
        </p>
        <div style={{
          display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(280px, 1fr))",
          gap: 12, marginTop: 28,
        }}>
          {FEATURES.map((f) => (
            <div key={f.title} style={{
              background: "linear-gradient(180deg, #101d33, #0c1524)",
              border: "1px solid rgba(255,255,255,0.07)", borderRadius: 12, padding: "18px 18px",
            }}>
              <div style={{
                width: 36, height: 36, borderRadius: 9, display: "flex", alignItems: "center", justifyContent: "center",
                background: "rgba(70,199,217,0.12)", border: "1px solid rgba(70,199,217,0.25)", marginBottom: 10,
              }}>
                <i className={`fas ${f.icon}`} style={{ color: CYAN_LIGHT, fontSize: 15 }} />
              </div>
              <div style={{ fontSize: 15, fontWeight: 700, color: "#eaf1f8" }}>{f.title}</div>
              <div style={{ fontSize: 13, color: "#93a5bd", lineHeight: 1.5, marginTop: 4 }}>{f.body}</div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

const ROWS: Array<[string, boolean | "~", boolean | "~", boolean | "~", boolean | "~"]> = [
  ["Modern, fast UI", true, "~", true, "~"],
  ["AI natural-language → SQL", true, false, true, false],
  ["Built-in SQL Lab", true, "~", false, true],
  ["Self-hosted + open source (MIT)", true, true, false, true],
  ["20+ chart types incl. 3D globe", true, "~", true, "~"],
  ["Microsoft Fabric-native", true, "~", true, false],
];

function Cell({ v }: { v: boolean | "~" }) {
  if (v === "~") return <span style={{ color: "#c79a3a" }}>~</span>;
  return v
    ? <i className="fas fa-check" style={{ color: CYAN }} />
    : <i className="fas fa-minus" style={{ color: "#5a6b7d" }} />;
}

function Compare() {
  const th: React.CSSProperties = { padding: "10px 10px", fontSize: 12.5, color: "#aebfd4", fontWeight: 700, textAlign: "center" };
  const td: React.CSSProperties = { padding: "9px 10px", textAlign: "center", borderTop: "1px solid rgba(255,255,255,0.05)" };
  return (
    <section id="compare" style={{ background: INK2, padding: "48px 24px" }}>
      <div style={{ maxWidth: 860, margin: "0 auto" }}>
        <h2 style={{ textAlign: "center", fontSize: 28, fontWeight: 800, color: "#f4f8fc", margin: 0 }}>
          How Lens compares
        </h2>
        <p style={{ textAlign: "center", color: "#8fa2ba", fontSize: 14, marginTop: 8, marginBottom: 24 }}>
          An honest look next to Superset, Power BI, and Redash.
        </p>
        <div style={{ overflowX: "auto", borderRadius: 12, border: "1px solid rgba(255,255,255,0.07)", background: "#0b1524" }}>
          <table style={{ width: "100%", borderCollapse: "collapse", minWidth: 600 }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.03)" }}>
                <th style={{ ...th, textAlign: "left", paddingLeft: 16 }}>Capability</th>
                <th style={{ ...th, color: CYAN_LIGHT }}>Lens</th>
                <th style={th}>Superset</th>
                <th style={th}>Power BI</th>
                <th style={th}>Redash</th>
              </tr>
            </thead>
            <tbody>
              {ROWS.map((r) => (
                <tr key={r[0] as string}>
                  <td style={{ ...td, textAlign: "left", paddingLeft: 16, color: "#d5deea", fontSize: 13, fontWeight: 600 }}>{r[0]}</td>
                  <td style={{ ...td, background: "rgba(70,199,217,0.05)" }}><Cell v={r[1]} /></td>
                  <td style={td}><Cell v={r[2]} /></td>
                  <td style={td}><Cell v={r[3]} /></td>
                  <td style={td}><Cell v={r[4]} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </section>
  );
}

function CTA() {
  return (
    <section id="stack" style={{
      background: `radial-gradient(700px 350px at 50% 120%, #10404f, ${INK})`,
      padding: "48px 24px 40px", textAlign: "center",
    }}>
      <h2 style={{ fontSize: 32, fontWeight: 850, color: "#f4f8fc", margin: 0, letterSpacing: "-0.02em" }}>
        Bring your data into focus.
      </h2>
      <p style={{ color: "#a9bace", fontSize: 15, marginTop: 8 }}>
        Self-hosted, open source, and ready in minutes.
      </p>
      <div style={{ display: "flex", gap: 12, justifyContent: "center", marginTop: 22, flexWrap: "wrap" }}>
        <Link href="/login" style={{
          padding: "11px 28px", borderRadius: 10, fontSize: 14, fontWeight: 700, textDecoration: "none",
          background: `linear-gradient(135deg, ${CYAN_LIGHT}, ${CYAN})`, color: INK,
          boxShadow: "0 8px 24px rgba(70,199,217,0.3)",
        }}>Get started free</Link>
        <a href="https://github.com/PruthviProdduturi/Lens" target="_blank" rel="noreferrer" style={{
          padding: "11px 24px", borderRadius: 10, fontSize: 14, fontWeight: 700, textDecoration: "none",
          background: "rgba(255,255,255,0.06)", color: "#e6eef7", border: "1px solid rgba(255,255,255,0.14)",
        }}>View on GitHub</a>
      </div>
      <div style={{ marginTop: 32, color: "#63748c", fontSize: 12 }}>
        © {new Date().getFullYear()} Lens — a Kaveon platform module. See the pattern.
      </div>
    </section>
  );
}

export default function AboutPage() {
  return (
    <div style={{ background: INK, minHeight: "100vh", fontFamily: "'Inter', 'Segoe UI', system-ui, sans-serif" }}>
      <Nav />
      <Hero />
      <Features />
      <Compare />
      <CTA />
    </div>
  );
}
