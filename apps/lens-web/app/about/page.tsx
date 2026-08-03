"use client";

import React from "react";
import Link from "next/link";
import { LensLogo, LensMark } from "../../components/LensLogo";

// ── Lens landing / about — bold dark hero + product shot ─────────────────────
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
      padding: "14px 28px", background: "rgba(10,16,30,0.72)",
      backdropFilter: "blur(12px)", borderBottom: "1px solid rgba(255,255,255,0.08)",
    }}>
      <LensLogo size={26} />
      <div style={{ display: "flex", alignItems: "center", gap: 28 }}>
        <a href="#features" style={{ color: "#c7d2e0", textDecoration: "none", fontSize: 14, fontWeight: 500 }}>Features</a>
        <a href="#compare" style={{ color: "#c7d2e0", textDecoration: "none", fontSize: 14, fontWeight: 500 }}>Compare</a>
        <a href="#stack" style={{ color: "#c7d2e0", textDecoration: "none", fontSize: 14, fontWeight: 500 }}>Platform</a>
        <Link href="/" style={{
          padding: "8px 18px", borderRadius: 9,
          background: `linear-gradient(135deg, ${CYAN_LIGHT}, ${CYAN})`,
          color: INK, fontWeight: 700, fontSize: 14, textDecoration: "none",
        }}>Sign in</Link>
      </div>
    </nav>
  );
}

// A self-contained CSS/SVG "product shot" — no screenshot needed.
function ProductShot() {
  const line = "M0,78 L40,70 L80,74 L120,52 L160,60 L200,34 L240,44 L280,18 L320,26 L360,8";
  const area = line + " L360,96 L0,96 Z";
  const bars = [42, 68, 55, 83, 60, 92, 48];
  return (
    <div style={{
      borderRadius: 16, background: "linear-gradient(180deg, #101d33, #0b1526)",
      border: "1px solid rgba(124,224,239,0.22)",
      boxShadow: "0 40px 120px rgba(0,0,0,0.6), 0 0 60px rgba(70,199,217,0.12)",
      padding: 18, display: "grid", gap: 14,
      gridTemplateColumns: "1fr 1fr",
    }}>
      <div style={{ gridColumn: "1 / -1", display: "flex", gap: 14 }}>
        {[["676.6M", "Total Cases"], ["6.9M", "Total Deaths"], ["201", "Countries"]].map(([v, l]) => (
          <div key={l} style={{
            flex: 1, background: "rgba(255,255,255,0.03)", border: "1px solid rgba(255,255,255,0.07)",
            borderRadius: 12, padding: "14px 16px",
          }}>
            <div style={{ fontSize: 22, fontWeight: 800, color: CYAN_LIGHT }}>{v}</div>
            <div style={{ fontSize: 11.5, color: "#8496ad", marginTop: 2 }}>{l}</div>
          </div>
        ))}
      </div>
      <div style={{ background: "rgba(255,255,255,0.03)", border: "1px solid rgba(255,255,255,0.07)", borderRadius: 12, padding: 14 }}>
        <div style={{ fontSize: 12, color: "#8496ad", marginBottom: 8 }}>New cases over time</div>
        <svg viewBox="0 0 360 96" width="100%" height="120" preserveAspectRatio="none">
          <defs>
            <linearGradient id="ar" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={CYAN} stopOpacity="0.35" />
              <stop offset="100%" stopColor={CYAN} stopOpacity="0" />
            </linearGradient>
          </defs>
          <path d={area} fill="url(#ar)" />
          <path d={line} fill="none" stroke={CYAN_LIGHT} strokeWidth="2.5" />
        </svg>
      </div>
      <div style={{ background: "rgba(255,255,255,0.03)", border: "1px solid rgba(255,255,255,0.07)", borderRadius: 12, padding: 14 }}>
        <div style={{ fontSize: 12, color: "#8496ad", marginBottom: 8 }}>Top countries</div>
        <div style={{ display: "flex", alignItems: "flex-end", gap: 8, height: 120 }}>
          {bars.map((h, i) => (
            <div key={i} style={{
              flex: 1, height: `${h}%`, borderRadius: "5px 5px 0 0",
              background: `linear-gradient(180deg, ${CYAN_LIGHT}, ${CYAN})`, opacity: 0.55 + i * 0.06,
            }} />
          ))}
        </div>
      </div>
    </div>
  );
}

