"use client";

import { useEffect, useRef } from "react";
import { KaveonMark } from "../../components/KaveonMark";

/* ─── Fade-in on scroll observer ─── */
function useFadeIn() {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.opacity = "0";
    el.style.transform = "translateY(32px)";
    el.style.transition = "opacity 0.7s ease-out, transform 0.7s ease-out";
    const obs = new IntersectionObserver(
      ([entry]) => { if (entry.isIntersecting) { el.style.opacity = "1"; el.style.transform = "translateY(0)"; obs.disconnect(); } },
      { threshold: 0.15 }
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, []);
  return ref;
}

function Section({ children, style }: { children: React.ReactNode; style?: React.CSSProperties }) {
  const ref = useFadeIn();
  return <div ref={ref} style={style}>{children}</div>;
}

export default function AboutPage() {
  return (
    <div style={{ background: "#0f0f0f", color: "#ececec", minHeight: "100vh", overflowX: "hidden" }}>

      {/* ─── Navigation ─── */}
      <nav style={{
        position: "fixed", top: 0, left: 0, right: 0, zIndex: 100,
        display: "flex", alignItems: "center", justifyContent: "space-between",
        padding: "16px 40px",
        background: "rgba(15,15,15,0.8)", backdropFilter: "blur(12px)",
        borderBottom: "1px solid rgba(255,255,255,0.06)",
      }}>
        <a href="/about" style={{ display: "flex", alignItems: "center", gap: 10, textDecoration: "none", color: "#ececec" }}>
          <KaveonMark size={24} useDirectColor />
          <span style={{ fontSize: 16, fontWeight: 600, letterSpacing: "1.5px", textTransform: "uppercase" }}>KAVEON</span>
        </a>
        <div style={{ display: "flex", alignItems: "center", gap: 24 }}>
          <a href="/docs" target="_blank" style={{ fontSize: 14, color: "#a0a0a0", textDecoration: "none" }}>Docs</a>
          <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" style={{ fontSize: 14, color: "#a0a0a0", textDecoration: "none" }}>GitHub</a>
          <a href="/" style={{ fontSize: 14, color: "#fff", textDecoration: "none", padding: "8px 20px", borderRadius: 8, background: "#4A9EE8" }}>Launch App</a>
        </div>
      </nav>

      {/* ─── Hero ─── */}
      <section style={{ position: "relative", textAlign: "center", paddingTop: 120, paddingBottom: 80, overflow: "hidden" }}>
        {/* Gradient orbs */}
        <div style={{ position: "absolute", top: -200, left: "20%", width: 600, height: 600, borderRadius: "50%", background: "radial-gradient(circle, rgba(74,158,232,0.12) 0%, transparent 70%)", pointerEvents: "none" }} />
        <div style={{ position: "absolute", top: -100, right: "10%", width: 400, height: 400, borderRadius: "50%", background: "radial-gradient(circle, rgba(139,92,246,0.08) 0%, transparent 70%)", pointerEvents: "none" }} />

        <div style={{ position: "relative" }}>
          <div style={{ marginBottom: 32 }}>
            <KaveonMark size={56} useDirectColor />
          </div>
          <h1 style={{ fontSize: 64, fontWeight: 600, letterSpacing: "-2px", lineHeight: 1.1, margin: "0 auto 20px", maxWidth: 700 }}>
            Your data speaks.<br />
            <span style={{ color: "#4A9EE8" }}>We translate.</span>
          </h1>
          <p style={{ fontSize: 20, color: "#888", maxWidth: 520, margin: "0 auto 48px", lineHeight: 1.6 }}>
            Ask a question in plain English. Kaveon generates SQL, executes it, picks the right chart, and explains the answer. No API keys. No LLM costs.
          </p>
          <div style={{ display: "flex", gap: 12, justifyContent: "center" }}>
            <a href="/" style={{ padding: "16px 36px", borderRadius: 12, background: "#4A9EE8", color: "#fff", fontSize: 16, fontWeight: 500, textDecoration: "none", boxShadow: "0 4px 20px rgba(74,158,232,0.3)" }}>
              Try Kaveon Free
            </a>
            <a href="/docs" target="_blank" style={{ padding: "16px 36px", borderRadius: 12, background: "rgba(255,255,255,0.06)", color: "#ccc", fontSize: 16, fontWeight: 500, textDecoration: "none", border: "1px solid rgba(255,255,255,0.1)" }}>
              Read the Docs
            </a>
          </div>
        </div>
      </section>

      {/* ─── Demo ─── */}
      <Section style={{ maxWidth: 800, margin: "0 auto", padding: "0 24px 80px" }}>
        <div style={{ background: "#1a1a1a", borderRadius: 20, border: "1px solid rgba(255,255,255,0.08)", overflow: "hidden", boxShadow: "0 20px 60px rgba(0,0,0,0.5)" }}>
          {/* Title bar */}
          <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "14px 20px", borderBottom: "1px solid rgba(255,255,255,0.06)" }}>
            <div style={{ width: 12, height: 12, borderRadius: "50%", background: "#ff5f57" }} />
            <div style={{ width: 12, height: 12, borderRadius: "50%", background: "#febc2e" }} />
            <div style={{ width: 12, height: 12, borderRadius: "50%", background: "#28c840" }} />
            <span style={{ flex: 1, textAlign: "center", fontSize: 12, color: "#666" }}>kaveon.vercel.app</span>
          </div>
          {/* Conversation */}
          <div style={{ padding: "28px" }}>
            {/* User message 1 */}
            <div style={{ display: "flex", gap: 10, marginBottom: 20, justifyContent: "flex-end" }}>
              <div style={{ background: "#4A9EE8", color: "#fff", padding: "10px 16px", borderRadius: "14px 4px 14px 14px", fontSize: 14 }}>
                Show confirmed cases by country
              </div>
              <div style={{ width: 26, height: 26, borderRadius: "50%", background: "linear-gradient(135deg, #6db3ed, #2d7dd2)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 11, fontWeight: 700, color: "#fff", flexShrink: 0 }}>P</div>
            </div>
            {/* Kaveon response 1 */}
            <div style={{ display: "flex", gap: 10, marginBottom: 24 }}>
              <div style={{ width: 26, height: 26, flexShrink: 0, display: "flex", alignItems: "center", justifyContent: "center" }}>
                <KaveonMark size={20} useDirectColor />
              </div>
              <div style={{ flex: 1 }}>
                <p style={{ fontSize: 13.5, color: "#bbb", lineHeight: 1.7, margin: "0 0 14px" }}>
                  Found <strong style={{ color: "#fff" }}>195</strong> results for confirmed by country. Top 3: <strong style={{ color: "#fff" }}>United States</strong> (103.8M), <strong style={{ color: "#fff" }}>India</strong> (45.0M), <strong style={{ color: "#fff" }}>France</strong> (39.9M).
                </p>
                {/* Chart */}
                <div style={{ background: "#1e1e1e", borderRadius: 12, padding: "16px 20px 12px", border: "1px solid rgba(255,255,255,0.06)" }}>
                  <div style={{ fontSize: 12, color: "#666", marginBottom: 10, fontWeight: 500 }}>Confirmed Cases by Country</div>
                  <div style={{ display: "flex", alignItems: "flex-end", gap: 5, height: 100 }}>
                    {[
                      { h: 100, c: "#4A9EE8" }, { h: 43, c: "#10b981" }, { h: 38, c: "#f59e0b" },
                      { h: 33, c: "#ef4444" }, { h: 31, c: "#8b5cf6" }, { h: 24, c: "#ec4899" },
                      { h: 23, c: "#06b6d4" }, { h: 20, c: "#f97316" }, { h: 18, c: "#6366f1" }, { h: 16, c: "#14b8a6" },
                    ].map((b, i) => (
                      <div key={i} style={{ flex: 1, height: `${b.h}px`, background: b.c, borderRadius: "3px 3px 0 0", opacity: 0.85 }} />
                    ))}
                  </div>
                  <div style={{ display: "flex", justifyContent: "space-between", marginTop: 6, fontSize: 9, color: "#555" }}>
                    <span>US</span><span>IN</span><span>FR</span><span>DE</span><span>BR</span><span>JP</span><span>KR</span><span>IT</span><span>UK</span><span>RU</span>
                  </div>
                </div>
                <div style={{ fontSize: 12, color: "#444", marginTop: 10 }}>
                  Want me to show just the <span style={{ color: "#4A9EE8" }}>top 10</span> or filter by <span style={{ color: "#4A9EE8" }}>region</span>?
                </div>
              </div>
            </div>
            {/* User message 2 */}
            <div style={{ display: "flex", gap: 10, marginBottom: 20, justifyContent: "flex-end" }}>
              <div style={{ background: "#4A9EE8", color: "#fff", padding: "10px 16px", borderRadius: "14px 4px 14px 14px", fontSize: 14 }}>
                Trend of new cases over time
              </div>
              <div style={{ width: 26, height: 26, borderRadius: "50%", background: "linear-gradient(135deg, #6db3ed, #2d7dd2)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 11, fontWeight: 700, color: "#fff", flexShrink: 0 }}>P</div>
            </div>
            {/* Kaveon response 2 */}
            <div style={{ display: "flex", gap: 10 }}>
              <div style={{ width: 26, height: 26, flexShrink: 0, display: "flex", alignItems: "center", justifyContent: "center" }}>
                <KaveonMark size={20} useDirectColor />
              </div>
              <div style={{ flex: 1 }}>
                <p style={{ fontSize: 13.5, color: "#bbb", lineHeight: 1.7, margin: "0 0 14px" }}>
                  Here&rsquo;s the global trend. Peak was in <strong style={{ color: "#fff" }}>January 2022</strong> at <strong style={{ color: "#fff" }}>23.4M</strong> weekly cases.
                </p>
                {/* Line chart */}
                <div style={{ background: "#1e1e1e", borderRadius: 12, padding: "16px 20px 12px", border: "1px solid rgba(255,255,255,0.06)" }}>
                  <div style={{ fontSize: 12, color: "#666", marginBottom: 10, fontWeight: 500 }}>Global New Cases (Weekly)</div>
                  <svg width="100%" height="80" viewBox="0 0 400 80" preserveAspectRatio="none">
                    <defs>
                      <linearGradient id="lineGrad" x1="0" y1="0" x2="0" y2="1">
                        <stop offset="0%" stopColor="#4A9EE8" stopOpacity="0.3" />
                        <stop offset="100%" stopColor="#4A9EE8" stopOpacity="0" />
                      </linearGradient>
                    </defs>
                    <path d="M0,75 C20,74 40,72 60,70 C80,68 100,65 120,55 C140,45 160,30 180,35 C200,40 220,50 240,20 C260,5 270,8 280,15 C300,25 320,35 340,40 C360,42 380,45 400,48" fill="url(#lineGrad)" stroke="none" />
                    <path d="M0,75 C20,74 40,72 60,70 C80,68 100,65 120,55 C140,45 160,30 180,35 C200,40 220,50 240,20 C260,5 270,8 280,15 C300,25 320,35 340,40 C360,42 380,45 400,48" fill="none" stroke="#4A9EE8" strokeWidth="2" />
                  </svg>
                  <div style={{ display: "flex", justifyContent: "space-between", fontSize: 9, color: "#555", marginTop: 4 }}>
                    <span>2020</span><span>2021</span><span>2022</span><span>2023</span>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </Section>

      {/* ─── How It Works ─── */}
      <Section style={{ maxWidth: 1000, margin: "0 auto", padding: "0 24px 80px" }}>
        <h2 style={{ fontSize: 13, fontWeight: 600, textTransform: "uppercase", letterSpacing: "3px", color: "#4A9EE8", textAlign: "center", marginBottom: 32 }}>
          How it works
        </h2>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 32 }}>
          {[
            { n: "01", title: "You ask", desc: "Type a question in natural language. \"Show revenue by region.\" \"Top 10 customers.\" \"Trend over time.\"" },
            { n: "02", title: "We parse", desc: "A deterministic NL→SQL engine matches your words against schema metadata. No LLM. No API key. Instant." },
            { n: "03", title: "Data answers", desc: "SQL executes, the right chart type is selected automatically, and you see your answer with an intelligent summary." },
          ].map((s) => (
            <div key={s.n} style={{ textAlign: "center" }}>
              <div style={{ fontSize: 48, fontWeight: 700, color: "rgba(74,158,232,0.2)", lineHeight: 1, marginBottom: 16 }}>{s.n}</div>
              <h3 style={{ fontSize: 22, fontWeight: 600, marginBottom: 10 }}>{s.title}</h3>
              <p style={{ fontSize: 15, color: "#888", lineHeight: 1.7 }}>{s.desc}</p>
            </div>
          ))}
        </div>
      </Section>

      {/* ─── Features ─── */}
      <Section style={{ maxWidth: 1100, margin: "0 auto", padding: "0 24px 80px" }}>
        <h2 style={{ fontSize: 13, fontWeight: 600, textTransform: "uppercase", letterSpacing: "3px", color: "#4A9EE8", textAlign: "center", marginBottom: 16 }}>
          Everything you need
        </h2>
        <p style={{ fontSize: 18, color: "#888", textAlign: "center", marginBottom: 32, maxWidth: 500, marginLeft: "auto", marginRight: "auto" }}>
          One platform. Ask questions, build charts, create dashboards, write SQL.
        </p>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 16 }}>
          {[
            { title: "Conversational Querying", desc: "Type questions, get charts. A template-based NL→SQL engine that works without an API key.", gradient: "linear-gradient(135deg, rgba(74,158,232,0.1) 0%, rgba(74,158,232,0.02) 100%)" },
            { title: "37 Chart Types", desc: "Bar, line, pie, heatmap, treemap, scatter, funnel, gauge, waterfall, 3D globe. All interactive.", gradient: "linear-gradient(135deg, rgba(16,185,129,0.1) 0%, rgba(16,185,129,0.02) 100%)" },
            { title: "SQL Lab", desc: "Monaco editor with autocomplete, multi-tab, query history, caching. VS Code in your browser.", gradient: "linear-gradient(135deg, rgba(139,92,246,0.1) 0%, rgba(139,92,246,0.02) 100%)" },
            { title: "Dashboards", desc: "Drag-and-drop canvas, cross-chart filtering, shared filters, auto-refresh, one-click publishing.", gradient: "linear-gradient(135deg, rgba(245,158,11,0.1) 0%, rgba(245,158,11,0.02) 100%)" },
            { title: "Multi-Source", desc: "Microsoft Fabric, Azure SQL, PostgreSQL, MySQL, StarRocks. Connect them all, query across them.", gradient: "linear-gradient(135deg, rgba(236,72,153,0.1) 0%, rgba(236,72,153,0.02) 100%)" },
            { title: "Self-Hosted", desc: "Your infrastructure, your data, your rules. OAuth sign-in, RBAC, encrypted secrets. MIT licensed.", gradient: "linear-gradient(135deg, rgba(6,182,212,0.1) 0%, rgba(6,182,212,0.02) 100%)" },
          ].map((f) => (
            <div key={f.title} style={{ padding: 32, borderRadius: 16, background: f.gradient, border: "1px solid rgba(255,255,255,0.06)" }}>
              <h3 style={{ fontSize: 18, fontWeight: 600, marginBottom: 10 }}>{f.title}</h3>
              <p style={{ fontSize: 14, color: "#999", lineHeight: 1.7, margin: 0 }}>{f.desc}</p>
            </div>
          ))}
        </div>
      </Section>

      {/* ─── Tech ─── */}
      <Section style={{ textAlign: "center", padding: "0 24px 80px" }}>
        <h2 style={{ fontSize: 13, fontWeight: 600, textTransform: "uppercase", letterSpacing: "3px", color: "#4A9EE8", marginBottom: 32 }}>
          Built with
        </h2>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 10, justifyContent: "center", maxWidth: 600, margin: "0 auto" }}>
          {["Next.js 15", "React 19", "TypeScript", "FastAPI", "Python", "ECharts", "PostgreSQL", "Azure", "Vercel"].map((t) => (
            <span key={t} style={{ padding: "10px 20px", borderRadius: 999, border: "1px solid rgba(255,255,255,0.08)", fontSize: 14, color: "#999", background: "rgba(255,255,255,0.03)" }}>{t}</span>
          ))}
        </div>
      </Section>

      {/* ─── CTA ─── */}
      <section style={{ textAlign: "center", padding: "60px 24px 80px", position: "relative" }}>
        <div style={{ position: "absolute", bottom: 0, left: "50%", transform: "translateX(-50%)", width: 800, height: 400, borderRadius: "50%", background: "radial-gradient(circle, rgba(74,158,232,0.08) 0%, transparent 70%)", pointerEvents: "none" }} />
        <div style={{ position: "relative" }}>
          <KaveonMark size={48} useDirectColor />
          <h2 style={{ fontSize: 40, fontWeight: 600, letterSpacing: "-1px", margin: "20px 0 12px" }}>
            Ready to talk to your data?
          </h2>
          <p style={{ fontSize: 16, color: "#666", marginBottom: 32 }}>Open source · Self-hosted · MIT License</p>
          <a href="/" style={{ padding: "16px 40px", borderRadius: 12, background: "#4A9EE8", color: "#fff", fontSize: 16, fontWeight: 500, textDecoration: "none", boxShadow: "0 4px 20px rgba(74,158,232,0.3)" }}>
            Get Started
          </a>
        </div>
      </section>

      {/* ─── Footer ─── */}
      <footer style={{ borderTop: "1px solid rgba(255,255,255,0.06)", padding: "32px 40px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <KaveonMark size={18} useDirectColor />
          <span style={{ fontSize: 13, color: "#555" }}>© {new Date().getFullYear()} Kaveon</span>
        </div>
        <div style={{ display: "flex", gap: 24 }}>
          <a href="/docs" target="_blank" style={{ fontSize: 13, color: "#555", textDecoration: "none" }}>Documentation</a>
          <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" style={{ fontSize: 13, color: "#555", textDecoration: "none" }}>GitHub</a>
          <a href="/" style={{ fontSize: 13, color: "#555", textDecoration: "none" }}>Launch App</a>
        </div>
      </footer>
    </div>
  );
}
