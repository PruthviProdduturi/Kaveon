"use client";

import { useEffect, useRef } from "react";
import { KaveonMark } from "../../components/KaveonMark";

function useFadeIn(delay = 0) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.opacity = "0";
    el.style.transform = "translateY(40px)";
    el.style.transition = `opacity 0.8s ease-out ${delay}ms, transform 0.8s ease-out ${delay}ms`;
    const obs = new IntersectionObserver(
      ([e]) => { if (e.isIntersecting) { el.style.opacity = "1"; el.style.transform = "translateY(0)"; obs.disconnect(); } },
      { threshold: 0.1 }
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [delay]);
  return ref;
}

export default function AboutPage() {
  const r1 = useFadeIn(0);
  const r2 = useFadeIn(100);
  const r3 = useFadeIn(0);
  const r4 = useFadeIn(0);
  const r5 = useFadeIn(0);
  const r6 = useFadeIn(0);

  return (
    <div style={{ background: "#0a0a0a", color: "#f5f5f5", minHeight: "100vh", overflowX: "hidden" }}>

      {/* ─── CSS ─── */}
      <style>{`
        @keyframes float { 0%,100% { transform: translateY(0); } 50% { transform: translateY(-20px); } }
        @keyframes pulse { 0%,100% { opacity: 0.4; } 50% { opacity: 0.8; } }
        @keyframes gradientShift { 0% { background-position: 0% 50%; } 50% { background-position: 100% 50%; } 100% { background-position: 0% 50%; } }
        .about-link:hover { color: #fff !important; }
        .about-card:hover { transform: translateY(-4px); border-color: rgba(255,255,255,0.12) !important; }
        .about-btn:hover { transform: translateY(-2px); box-shadow: 0 8px 30px rgba(74,158,232,0.4) !important; }
      `}</style>

      {/* ─── Nav ─── */}
      <nav style={{
        position: "fixed", top: 0, left: 0, right: 0, zIndex: 100,
        display: "flex", alignItems: "center", justifyContent: "space-between",
        padding: "14px 48px",
        background: "rgba(10,10,10,0.7)", backdropFilter: "blur(20px) saturate(180%)",
        borderBottom: "1px solid rgba(255,255,255,0.05)",
      }}>
        <a href="/about" style={{ display: "flex", alignItems: "center", textDecoration: "none" }}>
          <svg width="120" height="20" viewBox="60 50 1180 200" fill="none" xmlns="http://www.w3.org/2000/svg" style={{ shapeRendering: "geometricPrecision" }}>
            <g fill="#e2e8f0">
              <rect x="90" y="70" width="20" height="165" />
              <polygon points="108.73,161.20 215.73,86.39 204.27,70 97.27,144.80" />
              <polygon points="97.51,161.36 209.51,235 220.49,218.29 108.49,144.64" />
              <path d="M 260 235 L 330 70 L 350 70 L 420 235 L 397 235 L 340 104 L 283 235 Z" />
              <path d="M 465 70 L 488 70 L 545 201 L 602 70 L 625 70 L 555 235 L 535 235 Z" />
              <rect x="675" y="70" width="20" height="165" />
              <rect x="675" y="70" width="130" height="20" />
              <rect x="675" y="142.5" width="108" height="20" />
              <rect x="675" y="215" width="130" height="20" />
              <rect x="1060" y="70" width="20" height="165" />
              <rect x="1195" y="70" width="20" height="165" />
              <polygon points="1062.53,83.30 1197.53,235 1212.47,221.70 1077.47,70" />
            </g>
            <path d="M 966.25 215.29 A 72.5 72.5 0 1 0 893.75 215.29" fill="none" stroke="#4A9EE8" strokeWidth="20" strokeLinecap="butt" />
          </svg>
        </a>
        <div style={{ display: "flex", alignItems: "center", gap: 32 }}>
          <a href="#features" className="about-link" style={{ fontSize: 13, color: "#888", textDecoration: "none", transition: "color 0.2s" }}>Features</a>
          <a href="/docs" target="_blank" className="about-link" style={{ fontSize: 13, color: "#888", textDecoration: "none", transition: "color 0.2s" }}>Docs</a>
          <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" className="about-link" style={{ fontSize: 13, color: "#888", textDecoration: "none", transition: "color 0.2s" }}>GitHub</a>
          <a href="/" className="about-btn" style={{ fontSize: 13, color: "#fff", textDecoration: "none", padding: "8px 22px", borderRadius: 8, background: "#4A9EE8", transition: "all 0.2s" }}>Launch App →</a>
        </div>
      </nav>

      {/* ─── Hero ─── */}
      <section style={{ position: "relative", textAlign: "center", paddingTop: 160, paddingBottom: 100, overflow: "hidden" }}>
        {/* Mesh gradient background */}
        <div style={{
          position: "absolute", inset: 0, opacity: 0.6,
          background: "radial-gradient(ellipse 80% 60% at 50% -20%, rgba(74,158,232,0.15), transparent), radial-gradient(ellipse 60% 50% at 80% 50%, rgba(139,92,246,0.08), transparent), radial-gradient(ellipse 50% 40% at 20% 80%, rgba(16,185,129,0.06), transparent)",
        }} />
        {/* Floating orbs */}
        <div style={{ position: "absolute", top: 80, left: "15%", width: 300, height: 300, borderRadius: "50%", background: "radial-gradient(circle, rgba(74,158,232,0.08) 0%, transparent 70%)", animation: "float 8s ease-in-out infinite", pointerEvents: "none" }} />
        <div style={{ position: "absolute", top: 200, right: "10%", width: 200, height: 200, borderRadius: "50%", background: "radial-gradient(circle, rgba(139,92,246,0.06) 0%, transparent 70%)", animation: "float 10s ease-in-out infinite 2s", pointerEvents: "none" }} />

        <div ref={r1} style={{ position: "relative" }}>
          <div style={{ display: "inline-flex", alignItems: "center", gap: 8, padding: "6px 16px", borderRadius: 999, background: "rgba(74,158,232,0.1)", border: "1px solid rgba(74,158,232,0.2)", marginBottom: 32, fontSize: 13, color: "#4A9EE8" }}>
            <span style={{ width: 6, height: 6, borderRadius: "50%", background: "#4A9EE8", animation: "pulse 2s ease-in-out infinite" }} />
            Open Source · MIT Licensed
          </div>
          <h1 style={{ fontSize: "clamp(40px, 6vw, 72px)", fontWeight: 700, letterSpacing: "-2px", lineHeight: 1.05, margin: "0 auto 24px", maxWidth: 800 }}>
            Your data speaks.{" "}
            <span style={{
              background: "linear-gradient(135deg, #4A9EE8, #8b5cf6, #4A9EE8)",
              backgroundSize: "200% 200%",
              animation: "gradientShift 4s ease infinite",
              WebkitBackgroundClip: "text",
              WebkitTextFillColor: "transparent",
            }}>We translate.</span>
          </h1>
          <p style={{ fontSize: 18, color: "#777", maxWidth: 540, margin: "0 auto 44px", lineHeight: 1.7 }}>
            Ask a question in plain English. Kaveon generates SQL, executes it, picks the right chart, and explains the answer. No API keys. No LLM costs.
          </p>
          <div style={{ display: "flex", gap: 14, justifyContent: "center" }}>
            <a href="/" className="about-btn" style={{ padding: "16px 40px", borderRadius: 12, background: "#4A9EE8", color: "#fff", fontSize: 16, fontWeight: 600, textDecoration: "none", boxShadow: "0 4px 24px rgba(74,158,232,0.3)", transition: "all 0.2s" }}>
              Try Kaveon
            </a>
            <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" style={{ padding: "16px 40px", borderRadius: 12, background: "rgba(255,255,255,0.05)", color: "#aaa", fontSize: 16, fontWeight: 500, textDecoration: "none", border: "1px solid rgba(255,255,255,0.1)", transition: "all 0.2s" }}>
              Star on GitHub
            </a>
          </div>
        </div>
      </section>

      {/* ─── Product Demo ─── */}
      <div ref={r2} style={{ maxWidth: 820, margin: "0 auto", padding: "0 24px 120px" }}>
        <div style={{ background: "linear-gradient(180deg, #141414, #1a1a1a)", borderRadius: 20, border: "1px solid rgba(255,255,255,0.08)", overflow: "hidden", boxShadow: "0 40px 80px rgba(0,0,0,0.6), 0 0 0 1px rgba(255,255,255,0.03)" }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "12px 20px", borderBottom: "1px solid rgba(255,255,255,0.06)", background: "rgba(255,255,255,0.02)" }}>
            <div style={{ width: 12, height: 12, borderRadius: "50%", background: "#ff5f57" }} />
            <div style={{ width: 12, height: 12, borderRadius: "50%", background: "#febc2e" }} />
            <div style={{ width: 12, height: 12, borderRadius: "50%", background: "#28c840" }} />
            <span style={{ flex: 1, textAlign: "center", fontSize: 12, color: "#555" }}>kaveon.vercel.app</span>
          </div>
          <div style={{ display: "flex" }}>
            {/* Mini sidebar */}
            <div style={{ width: 180, borderRight: "1px solid rgba(255,255,255,0.06)", padding: "16px 12px", flexShrink: 0, display: "flex", flexDirection: "column", gap: 4 }}>
              <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "6px 8px", fontSize: 12, color: "#4A9EE8", background: "rgba(74,158,232,0.08)", borderRadius: 6 }}>
                <span>+</span> New Chat
              </div>
              <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "6px 8px", fontSize: 12, color: "#777", borderRadius: 6 }}>
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>
                Workspace
              </div>
              <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "6px 8px", fontSize: 12, color: "#777", borderRadius: 6 }}>
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>
                SQL Lab
              </div>
              <div style={{ height: 1, background: "rgba(255,255,255,0.06)", margin: "8px 0" }} />
              <div style={{ fontSize: 10, color: "#444", padding: "0 8px", marginBottom: 4 }}>RECENT</div>
              <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "5px 8px", fontSize: 11, color: "#555" }}>
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
                COVID by country
              </div>
              <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "5px 8px", fontSize: 11, color: "#555" }}>
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>
                Revenue Dashboard
              </div>
            </div>
            {/* Chat area */}
            <div style={{ flex: 1, padding: "24px" }}>
              {/* User */}
              <div style={{ display: "flex", gap: 10, marginBottom: 18, justifyContent: "flex-end" }}>
                <div style={{ background: "#4A9EE8", color: "#fff", padding: "10px 16px", borderRadius: "14px 4px 14px 14px", fontSize: 14 }}>
                  Show confirmed cases by country
                </div>
                <div style={{ width: 24, height: 24, borderRadius: "50%", background: "linear-gradient(135deg, #6db3ed, #2d7dd2)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 10, fontWeight: 700, color: "#fff", flexShrink: 0 }}>P</div>
              </div>
              {/* Kaveon */}
              <div style={{ display: "flex", gap: 10, marginBottom: 20 }}>
                <KaveonMark size={20} useDirectColor />
                <div style={{ flex: 1 }}>
                  <p style={{ fontSize: 13, color: "#bbb", lineHeight: 1.7, margin: "0 0 12px" }}>
                    Found <strong style={{ color: "#fff" }}>195</strong> results. Top 3: <strong style={{ color: "#fff" }}>United States</strong> (103.8M), <strong style={{ color: "#fff" }}>India</strong> (45.0M), <strong style={{ color: "#fff" }}>France</strong> (39.9M).
                  </p>
                  <div style={{ background: "#161616", borderRadius: 10, padding: "14px 18px 10px", border: "1px solid rgba(255,255,255,0.05)" }}>
                    <div style={{ fontSize: 11, color: "#555", marginBottom: 8, fontWeight: 500 }}>Confirmed Cases by Country</div>
                    <div style={{ display: "flex", alignItems: "flex-end", gap: 4, height: 80 }}>
                      {[{h:80,c:"#4A9EE8"},{h:35,c:"#10b981"},{h:31,c:"#f59e0b"},{h:27,c:"#ef4444"},{h:25,c:"#8b5cf6"},{h:20,c:"#ec4899"},{h:19,c:"#06b6d4"},{h:16,c:"#f97316"},{h:15,c:"#6366f1"},{h:13,c:"#14b8a6"}].map((b,i) => (
                        <div key={i} style={{ flex: 1, height: b.h, background: b.c, borderRadius: "2px 2px 0 0", opacity: 0.85 }} />
                      ))}
                    </div>
                    <div style={{ display: "flex", justifyContent: "space-between", marginTop: 5, fontSize: 8, color: "#444" }}>
                      {["US","IN","FR","DE","BR","JP","KR","IT","UK","RU"].map(c => <span key={c}>{c}</span>)}
                    </div>
                  </div>
                  <div style={{ fontSize: 11, color: "#444", marginTop: 8 }}>
                    Want me to show the <span style={{ color: "#4A9EE8" }}>top 10</span> or filter by <span style={{ color: "#4A9EE8" }}>region</span>?
                  </div>
                </div>
              </div>
              {/* User 2 */}
              <div style={{ display: "flex", gap: 10, justifyContent: "flex-end", marginBottom: 18 }}>
                <div style={{ background: "#4A9EE8", color: "#fff", padding: "10px 16px", borderRadius: "14px 4px 14px 14px", fontSize: 14 }}>
                  Trend over time
                </div>
                <div style={{ width: 24, height: 24, borderRadius: "50%", background: "linear-gradient(135deg, #6db3ed, #2d7dd2)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 10, fontWeight: 700, color: "#fff", flexShrink: 0 }}>P</div>
              </div>
              {/* Kaveon 2 */}
              <div style={{ display: "flex", gap: 10 }}>
                <KaveonMark size={20} useDirectColor />
                <div style={{ flex: 1 }}>
                  <p style={{ fontSize: 13, color: "#bbb", lineHeight: 1.7, margin: "0 0 12px" }}>
                    Peak was <strong style={{ color: "#fff" }}>January 2022</strong> at <strong style={{ color: "#fff" }}>23.4M</strong> weekly cases.
                  </p>
                  <div style={{ background: "#161616", borderRadius: 10, padding: "14px 18px 10px", border: "1px solid rgba(255,255,255,0.05)" }}>
                    <div style={{ fontSize: 11, color: "#555", marginBottom: 8, fontWeight: 500 }}>Global New Cases (Weekly)</div>
                    <svg width="100%" height="60" viewBox="0 0 400 60" preserveAspectRatio="none">
                      <defs>
                        <linearGradient id="areaFill" x1="0" y1="0" x2="0" y2="1">
                          <stop offset="0%" stopColor="#4A9EE8" stopOpacity="0.25" />
                          <stop offset="100%" stopColor="#4A9EE8" stopOpacity="0" />
                        </linearGradient>
                      </defs>
                      <path d="M0,55 C30,54 60,52 90,48 C120,44 140,35 170,28 C200,22 220,30 250,12 C270,4 285,8 300,16 C330,28 360,35 400,38 L400,60 L0,60 Z" fill="url(#areaFill)" />
                      <path d="M0,55 C30,54 60,52 90,48 C120,44 140,35 170,28 C200,22 220,30 250,12 C270,4 285,8 300,16 C330,28 360,35 400,38" fill="none" stroke="#4A9EE8" strokeWidth="2" />
                      <circle cx="250" cy="12" r="3" fill="#4A9EE8" />
                    </svg>
                    <div style={{ display: "flex", justifyContent: "space-between", fontSize: 8, color: "#444", marginTop: 4 }}>
                      <span>2020</span><span>2021</span><span>2022</span><span>2023</span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* ─── How It Works ─── */}
      <section ref={r3} style={{ maxWidth: 1000, margin: "0 auto", padding: "0 24px 120px" }}>
        <h2 style={{ fontSize: 12, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#4A9EE8", textAlign: "center", marginBottom: 48 }}>
          How it works
        </h2>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 1, background: "rgba(255,255,255,0.04)", borderRadius: 16, overflow: "hidden" }}>
          {[
            { n: "01", title: "You ask", desc: "Type a question in natural language. \"Show revenue by region.\" \"Top 10 customers.\" \"Trend over time.\"", color: "#4A9EE8" },
            { n: "02", title: "We parse", desc: "A deterministic NL→SQL engine matches your words against schema metadata. No LLM. No API key. Instant.", color: "#8b5cf6" },
            { n: "03", title: "Data answers", desc: "SQL executes, the right chart type is selected, and you see data with an intelligent summary.", color: "#10b981" },
          ].map((s) => (
            <div key={s.n} style={{ textAlign: "center", padding: "48px 32px", background: "#0a0a0a" }}>
              <div style={{ fontSize: 56, fontWeight: 800, color: s.color, opacity: 0.15, lineHeight: 1, marginBottom: 16 }}>{s.n}</div>
              <h3 style={{ fontSize: 22, fontWeight: 600, marginBottom: 10 }}>{s.title}</h3>
              <p style={{ fontSize: 14, color: "#777", lineHeight: 1.8 }}>{s.desc}</p>
            </div>
          ))}
        </div>
      </section>

      {/* ─── Features (Bento Grid) ─── */}
      <section id="features" ref={r4} style={{ maxWidth: 1100, margin: "0 auto", padding: "0 24px 120px" }}>
        <h2 style={{ fontSize: 12, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#4A9EE8", textAlign: "center", marginBottom: 12 }}>
          Everything you need
        </h2>
        <p style={{ fontSize: 17, color: "#666", textAlign: "center", marginBottom: 48 }}>
          One platform. Ask questions, build charts, create dashboards, write SQL.
        </p>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gridAutoRows: "auto", gap: 12 }}>
          {/* Big card — spans 2 cols */}
          <div className="about-card" style={{ gridColumn: "span 2", padding: 36, borderRadius: 16, background: "linear-gradient(135deg, rgba(74,158,232,0.08) 0%, rgba(74,158,232,0.02) 100%)", border: "1px solid rgba(255,255,255,0.06)", transition: "all 0.3s" }}>
            <div style={{ fontSize: 12, fontWeight: 600, color: "#4A9EE8", textTransform: "uppercase", letterSpacing: "2px", marginBottom: 12 }}>Core Feature</div>
            <h3 style={{ fontSize: 26, fontWeight: 600, marginBottom: 12, lineHeight: 1.3 }}>Conversational Data Querying</h3>
            <p style={{ fontSize: 15, color: "#888", lineHeight: 1.8, maxWidth: 500 }}>
              Type questions in plain English. A template-based NL→SQL engine parses your words, matches schema metadata, generates SQL, and renders the answer as an interactive chart — all without an API key or LLM.
            </p>
          </div>
          {/* Regular card */}
          <div className="about-card" style={{ padding: 32, borderRadius: 16, background: "linear-gradient(135deg, rgba(16,185,129,0.08) 0%, rgba(16,185,129,0.02) 100%)", border: "1px solid rgba(255,255,255,0.06)", transition: "all 0.3s" }}>
            <h3 style={{ fontSize: 20, fontWeight: 600, marginBottom: 10 }}>37 Chart Types</h3>
            <p style={{ fontSize: 14, color: "#888", lineHeight: 1.8 }}>Bar, line, pie, heatmap, treemap, scatter, funnel, gauge, waterfall, 3D globe. All interactive, all dark-mode aware.</p>
          </div>
          {/* Regular */}
          <div className="about-card" style={{ padding: 32, borderRadius: 16, background: "linear-gradient(135deg, rgba(139,92,246,0.08) 0%, rgba(139,92,246,0.02) 100%)", border: "1px solid rgba(255,255,255,0.06)", transition: "all 0.3s" }}>
            <h3 style={{ fontSize: 20, fontWeight: 600, marginBottom: 10 }}>SQL Lab</h3>
            <p style={{ fontSize: 14, color: "#888", lineHeight: 1.8 }}>Monaco editor with autocomplete, multi-tab, query history, and caching. VS Code in your browser.</p>
          </div>
          {/* Big card — spans 2 cols */}
          <div className="about-card" style={{ gridColumn: "span 2", padding: 36, borderRadius: 16, background: "linear-gradient(135deg, rgba(245,158,11,0.08) 0%, rgba(245,158,11,0.02) 100%)", border: "1px solid rgba(255,255,255,0.06)", transition: "all 0.3s" }}>
            <div style={{ fontSize: 12, fontWeight: 600, color: "#f59e0b", textTransform: "uppercase", letterSpacing: "2px", marginBottom: 12 }}>Build & Share</div>
            <h3 style={{ fontSize: 26, fontWeight: 600, marginBottom: 12, lineHeight: 1.3 }}>Dashboards That Tell Stories</h3>
            <p style={{ fontSize: 15, color: "#888", lineHeight: 1.8, maxWidth: 500 }}>
              Drag-and-drop canvas with cross-chart filtering, shared filter bar, auto-refresh, and one-click publishing. Every chart renders in parallel for instant load.
            </p>
          </div>
          {/* Small cards */}
          <div className="about-card" style={{ padding: 32, borderRadius: 16, background: "linear-gradient(135deg, rgba(236,72,153,0.08) 0%, rgba(236,72,153,0.02) 100%)", border: "1px solid rgba(255,255,255,0.06)", transition: "all 0.3s" }}>
            <h3 style={{ fontSize: 20, fontWeight: 600, marginBottom: 10 }}>Multi-Source</h3>
            <p style={{ fontSize: 14, color: "#888", lineHeight: 1.8 }}>Fabric, Azure SQL, PostgreSQL, MySQL, StarRocks. Connect them all.</p>
          </div>
          <div className="about-card" style={{ padding: 32, borderRadius: 16, background: "linear-gradient(135deg, rgba(6,182,212,0.08) 0%, rgba(6,182,212,0.02) 100%)", border: "1px solid rgba(255,255,255,0.06)", transition: "all 0.3s" }}>
            <h3 style={{ fontSize: 20, fontWeight: 600, marginBottom: 10 }}>Semantic Datasets</h3>
            <p style={{ fontSize: 14, color: "#888", lineHeight: 1.8 }}>Define dimensions, metrics, and joins once. Reuse across unlimited charts.</p>
          </div>
          <div className="about-card" style={{ padding: 32, borderRadius: 16, background: "linear-gradient(135deg, rgba(99,102,241,0.08) 0%, rgba(99,102,241,0.02) 100%)", border: "1px solid rgba(255,255,255,0.06)", transition: "all 0.3s" }}>
            <h3 style={{ fontSize: 20, fontWeight: 600, marginBottom: 10 }}>Self-Hosted</h3>
            <p style={{ fontSize: 14, color: "#888", lineHeight: 1.8 }}>Your infrastructure, your data, your rules. OAuth, RBAC, encrypted secrets. MIT licensed.</p>
          </div>
        </div>
      </section>

      {/* ─── Tech ─── */}
      <section ref={r5} style={{ textAlign: "center", padding: "0 24px 120px" }}>
        <h2 style={{ fontSize: 12, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#4A9EE8", marginBottom: 32 }}>
          Built with
        </h2>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 10, justifyContent: "center", maxWidth: 650, margin: "0 auto" }}>
          {["Next.js 15", "React 19", "TypeScript", "FastAPI", "Python 3.11", "ECharts", "PostgreSQL", "Azure", "Vercel", "Bicep IaC"].map((t) => (
            <span key={t} style={{ padding: "10px 22px", borderRadius: 999, border: "1px solid rgba(255,255,255,0.06)", fontSize: 14, color: "#777", background: "rgba(255,255,255,0.02)" }}>{t}</span>
          ))}
        </div>
      </section>

      {/* ─── CTA ─── */}
      <section ref={r6} style={{ textAlign: "center", padding: "80px 24px 100px", position: "relative" }}>
        <div style={{ position: "absolute", bottom: -100, left: "50%", transform: "translateX(-50%)", width: 800, height: 400, borderRadius: "50%", background: "radial-gradient(circle, rgba(74,158,232,0.1) 0%, transparent 70%)", pointerEvents: "none" }} />
        <div style={{ position: "relative" }}>
          <KaveonMark size={44} useDirectColor />
          <h2 style={{ fontSize: "clamp(28px, 4vw, 42px)", fontWeight: 600, letterSpacing: "-1px", margin: "20px 0 12px" }}>
            Ready to talk to your data?
          </h2>
          <p style={{ fontSize: 15, color: "#555", marginBottom: 36 }}>Open source · Self-hosted · MIT License</p>
          <a href="/" className="about-btn" style={{ display: "inline-block", padding: "16px 44px", borderRadius: 12, background: "#4A9EE8", color: "#fff", fontSize: 16, fontWeight: 600, textDecoration: "none", boxShadow: "0 4px 24px rgba(74,158,232,0.3)", transition: "all 0.2s" }}>
            Get Started
          </a>
        </div>
      </section>

      {/* ─── Footer ─── */}
      <footer style={{ borderTop: "1px solid rgba(255,255,255,0.05)", padding: "28px 48px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <KaveonMark size={16} useDirectColor />
          <span style={{ fontSize: 12, color: "#444" }}>© {new Date().getFullYear()} Kaveon</span>
        </div>
        <div style={{ display: "flex", gap: 24 }}>
          <a href="/docs" target="_blank" className="about-link" style={{ fontSize: 12, color: "#444", textDecoration: "none", transition: "color 0.2s" }}>Documentation</a>
          <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" className="about-link" style={{ fontSize: 12, color: "#444", textDecoration: "none", transition: "color 0.2s" }}>GitHub</a>
          <a href="/" className="about-link" style={{ fontSize: 12, color: "#444", textDecoration: "none", transition: "color 0.2s" }}>Launch App</a>
        </div>
      </footer>
    </div>
  );
}