function Hero() {
  return (
    <header style={{
      position: "relative", overflow: "hidden",
      background: `radial-gradient(1100px 520px at 50% -5%, #103240 0%, ${INK} 55%)`,
      padding: "72px 24px 88px",
    }}>
      <div style={{ maxWidth: 1080, margin: "0 auto", textAlign: "center" }}>
        <style>{`
          .hero-logo { transition: transform 0.9s cubic-bezier(.22,1,.36,1); cursor: pointer; }
          .hero-logo:hover { transform: rotate(360deg); }
        `}</style>
        <div style={{ display: "flex", justifyContent: "center", marginBottom: 22 }}>
          <div className="hero-logo" style={{
            display: "inline-flex", padding: 14, borderRadius: "50%",
            background: "rgba(124,224,239,0.06)", border: "1px solid rgba(124,224,239,0.18)",
            boxShadow: "0 0 50px rgba(70,199,217,0.25)",
          }}>
            <LensMark size={70} />
          </div>
        </div>
        <div style={{
          display: "inline-flex", alignItems: "center", gap: 8, padding: "6px 14px",
          borderRadius: 999, border: "1px solid rgba(124,224,239,0.3)",
          background: "rgba(124,224,239,0.08)", color: CYAN_LIGHT, fontSize: 12.5, fontWeight: 600, marginBottom: 26,
        }}>
          ✦ AI-native · Self-hosted · Open source
        </div>
        <h1 style={{
          fontSize: "clamp(40px, 7vw, 76px)", fontWeight: 850, lineHeight: 1.02, letterSpacing: "-0.02em",
          margin: 0, color: "#f4f8fc",
        }}>
          See the pattern.
        </h1>
        <p style={{
          fontSize: "clamp(16px, 2.4vw, 21px)", color: "#a9bace", maxWidth: 640,
          margin: "20px auto 0", lineHeight: 1.55,
        }}>
          The analytics platform for <strong style={{ color: "#e6eef7" }}>Microsoft Fabric</strong> and{" "}
          <strong style={{ color: "#e6eef7" }}>Trino</strong> — and every database.
          Query live data, build 20+ chart types, assemble dashboards, and ask questions in plain English.
        </p>
        <div style={{ display: "flex", gap: 14, justifyContent: "center", marginTop: 34, flexWrap: "wrap" }}>
          <Link href="/" style={{
            padding: "14px 30px", borderRadius: 11, fontSize: 15.5, fontWeight: 700, textDecoration: "none",
            background: `linear-gradient(135deg, ${CYAN_LIGHT}, ${CYAN})`, color: INK,
            boxShadow: "0 10px 30px rgba(70,199,217,0.35)",
          }}>Get started</Link>
          <Link href="/dashboards" style={{
            padding: "14px 28px", borderRadius: 11, fontSize: 15.5, fontWeight: 700, textDecoration: "none",
            background: "rgba(255,255,255,0.06)", color: "#e6eef7", border: "1px solid rgba(255,255,255,0.14)",
          }}>Live demo ▸</Link>
        </div>
        <div style={{ marginTop: 56, maxWidth: 900, marginInline: "auto" }}>
          <ProductShot />
        </div>
        <div style={{ marginTop: 30, fontSize: 13, color: "#7688a0", letterSpacing: "0.04em" }}>
          SQL LAB · 20+ CHARTS · AI NL→SQL · DASHBOARDS · MULTI-DATABASE
        </div>
      </div>
    </header>
  );
}

const FEATURES = [
  { icon: "fa-flask", title: "SQL Lab", body: "VS Code-grade Monaco editor, multi-tab sessions, query history, async execution, and result caching." },
  { icon: "fa-wand-magic-sparkles", title: "AI, built in", body: "Natural-language → SQL with live schema context. Anthropic, OpenAI, or GitHub Models — your keys." },
  { icon: "fa-chart-column", title: "20+ chart types", body: "Lines, bars, pies, heatmaps, treemaps, a 3D WebGL globe — with cross-filtering across the dashboard." },
  { icon: "fa-table-columns", title: "Dashboards", body: "Drag-and-drop canvas, rows, tabs, text, and shared filters that slice every tile at once." },
  { icon: "fa-database", title: "Multi-source", body: "Microsoft Fabric SQL, Azure SQL, PostgreSQL, MySQL — Trino & StarRocks on the way." },
  { icon: "fa-shield-halved", title: "Enterprise auth", body: "GitHub, Google, and Microsoft sign-in with 4-tier RBAC and per-object visibility." },
];

