"use client";

import { useEffect, useRef, useState } from "react";
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

function Section({ children, style }: { children: React.ReactNode; style?: React.CSSProperties }) {
  const ref = useFadeIn();
  return <div ref={ref} style={style}>{children}</div>;
}

export default function AboutPage() {
  const r1 = useFadeIn(0);
  const r2 = useFadeIn(100);
  const r3 = useFadeIn(0);
  const r4 = useFadeIn(0);
  const r5 = useFadeIn(0);
  const r6 = useFadeIn(0);
  const r7 = useFadeIn(0);

  const [showcaseTheme, setShowcaseTheme] = useState<"dark" | "light">("dark");

  const B = "#4A9EE8";

  return (
    <div style={{ position: "fixed", inset: 0, background: "#0a0a0a", color: "#f5f5f5", overflowY: "auto", overflowX: "hidden" }}>

      <style>{`
        @keyframes float { 0%,100% { transform: translateY(0); } 50% { transform: translateY(-20px); } }
        @keyframes pulse { 0%,100% { opacity: 0.4; } 50% { opacity: 0.8; } }
        @keyframes gradientShift { 0% { background-position: 0% 50%; } 50% { background-position: 100% 50%; } 100% { background-position: 0% 50%; } }
        @keyframes shimmer { 0% { background-position: -200% 0; } 100% { background-position: 200% 0; } }
        @keyframes orb1 { 0%,100% { transform: translate(0,0) scale(1); } 50% { transform: translate(60px,-30px) scale(1.1); } }
        @keyframes orb2 { 0%,100% { transform: translate(0,0) scale(1); } 50% { transform: translate(-40px,40px) scale(0.9); } }
        .about-link:hover { color: #fff !important; }
        .about-card:hover { transform: translateY(-4px); border-color: rgba(255,255,255,0.12) !important; box-shadow: 0 12px 40px rgba(0,0,0,0.4) !important; }
        .about-btn:hover { transform: translateY(-2px); box-shadow: 0 8px 30px rgba(74,158,232,0.4) !important; }
        .dash-frame:hover { transform: scale(1.02); box-shadow: 0 24px 80px rgba(0,0,0,0.6), 0 0 40px rgba(74,158,232,0.08) !important; }
        .dash-frame:hover .dash-label { opacity: 1 !important; }
      `}</style>

      {/* ─── Nav ─── */}
      <nav style={{
        position: "fixed", top: 0, left: 0, right: 0, zIndex: 100,
        display: "flex", alignItems: "center", justifyContent: "space-between",
        padding: "14px 48px",
        background: "rgba(10,10,10,0.7)", backdropFilter: "blur(20px) saturate(180%)",
        borderBottom: "1px solid rgba(255,255,255,0.04)",
      }}>
        <a href="/about" style={{ display: "flex", alignItems: "center", textDecoration: "none" }}>
          <svg width="140" height="24" viewBox="60 50 1180 200" fill="none" xmlns="http://www.w3.org/2000/svg" style={{ shapeRendering: "geometricPrecision" }}>
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
            <path d="M 966.25 215.29 A 72.5 72.5 0 1 0 893.75 215.29" fill="none" stroke={B} strokeWidth="20" strokeLinecap="butt" />
          </svg>
        </a>
        <div style={{ display: "flex", alignItems: "center", gap: 36 }}>
          <a href="#features" className="about-link" style={{ fontSize: 13, color: "#666", textDecoration: "none", transition: "color 0.2s" }}>Features</a>
          <a href="/docs" target="_blank" className="about-link" style={{ fontSize: 13, color: "#666", textDecoration: "none", transition: "color 0.2s" }}>Docs</a>
          <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" className="about-link" style={{ fontSize: 13, color: "#666", textDecoration: "none", transition: "color 0.2s" }}>GitHub</a>
          <a href="/" className="about-btn" style={{ display: "inline-block", padding: "8px 20px", borderRadius: 8, background: B, color: "#fff", fontSize: 13, fontWeight: 600, textDecoration: "none", transition: "all 0.2s" }}>
            Launch App
          </a>
        </div>
      </nav>

      {/* ─── Hero ─── */}
      <section ref={r1} style={{ position: "relative", minHeight: "100vh", display: "flex", alignItems: "center", justifyContent: "center", textAlign: "center", padding: "60px 24px 80px", overflow: "hidden" }}>
        {/* Layered ambient lighting */}
        <div style={{ position: "absolute", top: "8%", left: "50%", transform: "translateX(-50%)", width: 1000, height: 600, borderRadius: "50%", background: `radial-gradient(ellipse, ${B}0a 0%, transparent 60%)`, pointerEvents: "none" }} />
        <div style={{ position: "absolute", top: "20%", left: "15%", width: 600, height: 600, borderRadius: "50%", background: `radial-gradient(circle, rgba(139,92,246,0.05) 0%, transparent 60%)`, animation: "orb1 14s ease-in-out infinite", pointerEvents: "none" }} />
        <div style={{ position: "absolute", bottom: "5%", right: "10%", width: 500, height: 500, borderRadius: "50%", background: `radial-gradient(circle, rgba(16,185,129,0.04) 0%, transparent 60%)`, animation: "orb2 11s ease-in-out infinite", pointerEvents: "none" }} />
        {/* Subtle grid */}
        <div style={{ position: "absolute", inset: 0, backgroundImage: "linear-gradient(rgba(255,255,255,0.015) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.015) 1px, transparent 1px)", backgroundSize: "80px 80px", pointerEvents: "none", maskImage: "radial-gradient(ellipse 70% 60% at 50% 40%, black 20%, transparent 100%)" }} />
        {/* Top-down light beam */}
        <div style={{ position: "absolute", top: 0, left: "50%", transform: "translateX(-50%)", width: 2, height: "30%", background: `linear-gradient(180deg, ${B}30, transparent)`, pointerEvents: "none" }} />

        <div style={{ position: "relative", maxWidth: 900 }}>
          {/* Guardian O — large, with glow ring */}
          <div style={{ position: "relative", display: "inline-block", marginBottom: 0 }}>
            <div style={{
              position: "absolute", inset: -20,
              borderRadius: "50%",
              background: `radial-gradient(circle, ${B}15 0%, transparent 70%)`,
              filter: "blur(20px)",
              pointerEvents: "none",
            }} />
            <KaveonMark size={200} useDirectColor />
          </div>

          <h1 style={{
            fontSize: "clamp(56px, 8vw, 96px)", fontWeight: 800, letterSpacing: "-3px", lineHeight: 1,
            margin: "0 0 28px",
            background: `linear-gradient(135deg, #ffffff 0%, #e2e8f0 30%, ${B} 70%, #8b5cf6 100%)`,
            backgroundSize: "300% 300%",
            animation: "gradientShift 8s ease infinite",
            WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent",
          }}>
            Talk to your data
          </h1>

          <p style={{
            fontSize: "clamp(18px, 2.2vw, 22px)", color: "#888", lineHeight: 1.7,
            maxWidth: 640, margin: "0 auto 20px", fontWeight: 400,
          }}>
            Connect your databases. Ask anything. Get instant answers with interactive charts — powered by a deterministic engine, not an LLM.
          </p>

          {/* Subtle accent line */}
          <div style={{ width: 60, height: 3, borderRadius: 2, background: `linear-gradient(90deg, ${B}, #8b5cf6)`, margin: "0 auto 36px" }} />

          {/* Trust signals */}
          <div style={{ display: "flex", justifyContent: "center", gap: 24, marginBottom: 44, flexWrap: "wrap" }}>
            {[
              { icon: "fa-bolt", text: "No LLM required" },
              { icon: "fa-database", text: "Multi-database" },
              { icon: "fa-lock-open", text: "MIT Licensed" },
            ].map(({ icon, text }) => (
              <div key={text} style={{ display: "flex", alignItems: "center", gap: 7, fontSize: 13, color: "#666" }}>
                <i className={`fas ${icon}`} style={{ fontSize: 11, color: B, opacity: 0.7 }} />
                {text}
              </div>
            ))}
          </div>

          <div style={{ display: "flex", justifyContent: "center", gap: 14 }}>
            <a href="/" className="about-btn" style={{
              display: "inline-flex", alignItems: "center", gap: 10,
              padding: "18px 44px", borderRadius: 14,
              background: `linear-gradient(135deg, ${B}, #3b82f6)`,
              color: "#fff", fontSize: 17, fontWeight: 700, textDecoration: "none",
              boxShadow: `0 6px 30px ${B}50, 0 2px 8px rgba(0,0,0,0.3)`,
              transition: "all 0.2s",
              letterSpacing: "-0.01em",
            }}>
              Try Kaveon <span style={{ fontSize: 20 }}>&rarr;</span>
            </a>
            <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" style={{
              display: "inline-flex", alignItems: "center", gap: 10,
              padding: "18px 36px", borderRadius: 14,
              background: "rgba(255,255,255,0.03)",
              border: "1px solid rgba(255,255,255,0.08)",
              color: "#bbb", fontSize: 17, fontWeight: 500, textDecoration: "none",
              transition: "all 0.2s",
              backdropFilter: "blur(8px)",
            }}>
              <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/></svg>
              Star on GitHub
            </a>
          </div>

          {/* Scroll indicator — bottom right */}
          <div style={{
            position: "fixed", bottom: 28, right: 28, zIndex: 50,
            display: "flex", flexDirection: "column", alignItems: "center", gap: 6,
            animation: "float 3s ease-in-out infinite",
          }}>
            <svg width="28" height="44" viewBox="0 0 28 44" fill="none">
              <rect x="1.5" y="1.5" width="25" height="41" rx="12.5" stroke="rgba(255,255,255,0.35)" strokeWidth="2" />
              <circle cx="14" cy="14" r="3.5" fill="rgba(255,255,255,0.6)">
                <animate attributeName="cy" values="14;28;14" dur="2s" repeatCount="indefinite" />
              </circle>
            </svg>
          </div>
        </div>
      </section>


      {/* ─── Chat Demo ─── */}
      <Section style={{ padding: "100px 24px", background: "linear-gradient(180deg, #0a0a0a 0%, #0f1520 50%, #0a0a0a 100%)" }}>
        <div style={{ maxWidth: 920, margin: "0 auto" }}>
          <div style={{ textAlign: "center", marginBottom: 48 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#8b5cf6", marginBottom: 12 }}>Conversational Analytics</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>Ask anything. Get answers.</h2>
          </div>

          <div style={{ background: "#141414", borderRadius: 20, border: "1px solid rgba(255,255,255,0.06)", overflow: "hidden", boxShadow: "0 40px 80px rgba(0,0,0,0.5)" }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "12px 20px", borderBottom: "1px solid rgba(255,255,255,0.04)", background: "rgba(255,255,255,0.02)" }}>
              <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#ff5f57" }} />
              <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#febc2e" }} />
              <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#28c840" }} />
              <span style={{ flex: 1, textAlign: "center", fontSize: 11, color: "#444" }}>kaveon.vercel.app</span>
            </div>
            <div style={{ display: "flex" }}>
              {/* Mini sidebar */}
              <div style={{ width: 180, borderRight: "1px solid rgba(255,255,255,0.04)", padding: "16px 12px", flexShrink: 0, display: "flex", flexDirection: "column", gap: 4 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "6px 8px", fontSize: 12, color: B, background: `${B}10`, borderRadius: 6 }}>
                  <span>+</span> New Chat
                </div>
                <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "6px 8px", fontSize: 12, color: "#555", borderRadius: 6 }}>
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>
                  Library
                </div>
                <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "6px 8px", fontSize: 12, color: "#555", borderRadius: 6 }}>
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>
                  SQL Lab
                </div>
                <div style={{ height: 1, background: "rgba(255,255,255,0.04)", margin: "8px 0" }} />
                <div style={{ fontSize: 10, color: "#333", padding: "0 8px", marginBottom: 4 }}>RECENT</div>
                {["COVID by country", "Revenue by region", "Top customers"].map(t => (
                  <div key={t} style={{ display: "flex", alignItems: "center", gap: 6, padding: "5px 8px", fontSize: 11, color: "#444" }}>
                    <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
                    {t}
                  </div>
                ))}
              </div>
              {/* Chat */}
              <div style={{ flex: 1, padding: "24px" }}>
                <div style={{ display: "flex", gap: 10, marginBottom: 18, justifyContent: "flex-end" }}>
                  <div style={{ background: B, color: "#fff", padding: "10px 16px", borderRadius: "14px 4px 14px 14px", fontSize: 14 }}>
                    Show confirmed cases by country
                  </div>
                  <div style={{ width: 24, height: 24, borderRadius: "50%", background: `linear-gradient(135deg, #6db3ed, #2d7dd2)`, display: "flex", alignItems: "center", justifyContent: "center", fontSize: 10, fontWeight: 700, color: "#fff", flexShrink: 0 }}>P</div>
                </div>
                <div style={{ display: "flex", gap: 10, marginBottom: 20 }}>
                  <KaveonMark size={20} useDirectColor />
                  <div style={{ flex: 1 }}>
                    <p style={{ fontSize: 13, color: "#999", lineHeight: 1.7, margin: "0 0 12px" }}>
                      Found <strong style={{ color: "#fff" }}>195</strong> results. Top 3: <strong style={{ color: "#fff" }}>United States</strong> (103.8M), <strong style={{ color: "#fff" }}>India</strong> (45.0M), <strong style={{ color: "#fff" }}>France</strong> (39.9M).
                    </p>
                    <div style={{ background: "#1a1a1a", borderRadius: 10, padding: "14px 18px 10px", border: "1px solid rgba(255,255,255,0.04)" }}>
                      <div style={{ fontSize: 11, color: "#555", marginBottom: 8, fontWeight: 500 }}>Confirmed Cases by Country</div>
                      <div style={{ display: "flex", alignItems: "flex-end", gap: 4, height: 80 }}>
                        {[{h:80,c:B},{h:35,c:"#10b981"},{h:31,c:"#f59e0b"},{h:27,c:"#ef4444"},{h:25,c:"#8b5cf6"},{h:20,c:"#ec4899"},{h:19,c:"#06b6d4"},{h:16,c:"#f97316"},{h:15,c:"#6366f1"},{h:13,c:"#14b8a6"}].map((b,i) => (
                          <div key={i} style={{ flex: 1, height: b.h, background: b.c, borderRadius: "3px 3px 0 0", opacity: 0.8 }} />
                        ))}
                      </div>
                      <div style={{ display: "flex", justifyContent: "space-between", marginTop: 5, fontSize: 8, color: "#333" }}>
                        {["US","IN","FR","DE","BR","JP","KR","IT","UK","RU"].map(c => <span key={c}>{c}</span>)}
                      </div>
                    </div>
                  </div>
                </div>
                <div style={{ display: "flex", gap: 10, justifyContent: "flex-end", marginBottom: 18 }}>
                  <div style={{ background: B, color: "#fff", padding: "10px 16px", borderRadius: "14px 4px 14px 14px", fontSize: 14 }}>
                    Show this on a world map
                  </div>
                  <div style={{ width: 24, height: 24, borderRadius: "50%", background: "linear-gradient(135deg, #6db3ed, #2d7dd2)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 10, fontWeight: 700, color: "#fff", flexShrink: 0 }}>P</div>
                </div>
                <div style={{ display: "flex", gap: 10 }}>
                  <KaveonMark size={20} useDirectColor />
                  <div style={{ flex: 1 }}>
                    <p style={{ fontSize: 13, color: "#999", lineHeight: 1.7, margin: "0 0 12px" }}>
                      Here&#39;s the global distribution. Darker regions indicate higher case counts.
                    </p>
                    <div style={{ background: "#1a1a1a", borderRadius: 10, padding: "14px 18px", border: "1px solid rgba(255,255,255,0.04)", position: "relative", height: 100, display: "flex", alignItems: "center", justifyContent: "center" }}>
                      <div style={{ fontSize: 11, color: "#555", fontWeight: 500 }}>
                        <svg width="200" height="60" viewBox="0 0 400 120" style={{ opacity: 0.4 }}>
                          {/* Simplified world outline */}
                          <ellipse cx="200" cy="60" rx="180" ry="50" fill="none" stroke="#333" strokeWidth="1" />
                          <ellipse cx="140" cy="50" rx="40" ry="25" fill={B} opacity="0.3" />
                          <ellipse cx="220" cy="45" rx="50" ry="20" fill={B} opacity="0.2" />
                          <ellipse cx="300" cy="55" rx="35" ry="30" fill={B} opacity="0.15" />
                          <ellipse cx="170" cy="80" rx="30" ry="15" fill="#10b981" opacity="0.2" />
                        </svg>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </Section>

      {/* ─── How It Works ─── */}
      <section ref={r3} style={{ padding: "100px 24px", background: "#0a0a0a" }}>
        <div style={{ maxWidth: 1000, margin: "0 auto" }}>
          <div style={{ textAlign: "center", marginBottom: 56 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: B, marginBottom: 12 }}>How It Works</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>Three steps. Zero complexity.</h2>
          </div>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 24 }}>
            {[
              { n: "01", title: "You ask", desc: "Type a question in natural language. No syntax. No training.", color: B, icon: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" },
              { n: "02", title: "We parse", desc: "A deterministic engine matches your words to schema metadata and generates SQL. No LLM. Instant.", color: "#8b5cf6", icon: "M13 2L3 14h9l-1 8 10-12h-9l1-8z" },
              { n: "03", title: "Data answers", desc: "SQL executes, the right visualization is selected, and you see data with an intelligent summary.", color: "#10b981", icon: "M22 12h-4l-3 9L9 3l-3 9H2" },
            ].map((s) => (
              <div key={s.n} style={{
                padding: "40px 32px", borderRadius: 16,
                background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.05)",
                textAlign: "center", transition: "all 0.3s",
              }} className="about-card">
                <div style={{
                  width: 56, height: 56, borderRadius: 16, margin: "0 auto 20px",
                  background: `${s.color}10`, border: `1px solid ${s.color}20`,
                  display: "flex", alignItems: "center", justifyContent: "center",
                }}>
                  <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke={s.color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d={s.icon} /></svg>
                </div>
                <div style={{ fontSize: 11, fontWeight: 700, color: s.color, letterSpacing: "0.08em", marginBottom: 8 }}>STEP {s.n}</div>
                <h3 style={{ fontSize: 20, fontWeight: 700, marginBottom: 10, color: "#e2e8f0" }}>{s.title}</h3>
                <p style={{ fontSize: 14, color: "#666", lineHeight: 1.7 }}>{s.desc}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* ─── Dashboard Showcase ─── */}
      <section id="dashboards" ref={r7} style={{ padding: "60px 24px 120px", background: "#0a0a0a", position: "relative" }}>
        {/* Ambient glow behind the screenshot */}
        <div style={{ position: "absolute", top: "30%", left: "50%", transform: "translateX(-50%)", width: 900, height: 500, borderRadius: "50%", background: `radial-gradient(circle, ${B}06 0%, transparent 70%)`, pointerEvents: "none" }} />

        <div style={{ textAlign: "center", marginBottom: 48, position: "relative" }}>
          <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#f59e0b", marginBottom: 12 }}>Dashboards</div>
          <h2 style={{ fontSize: "clamp(28px, 4vw, 44px)", fontWeight: 700, letterSpacing: "-1px", marginBottom: 12 }}>
            Build dashboards that tell stories
          </h2>
          <p style={{ fontSize: 16, color: "#666", maxWidth: 540, margin: "0 auto 28px" }}>
            World maps, KPI cards, trend lines, donut charts — drag, drop, publish. Every chart renders in parallel with cross-filtering.
          </p>
          {/* Theme toggle */}
          <div style={{ display: "inline-flex", borderRadius: 10, background: "rgba(255,255,255,0.04)", border: "1px solid rgba(255,255,255,0.06)", padding: 3, gap: 2 }}>
            {(["dark", "light"] as const).map(t => (
              <button key={t} onClick={() => setShowcaseTheme(t)} style={{
                padding: "7px 18px", borderRadius: 8, border: "none", cursor: "pointer",
                fontSize: 12, fontWeight: 600, transition: "all 0.2s",
                background: showcaseTheme === t ? (t === "dark" ? "rgba(255,255,255,0.1)" : "rgba(255,255,255,0.15)") : "transparent",
                color: showcaseTheme === t ? "#fff" : "#555",
              }}>
                {t === "dark" ? (
                  <><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ verticalAlign: -2, marginRight: 5 }}><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>Dark</>
                ) : (
                  <><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ verticalAlign: -2, marginRight: 5 }}><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>Light</>
                )}
              </button>
            ))}
          </div>
        </div>

        <div style={{ maxWidth: 1140, margin: "0 auto", position: "relative" }}>
          {/* Browser frame */}
          <div className="dash-frame" style={{
            borderRadius: 16, overflow: "hidden",
            border: "1px solid rgba(255,255,255,0.08)",
            boxShadow: `0 40px 100px rgba(0,0,0,0.6), 0 0 60px ${B}06`,
            transition: "all 0.4s ease",
          }}>
            {/* Chrome bar */}
            <div style={{
              display: "flex", alignItems: "center", gap: 8,
              padding: "10px 16px",
              background: showcaseTheme === "dark" ? "rgba(255,255,255,0.03)" : "#f3f4f6",
              borderBottom: `1px solid ${showcaseTheme === "dark" ? "rgba(255,255,255,0.04)" : "#e5e7eb"}`,
            }}>
              <div style={{ display: "flex", gap: 6 }}>
                <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#ff5f57" }} />
                <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#febc2e" }} />
                <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#28c840" }} />
              </div>
              <div style={{ flex: 1, display: "flex", justifyContent: "center" }}>
                <div style={{
                  display: "flex", alignItems: "center", gap: 6,
                  padding: "4px 16px", borderRadius: 6,
                  background: showcaseTheme === "dark" ? "rgba(255,255,255,0.04)" : "#e5e7eb",
                  fontSize: 11,
                  color: showcaseTheme === "dark" ? "#555" : "#999",
                }}>
                  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>
                  kaveon.vercel.app/dashboards/global-overview
                </div>
              </div>
            </div>
            {/* Screenshot */}
            <div style={{ position: "relative", overflow: "hidden" }}>
              <img
                src={showcaseTheme === "dark" ? "/showcase/dashboard-dark.png" : "/showcase/dashboard-light.png"}
                alt="COVID-19 Global Overview Dashboard"
                style={{
                  width: "100%", display: "block",
                  transition: "opacity 0.3s ease",
                }}
              />
            </div>
          </div>

          {/* Feature pills below */}
          <div style={{ display: "flex", justifyContent: "center", gap: 10, marginTop: 24, flexWrap: "wrap" }}>
            {[
              { icon: "fa-globe", text: "World Maps" },
              { icon: "fa-chart-line", text: "KPI Cards" },
              { icon: "fa-chart-pie", text: "Donut Charts" },
              { icon: "fa-filter", text: "Cross-filtering" },
              { icon: "fa-sun", text: "Light & Dark" },
              { icon: "fa-arrows-rotate", text: "Auto-refresh" },
            ].map(({ icon, text }) => (
              <div key={text} style={{
                display: "flex", alignItems: "center", gap: 6,
                padding: "6px 14px", borderRadius: 20,
                background: "rgba(255,255,255,0.03)", border: "1px solid rgba(255,255,255,0.06)",
                fontSize: 11.5, color: "#888", fontWeight: 500,
              }}>
                <i className={`fas ${icon}`} style={{ fontSize: 10, color: B, opacity: 0.7 }} />
                {text}
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* ─── SQL Lab Showcase ─── */}
      <Section style={{ padding: "100px 24px", background: "linear-gradient(180deg, #0a0a0a 0%, #0f1520 50%, #0a0a0a 100%)" }}>
        <div style={{ maxWidth: 1100, margin: "0 auto", display: "grid", gridTemplateColumns: "1.4fr 1fr", gap: 48, alignItems: "center" }}>
          <div style={{ background: "#111", borderRadius: 16, border: "1px solid rgba(255,255,255,0.05)", overflow: "hidden", boxShadow: "0 20px 60px rgba(0,0,0,0.5)" }}>
            <div style={{ padding: "10px 16px", borderBottom: "1px solid rgba(255,255,255,0.04)", display: "flex", gap: 12 }}>
              <span style={{ fontSize: 12, color: B, borderBottom: `2px solid ${B}`, paddingBottom: 8 }}>Query 1</span>
              <span style={{ fontSize: 12, color: "#555", paddingBottom: 8 }}>Query 2</span>
              <span style={{ fontSize: 12, color: "#555", paddingBottom: 8 }}>+ New</span>
            </div>
            <div style={{ padding: 16 }}>
              <div style={{ background: "#0a0a0a", borderRadius: 8, padding: "14px 16px", fontFamily: "'Fira Code', 'JetBrains Mono', monospace", fontSize: 12, lineHeight: 1.8, marginBottom: 12 }}>
                <span style={{ color: "#c678dd" }}>SELECT</span>{" "}
                <span style={{ color: "#e5c07b" }}>country</span>{", "}
                <span style={{ color: "#61afef" }}>SUM</span>{"("}
                <span style={{ color: "#e5c07b" }}>confirmed</span>{") "}
                <span style={{ color: "#c678dd" }}>AS</span>{" "}
                <span style={{ color: "#e5c07b" }}>total</span><br />
                <span style={{ color: "#c678dd" }}>FROM</span>{" "}
                <span style={{ color: "#98c379" }}>covid_by_country</span><br />
                <span style={{ color: "#c678dd" }}>GROUP BY</span>{" "}
                <span style={{ color: "#e5c07b" }}>country</span><br />
                <span style={{ color: "#c678dd" }}>ORDER BY</span>{" "}
                <span style={{ color: "#e5c07b" }}>total</span>{" "}
                <span style={{ color: "#c678dd" }}>DESC</span><br />
                <span style={{ color: "#c678dd" }}>LIMIT</span>{" "}
                <span style={{ color: "#d19a66" }}>10</span>{";"}
              </div>
              <div style={{ fontSize: 10, color: "#555", marginBottom: 8, display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{ color: "#10b981" }}>&#x2713;</span> 195 rows &middot; 42ms &middot; cached
              </div>
              <div style={{ background: "#0a0a0a", borderRadius: 8, overflow: "hidden" }}>
                <div style={{ display: "grid", gridTemplateColumns: "1.5fr 1fr", fontSize: 10, borderBottom: "1px solid rgba(255,255,255,0.04)" }}>
                  <div style={{ padding: "6px 12px", color: "#666", fontWeight: 600 }}>country</div>
                  <div style={{ padding: "6px 12px", color: "#666", fontWeight: 600 }}>total</div>
                </div>
                {[["United States", "103,802,702"], ["India", "45,035,393"], ["France", "39,866,718"], ["Germany", "38,437,756"], ["Brazil", "37,519,960"]].map(([c, v]) => (
                  <div key={c} style={{ display: "grid", gridTemplateColumns: "1.5fr 1fr", fontSize: 11, borderBottom: "1px solid rgba(255,255,255,0.02)" }}>
                    <div style={{ padding: "5px 12px", color: "#999" }}>{c}</div>
                    <div style={{ padding: "5px 12px", color: "#777" }}>{v}</div>
                  </div>
                ))}
              </div>
            </div>
          </div>
          <div>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "3px", color: "#8b5cf6", marginBottom: 16 }}>SQL Lab</div>
            <h3 style={{ fontSize: 32, fontWeight: 700, lineHeight: 1.2, marginBottom: 16, letterSpacing: "-0.5px", color: "#e2e8f0" }}>VS Code in your browser</h3>
            <p style={{ fontSize: 15, color: "#666", lineHeight: 1.8, marginBottom: 24 }}>
              Monaco editor with full SQL autocomplete, syntax highlighting, multi-tab sessions, query history, and result caching. Write, run, and save queries against any connected database.
            </p>
            <a href="/lab" style={{ fontSize: 14, color: B, textDecoration: "none", fontWeight: 500 }}>Open SQL Lab &rarr;</a>
          </div>
        </div>
      </Section>

      {/* ─── Adaptive Context Routing ─── */}
      <Section style={{ padding: "100px 24px", background: "linear-gradient(180deg, #0a0a0a 0%, #0f1520 50%, #0a0a0a 100%)" }}>
        <div style={{ maxWidth: 1000, margin: "0 auto", display: "grid", gridTemplateColumns: "1fr 1fr", gap: 60, alignItems: "center" }}>
          <div>
            <h3 style={{ fontSize: 32, fontWeight: 700, lineHeight: 1.2, marginBottom: 16, letterSpacing: "-0.5px", color: "#e2e8f0" }}>
              Adaptive Context Routing
            </h3>
            <p style={{ fontSize: 15, color: "#777", lineHeight: 1.8, marginBottom: 24 }}>
              Every question is routed through a per-element staleness scorer that reads database modification counters — not re-queries. Fresh data answers instantly from context. Stale elements trigger targeted live queries and self-heal for next time.
            </p>
            <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              {[
                "Per-element validity scoring from DBMS statistics",
                "Zero-query answers from profiled column distributions",
                "Self-healing feedback loop after each live query",
                "No LLM, no API keys, no latency",
              ].map(t => (
                <div key={t} style={{ display: "flex", alignItems: "center", gap: 10 }}>
                  <div style={{ width: 18, height: 18, borderRadius: 5, background: `${B}14`, border: `1px solid ${B}25`, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0 }}>
                    <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke={B} strokeWidth="3"><polyline points="20 6 9 17 4 12" /></svg>
                  </div>
                  <span style={{ fontSize: 13.5, color: "#999" }}>{t}</span>
                </div>
              ))}
            </div>
          </div>
          {/* Routing diagram */}
          <div style={{ padding: 32, borderRadius: 16, background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.05)" }}>
            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              {[
                { label: "Question", sub: "\"How many deaths in the US?\"", color: "#e2e8f0", bg: "rgba(255,255,255,0.04)" },
                { label: "Element Matching", sub: "covid_daily.deaths, covid_daily.country", color: "#8b5cf6", bg: "rgba(139,92,246,0.06)" },
                { label: "Validity Check", sub: "deaths: 0.92  ·  country: 0.88  ·  min: 0.88", color: "#10b981", bg: "rgba(16,185,129,0.06)" },
                { label: "Route: Context", sub: "Score 0.88 ≥ 0.70 threshold — answer from cache", color: B, bg: `${B}08` },
                { label: "Answer", sub: "1,127,152 deaths · 42ms · no query executed", color: "#f59e0b", bg: "rgba(245,158,11,0.06)" },
              ].map((step, i) => (
                <div key={step.label}>
                  <div style={{ padding: "12px 16px", borderRadius: 10, background: step.bg, border: `1px solid ${step.color}15` }}>
                    <div style={{ fontSize: 10, fontWeight: 700, color: step.color, textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 3 }}>{step.label}</div>
                    <div style={{ fontSize: 12, color: "#888", fontFamily: "var(--font-mono, monospace)" }}>{step.sub}</div>
                  </div>
                  {i < 4 && (
                    <div style={{ display: "flex", justifyContent: "center", padding: "4px 0" }}>
                      <div style={{ width: 1, height: 10, background: "rgba(255,255,255,0.08)" }} />
                    </div>
                  )}
                </div>
              ))}
            </div>
          </div>
        </div>
      </Section>

      {/* ─── Features Grid ─── */}
      <section id="features" ref={r4} style={{ padding: "100px 24px", background: "#0a0a0a" }}>
        <div style={{ maxWidth: 1100, margin: "0 auto" }}>
          <div style={{ textAlign: "center", marginBottom: 56 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: B, marginBottom: 12 }}>Platform</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>Everything you need</h2>
            <p style={{ fontSize: 16, color: "#666", marginTop: 12 }}>One platform. Ask questions, build charts, create dashboards, write SQL.</p>
          </div>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gridAutoRows: "auto", gap: 14 }}>
            <div className="about-card" style={{ gridColumn: "span 2", padding: 40, borderRadius: 16, background: `linear-gradient(135deg, ${B}06 0%, transparent 100%)`, border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s" }}>
              <div style={{ fontSize: 11, fontWeight: 700, color: B, textTransform: "uppercase", letterSpacing: "2px", marginBottom: 12 }}>Core</div>
              <h3 style={{ fontSize: 24, fontWeight: 700, marginBottom: 12, lineHeight: 1.3 }}>Conversational Data Querying</h3>
              <p style={{ fontSize: 15, color: "#777", lineHeight: 1.8, maxWidth: 500 }}>
                Type questions in plain English. A template-based NL&#x2192;SQL engine parses your words, matches schema metadata, generates SQL, and renders the answer as an interactive chart.
              </p>
            </div>
            <div className="about-card" style={{ padding: 36, borderRadius: 16, background: "linear-gradient(135deg, rgba(16,185,129,0.05) 0%, transparent 100%)", border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s" }}>
              <h3 style={{ fontSize: 18, fontWeight: 700, marginBottom: 10 }}>37 Chart Types</h3>
              <p style={{ fontSize: 13.5, color: "#777", lineHeight: 1.8 }}>Bar, line, pie, heatmap, treemap, scatter, funnel, gauge, world map, 3D globe. All dark-mode aware.</p>
            </div>
            <div className="about-card" style={{ padding: 36, borderRadius: 16, background: "linear-gradient(135deg, rgba(139,92,246,0.05) 0%, transparent 100%)", border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s" }}>
              <h3 style={{ fontSize: 18, fontWeight: 700, marginBottom: 10 }}>SQL Lab</h3>
              <p style={{ fontSize: 13.5, color: "#777", lineHeight: 1.8 }}>Monaco editor with autocomplete, multi-tab, query history, and caching.</p>
            </div>
            <div className="about-card" style={{ gridColumn: "span 2", padding: 40, borderRadius: 16, background: "linear-gradient(135deg, rgba(245,158,11,0.05) 0%, transparent 100%)", border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s" }}>
              <div style={{ fontSize: 11, fontWeight: 700, color: "#f59e0b", textTransform: "uppercase", letterSpacing: "2px", marginBottom: 12 }}>Build &amp; Share</div>
              <h3 style={{ fontSize: 24, fontWeight: 700, marginBottom: 12, lineHeight: 1.3 }}>Dashboards That Tell Stories</h3>
              <p style={{ fontSize: 15, color: "#777", lineHeight: 1.8, maxWidth: 500 }}>
                Drag-and-drop canvas with cross-chart filtering, shared filter bar, auto-refresh, and one-click publishing.
              </p>
            </div>
            {[
              { title: "Multi-Source", desc: "Fabric, Azure SQL, PostgreSQL, MySQL. Connect them all.", color: "rgba(236,72,153,0.05)" },
              { title: "Semantic Datasets", desc: "Define dimensions, metrics, and joins once. Reuse everywhere.", color: "rgba(6,182,212,0.05)" },
              { title: "Self-Hosted", desc: "Your infrastructure, your data. OAuth, RBAC, MIT licensed.", color: "rgba(99,102,241,0.05)" },
            ].map(f => (
              <div key={f.title} className="about-card" style={{ padding: 36, borderRadius: 16, background: `linear-gradient(135deg, ${f.color} 0%, transparent 100%)`, border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s" }}>
                <h3 style={{ fontSize: 18, fontWeight: 700, marginBottom: 10 }}>{f.title}</h3>
                <p style={{ fontSize: 13.5, color: "#777", lineHeight: 1.8 }}>{f.desc}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* ─── Tech Stack ─── */}
      <section ref={r5} style={{ padding: "100px 24px", background: "linear-gradient(180deg, #0a0a0a 0%, #0f1520 50%, #0a0a0a 100%)" }}>
        <div style={{ maxWidth: 900, margin: "0 auto" }}>
          <div style={{ textAlign: "center", marginBottom: 48 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: B, marginBottom: 12 }}>Stack</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>Built with</h2>
          </div>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(5, 1fr)", gap: 12 }}>
            {[
              { name: "Next.js", version: "15", color: "#fff" },
              { name: "React", version: "19", color: "#61dafb" },
              { name: "TypeScript", version: "5.x", color: "#3178c6" },
              { name: "FastAPI", version: "0.115", color: "#009688" },
              { name: "Python", version: "3.11", color: "#ffd43b" },
              { name: "ECharts", version: "5.x", color: "#e43961" },
              { name: "PostgreSQL", version: "16", color: "#336791" },
              { name: "Azure", version: "Container Apps", color: "#0078d4" },
              { name: "Vercel", version: "Edge", color: "#fff" },
              { name: "Bicep", version: "IaC", color: "#f7a21b" },
            ].map((t) => (
              <div key={t.name} className="about-card" style={{
                padding: "20px 16px", borderRadius: 12,
                background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.05)",
                textAlign: "center", transition: "all 0.3s",
              }}>
                <div style={{ fontSize: 15, fontWeight: 600, color: t.color, marginBottom: 4 }}>{t.name}</div>
                <div style={{ fontSize: 11, color: "#444" }}>{t.version}</div>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* ─── CTA ─── */}
      <section ref={r6} style={{ textAlign: "center", padding: "80px 24px 100px", position: "relative" }}>
        <div style={{ position: "absolute", bottom: -100, left: "50%", transform: "translateX(-50%)", width: 800, height: 400, borderRadius: "50%", background: `radial-gradient(circle, ${B}08 0%, transparent 70%)`, pointerEvents: "none" }} />
        <div style={{ position: "relative" }}>
          <KaveonMark size={44} useDirectColor />
          <h2 style={{ fontSize: "clamp(28px, 4vw, 42px)", fontWeight: 700, letterSpacing: "-1px", margin: "20px 0 12px" }}>
            Ready to talk to your data?
          </h2>
          <p style={{ fontSize: 15, color: "#666", marginBottom: 36 }}>Open source &middot; Self-hosted &middot; MIT License</p>
          <a href="/" className="about-btn" style={{ display: "inline-block", padding: "16px 44px", borderRadius: 12, background: B, color: "#fff", fontSize: 16, fontWeight: 600, textDecoration: "none", boxShadow: `0 4px 24px ${B}30`, transition: "all 0.2s" }}>
            Get Started
          </a>
        </div>
      </section>

      {/* ─── Footer ─── */}
      <footer style={{ borderTop: "1px solid rgba(255,255,255,0.04)", padding: "28px 48px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <KaveonMark size={16} useDirectColor />
          <span style={{ fontSize: 12, color: "#333" }}>&copy; {new Date().getFullYear()} Kaveon</span>
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