function Features() {
  return (
    <section id="features" style={{ background: INK, padding: "88px 24px" }}>
      <div style={{ maxWidth: 1120, margin: "0 auto" }}>
        <h2 style={{ textAlign: "center", fontSize: 34, fontWeight: 800, color: "#f4f8fc", margin: 0 }}>
          Everything to explore, visualise, and share
        </h2>
        <p style={{ textAlign: "center", color: "#8fa2ba", fontSize: 16, marginTop: 12 }}>
          A modern command centre for your data — not a dashboard bolted onto a warehouse.
        </p>
        <div style={{
          display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))",
          gap: 18, marginTop: 44,
        }}>
          {FEATURES.map((f) => (
            <div key={f.title} style={{
              background: "linear-gradient(180deg, #101d33, #0c1524)",
              border: "1px solid rgba(255,255,255,0.08)", borderRadius: 16, padding: "26px 24px",
            }}>
              <div style={{
                width: 44, height: 44, borderRadius: 11, display: "flex", alignItems: "center", justifyContent: "center",
                background: "rgba(70,199,217,0.14)", border: "1px solid rgba(70,199,217,0.3)", marginBottom: 16,
              }}>
                <i className={`fas ${f.icon}`} style={{ color: CYAN_LIGHT, fontSize: 18 }} />
              </div>
              <div style={{ fontSize: 18, fontWeight: 700, color: "#eaf1f8" }}>{f.title}</div>
              <div style={{ fontSize: 14.5, color: "#93a5bd", lineHeight: 1.6, marginTop: 8 }}>{f.body}</div>
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
  const th: React.CSSProperties = { padding: "14px 12px", fontSize: 13.5, color: "#aebfd4", fontWeight: 700, textAlign: "center" };
  const td: React.CSSProperties = { padding: "13px 12px", textAlign: "center", borderTop: "1px solid rgba(255,255,255,0.06)" };
  return (
    <section id="compare" style={{ background: INK2, padding: "88px 24px" }}>
      <div style={{ maxWidth: 900, margin: "0 auto" }}>
        <h2 style={{ textAlign: "center", fontSize: 34, fontWeight: 800, color: "#f4f8fc", margin: 0 }}>
          How Lens compares
        </h2>
        <p style={{ textAlign: "center", color: "#8fa2ba", fontSize: 16, marginTop: 12, marginBottom: 36 }}>
          An honest look next to Superset, Power BI, and Redash.
        </p>
        <div style={{ overflowX: "auto", borderRadius: 16, border: "1px solid rgba(255,255,255,0.08)", background: "#0b1524" }}>
          <table style={{ width: "100%", borderCollapse: "collapse", minWidth: 640 }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.03)" }}>
                <th style={{ ...th, textAlign: "left", paddingLeft: 20 }}>Capability</th>
                <th style={{ ...th, color: CYAN_LIGHT }}>Lens</th>
                <th style={th}>Superset</th>
                <th style={th}>Power BI</th>
                <th style={th}>Redash</th>
              </tr>
            </thead>
            <tbody>
              {ROWS.map((r) => (
                <tr key={r[0] as string}>
                  <td style={{ ...td, textAlign: "left", paddingLeft: 20, color: "#d5deea", fontSize: 14.5, fontWeight: 600 }}>{r[0]}</td>
                  <td style={{ ...td, background: "rgba(70,199,217,0.06)" }}><Cell v={r[1]} /></td>
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
      background: `radial-gradient(800px 400px at 50% 120%, #10404f, ${INK})`,
      padding: "96px 24px", textAlign: "center",
    }}>
      <div style={{ display: "flex", justifyContent: "center", marginBottom: 22 }}>
        <LensLogo size={40} />
      </div>
      <h2 style={{ fontSize: 40, fontWeight: 850, color: "#f4f8fc", margin: 0, letterSpacing: "-0.02em" }}>
        Bring your data into focus.
      </h2>
      <p style={{ color: "#a9bace", fontSize: 17, marginTop: 14 }}>
        Self-hosted, open source, and ready in minutes.
      </p>
      <div style={{ display: "flex", gap: 14, justifyContent: "center", marginTop: 30, flexWrap: "wrap" }}>
        <Link href="/" style={{
          padding: "14px 32px", borderRadius: 11, fontSize: 15.5, fontWeight: 700, textDecoration: "none",
          background: `linear-gradient(135deg, ${CYAN_LIGHT}, ${CYAN})`, color: INK,
          boxShadow: "0 10px 30px rgba(70,199,217,0.35)",
        }}>Get started free</Link>
        <a href="https://github.com/PruthviProdduturi/Lens" target="_blank" rel="noreferrer" style={{
          padding: "14px 28px", borderRadius: 11, fontSize: 15.5, fontWeight: 700, textDecoration: "none",
          background: "rgba(255,255,255,0.06)", color: "#e6eef7", border: "1px solid rgba(255,255,255,0.14)",
        }}>View on GitHub</a>
      </div>
      <div style={{ marginTop: 60, color: "#63748c", fontSize: 13 }}>
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
