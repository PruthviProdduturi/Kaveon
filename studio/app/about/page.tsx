"use client";

import { Fragment, useEffect, useRef, useState } from "react";
import Image from "next/image";
import Link from "next/link";
import { KaveonMark } from "../../components/KaveonMark";
import { PublicHeader } from "../../components/PublicHeader";

type AnimDirection = "up" | "down" | "left" | "right" | "scale" | "none";

function useScrollAnim(direction: AnimDirection = "up", delay = 0, duration = 0.8) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const transforms: Record<AnimDirection, string> = {
      up: "translateY(60px)",
      down: "translateY(-60px)",
      left: "translateX(-80px)",
      right: "translateX(80px)",
      scale: "scale(0.9)",
      none: "none",
    };
    el.style.opacity = "0";
    el.style.transform = transforms[direction];
    el.style.transition = `opacity ${duration}s cubic-bezier(0.16, 1, 0.3, 1) ${delay}ms, transform ${duration}s cubic-bezier(0.16, 1, 0.3, 1) ${delay}ms`;
    const obs = new IntersectionObserver(
      ([e]) => { if (e.isIntersecting) { el.style.opacity = "1"; el.style.transform = "translate(0) scale(1)"; obs.disconnect(); } },
      { threshold: 0.08, rootMargin: "0px 0px -50px 0px" }
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [direction, delay, duration]);
  return ref;
}

function Anim({ dir = "up" as AnimDirection, delay = 0, duration = 0.8, children, style, className }: {
  dir?: AnimDirection; delay?: number; duration?: number; children: React.ReactNode; style?: React.CSSProperties; className?: string;
}) {
  const ref = useScrollAnim(dir, delay, duration);
  return <div ref={ref} style={style} className={className}>{children}</div>;
}

// Legacy compat
function useFadeIn(delay = 0) { return useScrollAnim("up", delay); }
function Section({ children, style }: { children: React.ReactNode; style?: React.CSSProperties }) {
  return <Anim dir="up">{(() => { return <div style={style}>{children}</div>; })()}</Anim>;
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
  const [activeSlide, setActiveSlide] = useState(0);
  const [showScroll, setShowScroll] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);

  const [scrollProgress, setScrollProgress] = useState(0);

  useEffect(() => {
    const container = scrollRef.current;
    if (!container) return;
    const onScroll = () => {
      const max = container.scrollHeight - container.clientHeight;
      const progress = max > 0 ? container.scrollTop / max : 0;
      setScrollProgress(progress);
      setShowScroll(progress < 0.85);
    };
    container.addEventListener("scroll", onScroll, { passive: true });
    return () => container.removeEventListener("scroll", onScroll);
  }, []);
  const slides = [
    { dark: "/showcase/climate-dark.png", light: "/showcase/climate-dark.png", title: "Climate x Energy Impact", desc: "Cross-domain — warming vs renewables, carbon intensity across 188 countries" },
    { dark: "/showcase/ai-arena-dark.png", light: "/showcase/ai-arena-dark.png", title: "AI Model Arena", desc: "34 LLMs — Arena ELO rankings, benchmarks, pricing, head-to-head" },
    { dark: "/showcase/energy-dark.png", light: "/showcase/energy-dark.png", title: "Global Energy", desc: "220 countries — consumption, carbon intensity, energy mix, GHG trends" },
    { dark: "/showcase/taxi-dark.png", light: "/showcase/taxi-dark.png", title: "NYC Yellow Taxi", desc: "3M rides — borough map, revenue breakdown, trip analysis" },
  ];

  const B = "#4A9EE8";

  return (
    <div ref={scrollRef} style={{ position: "fixed", inset: 0, background: "#0a0a0a", color: "#f5f5f5", overflowY: "auto", overflowX: "hidden", scrollBehavior: "smooth", scrollPaddingTop: 60 }}>

      <style>{`
        @keyframes float { 0%,100% { transform: translateY(0); } 50% { transform: translateY(-20px); } }
        @keyframes pulse { 0%,100% { opacity: 0.4; } 50% { opacity: 0.8; } }
        @keyframes gradientShift { 0% { background-position: 0% 50%; } 50% { background-position: 100% 50%; } 100% { background-position: 0% 50%; } }
        @keyframes shimmer { 0% { background-position: -200% 0; } 100% { background-position: 200% 0; } }
        @keyframes orb1 { 0%,100% { transform: translate(0,0) scale(1); } 50% { transform: translate(60px,-30px) scale(1.1); } }
        @keyframes orb2 { 0%,100% { transform: translate(0,0) scale(1); } 50% { transform: translate(-40px,40px) scale(0.9); } }
        @keyframes beam-flow { 0% { transform: translateY(-100%); } 100% { transform: translateY(200%); } }
        @keyframes logo-drop-in {
          0% { opacity: 0; transform: translateY(-120px) scale(0.4); filter: blur(20px); }
          35% { opacity: 1; transform: translateY(8px) scale(1.08); filter: blur(0); }
          55% { transform: translateY(-4px) scale(0.97); }
          70% { transform: translateY(2px) scale(1.02); }
          85% { transform: translateY(-1px) scale(0.99); }
          100% { transform: translateY(0) scale(1); }
        }
        @keyframes logo-glow-settle {
          0% { opacity: 0; transform: scale(0.5); }
          50% { opacity: 0.7; transform: scale(1.3); }
          100% { opacity: 0.3; transform: scale(1); }
        }
        @keyframes logo-glow-breathe { 0%,100% { opacity: 0.25; transform: scale(1); } 50% { opacity: 0.5; transform: scale(1.1); } }
        @keyframes hero-title-drop {
          0% { opacity: 0; transform: translateY(-60px); filter: blur(10px); }
          40% { opacity: 1; transform: translateY(4px); filter: blur(0); }
          60% { transform: translateY(-2px); }
          100% { transform: translateY(0); }
        }
        @keyframes hero-sub-drop {
          0% { opacity: 0; transform: translateY(-40px); filter: blur(6px); }
          50% { opacity: 1; transform: translateY(3px); filter: blur(0); }
          100% { transform: translateY(0); }
        }
        @keyframes hero-cta-drop {
          0% { opacity: 0; transform: translateY(-30px) scale(0.9); }
          50% { opacity: 1; transform: translateY(3px) scale(1.02); }
          100% { transform: translateY(0) scale(1); }
        }
        @keyframes hero-line-draw { 0% { width: 0; opacity: 0; } 100% { width: 60px; opacity: 1; } }
        .hero-cta-primary { position: relative; overflow: hidden; }
        .hero-cta-primary::after { content: ''; position: absolute; top: -50%; left: -50%; width: 200%; height: 200%; background: linear-gradient(45deg, transparent 40%, rgba(255,255,255,0.18) 50%, transparent 60%); animation: shimmer 2.5s ease-in-out infinite; }
        .hero-cta-primary:hover::after { animation-duration: 1s; }
        .about-link:hover { color: #fff !important; }
        .about-card:hover { transform: translateY(-4px); border-color: rgba(255,255,255,0.12) !important; box-shadow: 0 12px 40px rgba(0,0,0,0.4) !important; }
        .about-btn:hover { transform: translateY(-2px); box-shadow: 0 8px 30px rgba(74,158,232,0.4) !important; }
        .dash-frame:hover { transform: scale(1.02); box-shadow: 0 24px 80px rgba(0,0,0,0.6), 0 0 40px rgba(74,158,232,0.08) !important; }
        .dash-frame:hover .dash-label { opacity: 1 !important; }
        @media (max-width: 768px) {
          .about-hero-actions { flex-direction: column; align-items: stretch !important; }
          .about-hero-actions > * { width: 100%; justify-content: center; }
          .about-chat-sidebar { display: none !important; }
          .about-slide-tabs { overflow-x: auto; justify-content: flex-start !important; padding-bottom: 8px; }
          .about-grid-3 { grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)) !important; }
          .about-grid-3 > * { grid-column: span 1 !important; }
          .about-grid-2 { grid-template-columns: 1fr !important; }
          .about-compare { grid-template-columns: 1fr !important; }
          .about-grid-sql { grid-template-columns: 1fr !important; }
          .about-flow { grid-template-columns: 1fr !important; }
          .about-tech { grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)) !important; }
          .about-footer { padding: 20px 16px !important; flex-wrap: wrap; gap: 12px !important; }
        }
        @media (prefers-reduced-motion: reduce) {
          *, *::before, *::after {
            animation-duration: 0.01ms !important;
            animation-iteration-count: 1 !important;
            scroll-behavior: auto !important;
            transition-duration: 0.01ms !important;
          }
        }
      `}</style>

      <PublicHeader active="about" />

      {/* Scroll indicator — travels down with scroll, fades near bottom */}
      <a
        href="#dashboards"
        style={{
          position: "fixed",
          bottom: `calc(${28 + scrollProgress * 60}px)`,
          right: 28,
          zIndex: 9999,
          opacity: showScroll ? Math.max(0, 1 - scrollProgress * 1.3) : 0,
          transition: "top 0.15s ease-out, opacity 0.3s ease",
          pointerEvents: showScroll ? "auto" : "none",
          display: "flex", flexDirection: "column", alignItems: "center", gap: 6,
          textDecoration: "none", cursor: "pointer",
          padding: 8, borderRadius: 20,
          background: "rgba(10,10,10,0.6)",
          border: "1px solid rgba(255,255,255,0.1)",
          backdropFilter: "blur(12px)",
          animation: "float 3s ease-in-out infinite",
        }}
      >
        <svg width="24" height="40" viewBox="0 0 24 40" fill="none">
          <rect x="1.5" y="1.5" width="21" height="37" rx="10.5" stroke={B} strokeWidth="1.5" strokeOpacity="0.6" />
          <circle cx="12" cy="12" r="3" fill={B} opacity="0.8">
            <animate attributeName="cy" values="12;26;12" dur="1.8s" repeatCount="indefinite" />
          </circle>
        </svg>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke={B} strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" style={{ opacity: 0.7 }}>
          <polyline points="6 9 12 15 18 9" />
        </svg>
      </a>

      {/* ─── Hero ─── */}
      <section ref={r1} style={{ position: "relative", minHeight: "100vh", display: "flex", alignItems: "center", justifyContent: "center", textAlign: "center", padding: "60px 24px 100px", overflow: "hidden" }}>
        {/* Layered ambient lighting */}
        <div style={{ position: "absolute", top: "8%", left: "50%", transform: "translateX(-50%)", width: 1000, height: 600, borderRadius: "50%", background: `radial-gradient(ellipse, ${B}0a 0%, transparent 60%)`, pointerEvents: "none" }} />
        <div style={{ position: "absolute", top: "20%", left: "15%", width: 600, height: 600, borderRadius: "50%", background: `radial-gradient(circle, rgba(139,92,246,0.05) 0%, transparent 60%)`, animation: "orb1 14s ease-in-out infinite", pointerEvents: "none" }} />
        <div style={{ position: "absolute", bottom: "5%", right: "10%", width: 500, height: 500, borderRadius: "50%", background: `radial-gradient(circle, rgba(16,185,129,0.04) 0%, transparent 60%)`, animation: "orb2 11s ease-in-out infinite", pointerEvents: "none" }} />
        {/* Subtle grid */}
        <div style={{ position: "absolute", inset: 0, backgroundImage: "linear-gradient(rgba(255,255,255,0.015) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.015) 1px, transparent 1px)", backgroundSize: "80px 80px", pointerEvents: "none", maskImage: "radial-gradient(ellipse 70% 60% at 50% 40%, black 20%, transparent 100%)" }} />

        {/* Beam — short, top only */}
        <div style={{
          position: "absolute", top: 0, left: "50%", transform: "translateX(-50%)",
          width: 1, height: "20%",
          background: `linear-gradient(180deg, ${B}30, ${B}10, transparent)`,
          pointerEvents: "none", zIndex: 1,
        }} />

        <div style={{ position: "relative", maxWidth: 900, zIndex: 2 }}>
          {/* Guardian O — spins into existence */}
          <style>{`
            @keyframes logo-materialize {
              0% { opacity: 0; transform: scale(2.5) rotate(0deg); filter: blur(40px) brightness(3); }
              25% { opacity: 0.6; transform: scale(1.4) rotate(0deg); filter: blur(15px) brightness(2); }
              50% { opacity: 0.9; transform: scale(0.92) rotate(0deg); filter: blur(3px) brightness(1.2); }
              70% { opacity: 1; transform: scale(1.05) rotate(0deg); filter: blur(0) brightness(1); }
              85% { transform: scale(0.98); }
              100% { transform: scale(1); }
            }
            @keyframes logo-ring-trace {
              0% { stroke-dashoffset: 754; opacity: 0; }
              10% { opacity: 1; }
              80% { stroke-dashoffset: 0; opacity: 0.6; }
              100% { stroke-dashoffset: 0; opacity: 0; }
            }
          `}</style>
          <div style={{ position: "relative", display: "inline-block", marginBottom: -24 }}>
            {/* Soft glow */}
            <div style={{
              position: "absolute", inset: -40,
              borderRadius: "50%",
              background: `radial-gradient(circle, ${B}15 0%, transparent 70%)`,
              filter: "blur(30px)",
              pointerEvents: "none",
              animation: "logo-glow-settle 0.8s ease 0.6s both, logo-glow-breathe 5s ease-in-out infinite 2s",
            }} />
            {/* Logo — spins in */}
            <div style={{ animation: "logo-materialize 1.4s cubic-bezier(0.16, 1, 0.3, 1) forwards" }}>
              <KaveonMark size={240} useDirectColor />
            </div>
          </div>

          <h1 style={{
            fontSize: "clamp(56px, 8vw, 96px)", fontWeight: 800, letterSpacing: "-3px", lineHeight: 1.2,
            margin: "0 0 20px", padding: "0 0 8px",
            background: `linear-gradient(135deg, #ffffff 0%, #e2e8f0 30%, ${B} 70%, #8b5cf6 100%)`,
            backgroundSize: "300% 300%",
            animation: "gradientShift 8s ease infinite, hero-title-drop 0.6s cubic-bezier(0.34, 1.56, 0.64, 1) 0.3s both",
            WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent",
          }}>
            Talk to your data
          </h1>

          <p style={{
            fontSize: "clamp(18px, 2.2vw, 22px)", color: "#888", lineHeight: 1.7,
            maxWidth: 640, margin: "0 auto 20px", fontWeight: 400,
            animation: "hero-sub-drop 0.5s cubic-bezier(0.34, 1.56, 0.64, 1) 0.5s both",
          }}>
            Connect your data. Ask questions. Build intelligence.<br />One governed platform for analytics, dashboards, and deterministic data reasoning.
          </p>

          {/* Subtle accent line */}
          <div style={{ height: 3, borderRadius: 2, background: `linear-gradient(90deg, ${B}, #8b5cf6)`, margin: "0 auto 36px", animation: "hero-line-draw 0.4s ease 0.7s both" }} />

          {/* Trust signals */}
          <div style={{ display: "flex", justifyContent: "center", gap: 24, marginBottom: 44, flexWrap: "wrap", animation: "hero-sub-drop 0.5s cubic-bezier(0.34, 1.56, 0.64, 1) 0.8s both" }}>
            {[
              { icon: "fa-shield-check", text: "Governed Semantics" },
              { icon: "fa-bolt", text: "Deterministic Resolution" },
              { icon: "fa-lock", text: "No Model Call on DLM Path" },
            ].map(({ icon, text }) => (
              <div key={text} style={{ display: "flex", alignItems: "center", gap: 7, fontSize: 13, color: "#666" }}>
                <i className={`fas ${icon}`} style={{ fontSize: 11, color: B, opacity: 0.7 }} />
                {text}
              </div>
            ))}
          </div>

          <div className="about-hero-actions" style={{ display: "flex", justifyContent: "center", gap: 14, animation: "hero-cta-drop 0.5s cubic-bezier(0.34, 1.56, 0.64, 1) 1.0s both" }}>
            <Link href="/" className="about-btn hero-cta-primary" style={{
              display: "inline-flex", alignItems: "center", gap: 10,
              padding: "18px 44px", borderRadius: 14,
              background: `linear-gradient(135deg, ${B}, #3b82f6)`,
              color: "#fff", fontSize: 17, fontWeight: 700, textDecoration: "none",
              boxShadow: `0 6px 30px ${B}50, 0 2px 8px rgba(0,0,0,0.3)`,
              transition: "all 0.2s",
              letterSpacing: "-0.01em",
            }}>
              Try Kaveon <span style={{ fontSize: 20 }}>&rarr;</span>
            </Link>
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

        </div>
      </section>

      {/* ─── Unified Platform ─── */}
      <section style={{ padding: "96px 24px", background: "#0a0a0a", borderTop: "1px solid rgba(255,255,255,0.05)", borderBottom: "1px solid rgba(255,255,255,0.05)" }}>
        <div style={{ maxWidth: 1080, margin: "0 auto" }}>
          <Anim dir="up" style={{ textAlign: "center", marginBottom: 48 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: B, marginBottom: 12 }}>The Unified Data Intelligence Platform</div>
            <h2 style={{ fontSize: "clamp(30px, 4vw, 44px)", fontWeight: 700, letterSpacing: "-1px", marginBottom: 14 }}>Three pillars. One governed system.</h2>
            <p style={{ maxWidth: 720, margin: "0 auto", color: "#8b98a9", fontSize: 16, lineHeight: 1.75 }}>Studio is the intelligence surface. The Data Language Model resolves supported questions deterministically. The Rust Engine provides the vectorized analytical foundation beneath both.</p>
          </Anim>
          <div className="about-grid-3" style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 18 }}>
            {[
              { name: "Kaveon Studio", status: "Available", color: B, body: "Questions, SQL Lab, governed dashboards, chart building, and operational exploration." },
              { name: "Data Language Model", status: "Available", color: "#8b5cf6", body: "Compiled dataset semantics and deterministic NL→SQL for supported question classes." },
              { name: "Kaveon Engine", status: "Alpha", color: "#f59e0b", body: "Arrow batch execution with local Parquet and Delta reads today; cloud object storage and distributed execution are targets." },
            ].map((pillar, index) => (
              <Anim key={pillar.name} dir="up" delay={index * 120} style={{ padding: "30px 28px", borderRadius: 16, background: "rgba(255,255,255,0.022)", border: "1px solid rgba(255,255,255,0.07)", minHeight: 220 }} className="about-card">
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12, marginBottom: 28 }}>
                  <span style={{ fontSize: 11, fontFamily: "monospace", color: "#596678" }}>0{index + 1}</span>
                  <span style={{ padding: "4px 9px", borderRadius: 999, border: `1px solid ${pillar.color}35`, background: `${pillar.color}10`, color: pillar.color, fontSize: 10, fontWeight: 800, letterSpacing: "0.06em", textTransform: "uppercase" }}>{pillar.status}</span>
                </div>
                <h3 style={{ margin: "0 0 12px", fontSize: 22, color: "#e8edf4" }}>{pillar.name}</h3>
                <p style={{ margin: 0, color: "#8592a3", fontSize: 14, lineHeight: 1.75 }}>{pillar.body}</p>
              </Anim>
            ))}
          </div>
          <div style={{ display: "flex", justifyContent: "center", gap: 18, flexWrap: "wrap", marginTop: 26, color: "#6f7d90", fontSize: 12 }}><span>Solid capabilities are available today</span><span>·</span><span>Alpha and target work is labeled explicitly</span></div>
        </div>
      </section>


      {/* ─── Chat Demo ─── */}
      <section style={{ padding: "100px 24px", background: "linear-gradient(180deg, #0a0a0a 0%, #0f1520 50%, #0a0a0a 100%)", overflow: "hidden" }}>
        <div style={{ maxWidth: 920, margin: "0 auto" }}>
          <Anim dir="up" style={{ textAlign: "center", marginBottom: 48 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#8b5cf6", marginBottom: 12 }}>Conversational Analytics</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>Ask anything. Get answers.</h2>
          </Anim>

          <Anim dir="right" delay={200} duration={1.0} style={{ background: "#141414", borderRadius: 20, border: "1px solid rgba(255,255,255,0.06)", overflow: "hidden", boxShadow: "0 40px 80px rgba(0,0,0,0.5)" }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "12px 20px", borderBottom: "1px solid rgba(255,255,255,0.04)", background: "rgba(255,255,255,0.02)" }}>
              <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#ff5f57" }} />
              <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#febc2e" }} />
              <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#28c840" }} />
              <span style={{ flex: 1, textAlign: "center", fontSize: 11, color: "#444" }}>kaveon.vercel.app</span>
            </div>
            <div style={{ display: "flex" }}>
              {/* Mini sidebar */}
              <div className="about-chat-sidebar" style={{ width: 180, borderRight: "1px solid rgba(255,255,255,0.04)", padding: "16px 12px", flexShrink: 0, display: "flex", flexDirection: "column", gap: 4 }}>
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
                {["Energy consumption by country", "Top models by ELO", "Renewables share trend"].map(t => (
                  <div key={t} style={{ display: "flex", alignItems: "center", gap: 6, padding: "5px 8px", fontSize: 11, color: "#444" }}>
                    <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
                    {t}
                  </div>
                ))}
              </div>
              {/* Chat — shows text-first, chart only when asked */}
              <div style={{ flex: 1, padding: "20px 24px" }}>
                {/* User: text question */}
                <div style={{ display: "flex", gap: 10, marginBottom: 16, justifyContent: "flex-end" }}>
                  <div style={{ background: B, color: "#fff", padding: "10px 16px", borderRadius: "14px 4px 14px 14px", fontSize: 14 }}>
                    Top 10 countries by carbon intensity
                  </div>
                  <div style={{ width: 24, height: 24, borderRadius: "50%", background: "linear-gradient(135deg, #6db3ed, #2d7dd2)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 10, fontWeight: 700, color: "#fff", flexShrink: 0 }}>P</div>
                </div>
                {/* Kaveon: text response — numbered list, no chart */}
                <div style={{ display: "flex", gap: 10, marginBottom: 20 }}>
                  <KaveonMark size={20} useDirectColor />
                  <div style={{ flex: 1, fontSize: 13, color: "#999", lineHeight: 1.8 }}>
                    <p style={{ margin: "0 0 8px", color: "#ccc", fontWeight: 600, fontSize: 14 }}>Top country by Carbon Intensity (10 results)</p>
                    <div style={{ display: "grid", gridTemplateColumns: "auto 1fr auto", gap: "2px 10px" }}>
                      {[["1","Turkmenistan","1,340"],["2","Uzbekistan","1,100"],["3","Bahrain","903"],["4","Libya","828"],["5","Kazakhstan","822"]].map(([n,c,v]) => (
                        <Fragment key={n}>
                          <span style={{ color: "#555", fontSize: 12 }}>{n}.</span>
                          <span style={{ color: "#e2e8f0", fontWeight: 500 }}>{c}</span>
                          <span style={{ color: "#777", fontFamily: "monospace", fontSize: 12 }}>{v} gCO₂</span>
                        </Fragment>
                      ))}
                    </div>
                    <p style={{ margin: "6px 0 0", fontSize: 11, color: "#555" }}>*...and 5 more*</p>
                    <div style={{ display: "flex", gap: 8, marginTop: 8, fontSize: 10, color: "#444" }}>
                      <span style={{ padding: "2px 8px", borderRadius: 10, background: "rgba(16,185,129,0.1)", color: "#10b981", fontWeight: 600 }}>From context</span>
                      <span>680ms · 4 elements</span>
                    </div>
                  </div>
                </div>
                {/* User: explicitly asks for chart */}
                <div style={{ display: "flex", gap: 10, marginBottom: 16, justifyContent: "flex-end" }}>
                  <div style={{ background: B, color: "#fff", padding: "10px 16px", borderRadius: "14px 4px 14px 14px", fontSize: 14 }}>
                    Show me a chart of that
                  </div>
                  <div style={{ width: 24, height: 24, borderRadius: "50%", background: "linear-gradient(135deg, #6db3ed, #2d7dd2)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 10, fontWeight: 700, color: "#fff", flexShrink: 0 }}>P</div>
                </div>
                {/* Kaveon: chart response — only because user asked */}
                <div style={{ display: "flex", gap: 10 }}>
                  <KaveonMark size={20} useDirectColor />
                  <div style={{ flex: 1 }}>
                    <div style={{ background: "#1a1a1a", borderRadius: 10, padding: "14px 18px 10px", border: "1px solid rgba(255,255,255,0.04)" }}>
                      <div style={{ fontSize: 11, color: "#555", marginBottom: 8, fontWeight: 500 }}>Carbon Intensity by Country</div>
                      <div style={{ display: "flex", alignItems: "flex-end", gap: 4, height: 60 }}>
                        {[80,60,54,50,49,46,44,40,38,36].map((h,i) => (
                          <div key={i} style={{ flex: 1, height: h, background: B, borderRadius: "3px 3px 0 0", opacity: 0.7 + i * -0.04 }} />
                        ))}
                      </div>
                    </div>
                    <div style={{ display: "flex", gap: 8, marginTop: 6, fontSize: 10, color: "#444" }}>
                      <span style={{ padding: "2px 8px", borderRadius: 10, background: `${B}15`, color: B, fontWeight: 600 }}>Live query</span>
                      <span>456ms</span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </Anim>
        </div>
      </section>

      {/* ─── How It Works ─── */}

      <section ref={r3} style={{ padding: "100px 24px", background: "#0a0a0a" }}>
        <div style={{ maxWidth: 1000, margin: "0 auto" }}>
          <div style={{ textAlign: "center", marginBottom: 56 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: B, marginBottom: 12 }}>One Governed Path</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>From source to decision.</h2>
            <p style={{ maxWidth: 660, margin: "14px auto 0", color: "#718094", fontSize: 15, lineHeight: 1.7 }}>Connect once, define meaning once, then explore through natural language, SQL, and reusable visual intelligence.</p>
          </div>
          <div className="about-flow" style={{ display: "grid", gridTemplateColumns: "repeat(5, 1fr)", gap: 14 }}>
            {[
              { n: "01", title: "Connect", desc: "Register governed sources without moving the underlying data.", color: B, icon: "M4 4h16v6H4zM4 14h16v6H4z" },
              { n: "02", title: "Model", desc: "Define reusable dimensions, metrics, relationships, and business meaning.", color: "#06b6d4", icon: "M12 2v20M2 12h20" },
              { n: "03", title: "Ask or query", desc: "Use deterministic DLM resolution or write SQL directly in Studio.", color: "#8b5cf6", icon: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" },
              { n: "04", title: "Execute", desc: "Run vectorized analytical work through the Engine or a selected source.", color: "#f59e0b", icon: "M13 2L3 14h9l-1 8 10-12h-9l1-8z" },
              { n: "05", title: "Visualize", desc: "Turn answers into charts, dashboards, and operational decisions.", color: "#10b981", icon: "M4 19V9m6 10V5m6 14v-7m4 7H2" },
            ].map((s, idx) => (
              <Anim key={s.n} dir="up" delay={idx * 150} style={{
                padding: "32px 22px", borderRadius: 16,
                background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.05)",
                textAlign: "center", transition: "all 0.3s",
              }} className="about-card">
                <div style={{
                  width: 50, height: 50, borderRadius: 14, margin: "0 auto 20px",
                  background: `${s.color}10`, border: `1px solid ${s.color}20`,
                  display: "flex", alignItems: "center", justifyContent: "center",
                }}>
                  <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke={s.color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d={s.icon} /></svg>
                </div>
                <div style={{ fontSize: 11, fontWeight: 700, color: s.color, letterSpacing: "0.08em", marginBottom: 8 }}>STEP {s.n}</div>
                <h3 style={{ fontSize: 18, fontWeight: 700, marginBottom: 10, color: "#e2e8f0" }}>{s.title}</h3>
                <p style={{ fontSize: 13, color: "#718094", lineHeight: 1.65 }}>{s.desc}</p>
              </Anim>
            ))}
          </div>
        </div>
      </section>

      {/* ─── Why No LLM — Enterprise value props ─────────────────────── */}
      <Section style={{ padding: "100px 24px", background: "linear-gradient(180deg, #0a0a0a 0%, #0f1218 50%, #0a0a0a 100%)" }}>
        <div style={{ maxWidth: 1000, margin: "0 auto" }}>
          <div style={{ textAlign: "center", marginBottom: 56 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#10b981", marginBottom: 12 }}>Architecture</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>
              Why deterministic, not generative?
            </h2>
            <p style={{ fontSize: 16, color: "#666", maxWidth: 560, margin: "12px auto 0" }}>
              Every NL&#x2192;SQL product today sends your schema to an LLM. Kaveon doesn&apos;t.
            </p>
          </div>

          <div className="about-grid-3" style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 20 }}>
            {[
              {
                icon: "fa-shield-halved",
                title: "Bounded Semantics",
                desc: "Supported questions map to compiled dataset metadata and schema boundaries instead of unconstrained token prediction.",
                color: "#10b981",
                metric: "Governed",
                metricLabel: "resolution path",
              },
              {
                icon: "fa-bolt",
                title: "Predictable Runtime",
                desc: "Compiled rule-based parsing avoids a token-generation pipeline. Published latency claims require a reproducible benchmark context.",
                color: B,
                metric: "Compiled",
                metricLabel: "dataset context",
              },
              {
                icon: "fa-lock",
                title: "Explicit AI Boundary",
                desc: "The deterministic DLM path makes no model call. Optional hosted-AI features remain separately configured and governed.",
                color: "#8b5cf6",
                metric: "No tokens",
                metricLabel: "on the DLM path",
              },
            ].map(({ icon, title, desc, color, metric, metricLabel }, idx) => (
              <Anim key={title} dir="up" delay={idx * 150} style={{
                padding: "36px 28px", borderRadius: 16,
                background: "rgba(255,255,255,0.02)",
                border: "1px solid rgba(255,255,255,0.05)",
                transition: "all 0.3s",
              }} className="about-card">
                <div style={{
                  fontSize: 36, fontWeight: 800, color, letterSpacing: "-1px",
                  marginBottom: 4, lineHeight: 1,
                }}>
                  {metric}
                </div>
                <div style={{ fontSize: 11, color: "#555", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 20 }}>
                  {metricLabel}
                </div>
                <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}>
                  <i className={`fas ${icon}`} style={{ fontSize: 14, color, opacity: 0.8 }} />
                  <h3 style={{ fontSize: 17, fontWeight: 700, color: "#e2e8f0", margin: 0 }}>{title}</h3>
                </div>
                <p style={{ fontSize: 13.5, color: "#777", lineHeight: 1.7, margin: 0 }}>{desc}</p>
              </Anim>
            ))}
          </div>

          {/* Comparison row */}
          <div className="about-compare" style={{
            display: "grid", gridTemplateColumns: "1fr auto 1fr", gap: 20, alignItems: "center",
            marginTop: 40, padding: "28px 32px", borderRadius: 16,
            background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.05)",
          }}>
            <div style={{ textAlign: "center" }}>
              <div style={{ fontSize: 13, fontWeight: 700, color: "#ef4444", marginBottom: 8 }}>LLM-based NL&#x2192;SQL</div>
              <div style={{ fontSize: 12, color: "#666", lineHeight: 1.7 }}>
                500-2000ms latency &middot; $0.01-0.05/query &middot; Schema leaked to provider &middot; Hallucination risk &middot; Rate limits
              </div>
            </div>
            <div style={{ fontSize: 20, color: "#333", fontWeight: 300 }}>vs</div>
            <div style={{ textAlign: "center" }}>
              <div style={{ fontSize: 13, fontWeight: 700, color: "#10b981", marginBottom: 8 }}>Kaveon Data Language Model</div>
              <div style={{ fontSize: 12, color: "#666", lineHeight: 1.7 }}>
                Compiled semantics &middot; No model call on the DLM path &middot; Governed fallback to live SQL
              </div>
            </div>
          </div>
        </div>
      </Section>

      {/* ─── Data Language Model (DLM) ─── */}
      <section style={{ padding: "100px 24px", background: "#0a0a0a", position: "relative", overflow: "hidden" }}>
        <div style={{ position: "absolute", top: "30%", right: "-10%", width: 800, height: 800, borderRadius: "50%", background: "radial-gradient(circle, rgba(245,158,11,0.04) 0%, transparent 60%)", pointerEvents: "none" }} />
        <div style={{ maxWidth: 1000, margin: "0 auto", position: "relative" }}>
          <Anim dir="up" style={{ textAlign: "center", marginBottom: 56 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#f59e0b", marginBottom: 12 }}>Data Language Layer</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>
              The Data Language Model
            </h2>
            <p style={{ fontSize: 16, color: "#666", maxWidth: 620, margin: "12px auto 0" }}>
              A self-compiling semantic layer that turns your schema into a deterministic question-answering engine — no training, no fine-tuning, no LLM.
            </p>
          </Anim>

          <div className="about-grid-2" style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 60, alignItems: "center" }}>
            <Anim dir="left" style={{ padding: 32, borderRadius: 16, background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.05)" }}>
              <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
                {[
                  { label: "Register", sub: "Define metrics, dimensions, and joins on your tables", color: "#f59e0b" },
                  { label: "Compile", sub: "Auto-index every distinct value, build synonym maps", color: "#8b5cf6" },
                  { label: "Precompute", sub: "Metric totals + per-dimension breakdowns with HLL sketches", color: "#10b981" },
                  { label: "Route", sub: "Weighted lexical scoring ranks datasets per question", color: B },
                  { label: "Answer", sub: "Serve compiled context or issue one governed live query", color: "#ec4899" },
                ].map((step, i) => (
                  <div key={step.label}>
                    <div style={{ padding: "12px 16px", borderRadius: 10, background: `${step.color}08`, border: `1px solid ${step.color}15` }}>
                      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 3 }}>
                        <div style={{ width: 20, height: 20, borderRadius: 6, background: `${step.color}18`, display: "flex", alignItems: "center", justifyContent: "center", fontSize: 10, fontWeight: 800, color: step.color }}>{i + 1}</div>
                        <div style={{ fontSize: 12, fontWeight: 700, color: step.color, textTransform: "uppercase", letterSpacing: "0.06em" }}>{step.label}</div>
                      </div>
                      <div style={{ fontSize: 12, color: "#888", fontFamily: "var(--font-mono, monospace)", paddingLeft: 28 }}>{step.sub}</div>
                    </div>
                    {i < 4 && (
                      <div style={{ display: "flex", justifyContent: "center", padding: "4px 0" }}>
                        <div style={{ width: 1, height: 10, background: "rgba(255,255,255,0.08)" }} />
                      </div>
                    )}
                  </div>
                ))}
              </div>
            </Anim>

            <Anim dir="right" delay={200}>
              <h3 style={{ fontSize: 28, fontWeight: 700, lineHeight: 1.2, marginBottom: 16, letterSpacing: "-0.5px", color: "#e2e8f0" }}>
                Schema in, answers out
              </h3>
              <p style={{ fontSize: 15, color: "#777", lineHeight: 1.8, marginBottom: 24 }}>
                Register a dataset and the DLM compiles itself — indexing every column value, mapping synonyms, and precomputing metric rollups across every dimension. Questions resolve to SQL through deterministic pattern matching, not token prediction.
              </p>
              <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                {[
                  { text: "Precomputed context can answer without a source scan", color: "#10b981" },
                  { text: "HLL sketches for COUNT(DISTINCT) without full scans", color: "#8b5cf6" },
                  { text: "Value index resolves entity filters from natural language", color: "#f59e0b" },
                  { text: "Multi-dataset routing scores each question across all DLMs", color: B },
                  { text: "Self-healing — stale context auto-rebuilds on next ask", color: "#ec4899" },
                ].map(({ text, color }) => (
                  <div key={text} style={{ display: "flex", alignItems: "center", gap: 10 }}>
                    <div style={{ width: 18, height: 18, borderRadius: 5, background: `${color}14`, border: `1px solid ${color}25`, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0 }}>
                      <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="3"><polyline points="20 6 9 17 4 12" /></svg>
                    </div>
                    <span style={{ fontSize: 13.5, color: "#999" }}>{text}</span>
                  </div>
                ))}
              </div>
            </Anim>
          </div>
        </div>
      </section>

      {/* ─── Freshness Algorithm ─── */}
      <section style={{ padding: "100px 24px", background: "linear-gradient(180deg, #0a0a0a 0%, #0f1520 50%, #0a0a0a 100%)", position: "relative", overflow: "hidden" }}>
        <div style={{ position: "absolute", bottom: "10%", left: "-5%", width: 700, height: 700, borderRadius: "50%", background: "radial-gradient(circle, rgba(16,185,129,0.04) 0%, transparent 60%)", pointerEvents: "none" }} />
        <div style={{ maxWidth: 1000, margin: "0 auto", position: "relative" }}>
          <Anim dir="up" style={{ textAlign: "center", marginBottom: 56 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#10b981", marginBottom: 12 }}>Freshness Algorithm</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>
              Zero-scan change detection
            </h2>
            <p style={{ fontSize: 16, color: "#666", maxWidth: 600, margin: "12px auto 0" }}>
              Every precomputed answer carries a validity score in [0, 1]. The algorithm detects data drift from database catalog counters — never by re-querying your tables.
            </p>
          </Anim>

          {/* Formula */}
          <Anim dir="scale" style={{ textAlign: "center", marginBottom: 48 }}>
            <div style={{
              display: "inline-block", padding: "20px 40px", borderRadius: 14,
              background: "rgba(16,185,129,0.04)", border: "1px solid rgba(16,185,129,0.12)",
            }}>
              <div style={{ fontFamily: "var(--font-mono, monospace)", fontSize: "clamp(16px, 2.5vw, 22px)", color: "#e2e8f0", letterSpacing: "0.02em" }}>
                <span style={{ color: "#10b981", fontWeight: 700 }}>score</span>
                <span style={{ color: "#555", margin: "0 8px" }}>=</span>
                <span style={{ color: "#f59e0b" }}>time_decay</span>
                <span style={{ color: "#555", margin: "0 8px" }}>&times;</span>
                <span style={{ color: "#8b5cf6" }}>change_factor</span>
              </div>
              <div style={{ fontSize: 11, color: "#555", marginTop: 6 }}>
                score &lt; 0.5 triggers rebuild &middot; per-element, not per-dataset
              </div>
            </div>
          </Anim>

          {/* Three factors */}
          <div className="about-grid-3" style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 20, marginBottom: 40 }}>
            {[
              {
                label: "Time Decay",
                color: "#f59e0b",
                formula: "e^(-ln2 · age / half_life)",
                desc: "Exponential decay with a 6-hour base half-life. Unused context ages out gracefully; heavily-used context decays faster so your most-relied-upon answers stay freshest.",
                detail: "6h base · 10min floor",
              },
              {
                label: "Change Factor",
                color: "#8b5cf6",
                formula: "e^(-ln2 · Δrows / threshold)",
                desc: "Reads pg_stat_user_tables.n_mod_since_analyze — a counter the database maintains for free. Detects data drift without scanning a single row.",
                detail: "5% churn = half stale",
              },
              {
                label: "Usage Weight",
                color: "#10b981",
                formula: "hl / (1 + 0.35 · ln(usage))",
                desc: "Hot elements get a shorter effective half-life. The answers your team relies on most are kept the freshest — the algorithm learns from access patterns.",
                detail: "Modulates time decay",
              },
            ].map(({ label, color, formula, desc, detail }, idx) => (
              <Anim key={label} dir="up" delay={idx * 120}>
                <div className="about-card" style={{
                  padding: "32px 24px", borderRadius: 16,
                  background: `${color}06`, border: `1px solid ${color}12`,
                  transition: "all 0.3s", height: "100%",
                }}>
                  <div style={{ fontSize: 11, fontWeight: 700, color, textTransform: "uppercase", letterSpacing: "0.08em", marginBottom: 12 }}>{label}</div>
                  <div style={{
                    fontFamily: "var(--font-mono, monospace)", fontSize: 12, color: "#ccc",
                    padding: "8px 12px", borderRadius: 8, background: "rgba(0,0,0,0.3)",
                    marginBottom: 14, textAlign: "center",
                  }}>{formula}</div>
                  <p style={{ fontSize: 13.5, color: "#777", lineHeight: 1.7, margin: "0 0 12px" }}>{desc}</p>
                  <div style={{ fontSize: 11, color: "#555", fontWeight: 600 }}>{detail}</div>
                </div>
              </Anim>
            ))}
          </div>

          {/* Three trigger paths */}
          <Anim dir="up" delay={200}>
            <div style={{
              padding: "28px 32px", borderRadius: 16,
              background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.05)",
            }}>
              <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "3px", color: "#10b981", marginBottom: 16 }}>Three Rebuild Triggers</div>
              <div className="about-grid-3" style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 16 }}>
                {[
                  { title: "On Ask", desc: "Every question checks freshness. Stale context triggers a background rebuild while the live query answers.", icon: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" },
                  { title: "Proactive Sweep", desc: "A background loop checks all datasets every 30 minutes. Stale artifacts rebuild before anyone asks.", icon: "M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" },
                  { title: "Pipeline Webhook", desc: "POST /dlm/notify-data-change after ETL completes. Instant DLM context invalidation and rebuild.", icon: "M13 2L3 14h9l-1 8 10-12h-9l1-8z" },
                ].map(({ title, desc, icon }) => (
                  <div key={title} style={{ display: "flex", gap: 12, alignItems: "flex-start" }}>
                    <div style={{
                      width: 32, height: 32, borderRadius: 8, background: "rgba(16,185,129,0.08)", border: "1px solid rgba(16,185,129,0.15)",
                      display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0, marginTop: 2,
                    }}>
                      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#10b981" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d={icon} /></svg>
                    </div>
                    <div>
                      <div style={{ fontSize: 14, fontWeight: 700, color: "#e2e8f0", marginBottom: 4 }}>{title}</div>
                      <div style={{ fontSize: 12.5, color: "#777", lineHeight: 1.6 }}>{desc}</div>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </Anim>
        </div>
      </section>

      {/* ─── Adaptive Context Routing ─── */}
      <Section style={{ padding: "100px 24px", background: "#0a0a0a" }}>
        <div className="about-grid-2" style={{ maxWidth: 1000, margin: "0 auto", display: "grid", gridTemplateColumns: "1fr 1fr", gap: 60, alignItems: "center" }}>
          <Anim dir="left">
            <h3 style={{ fontSize: 32, fontWeight: 700, lineHeight: 1.2, marginBottom: 16, letterSpacing: "-0.5px", color: "#e2e8f0" }}>
              Adaptive Context Routing
            </h3>
            <p style={{ fontSize: 15, color: "#777", lineHeight: 1.8, marginBottom: 24 }}>
              Every question is routed through a per-element staleness scorer that reads database modification counters — not re-queries. Fresh data answers instantly from DLM context. Stale elements trigger targeted live queries and self-heal for next time.
            </p>
            <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              {[
                "Per-element validity scoring from DBMS statistics",
                "Zero-query answers from precomputed DLM context",
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
          </Anim>
          {/* Routing diagram */}
          <Anim dir="right" delay={200} style={{ padding: 32, borderRadius: 16, background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.05)" }}>
            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              {[
                { label: "Question", sub: "\"How many deaths in the US?\"", color: "#e2e8f0", bg: "rgba(255,255,255,0.04)" },
                { label: "Element Matching", sub: "deaths → metric, US → country filter", color: "#8b5cf6", bg: "rgba(139,92,246,0.06)" },
                { label: "Validity Check", sub: "deaths: 0.92  ·  country: 0.88  ·  min: 0.88", color: "#10b981", bg: "rgba(16,185,129,0.06)" },
                { label: "Route: DLM Context", sub: "Score 0.88 ≥ 0.70 threshold — answer from precomputed context", color: B, bg: `${B}08` },
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
          </Anim>
        </div>
      </Section>

      {/* ─── Dashboard Showcase — Apple-style horizontal scroll ─────── */}
      <section id="dashboards" ref={r7} style={{ padding: "60px 0 100px", background: "#0a0a0a", position: "relative", overflow: "hidden" }}>
        <div style={{ position: "absolute", top: "20%", left: "50%", transform: "translateX(-50%)", width: 1200, height: 600, borderRadius: "50%", background: `radial-gradient(circle, ${B}06 0%, transparent 60%)`, pointerEvents: "none" }} />

        <Anim dir="up" style={{ textAlign: "center", marginBottom: 40, position: "relative", padding: "0 24px" }}>
          <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: "#f59e0b", marginBottom: 12 }}>Dashboards</div>
          <h2 style={{ fontSize: "clamp(28px, 4vw, 44px)", fontWeight: 700, letterSpacing: "-1px", marginBottom: 12 }}>
            Build dashboards that tell stories
          </h2>
          <p style={{ fontSize: 16, color: "#666", maxWidth: 540, margin: "0 auto 28px" }}>
            World maps, KPI cards, trend lines, donut charts — drag, drop, publish.
          </p>
          {/* Theme toggle */}
          <div role="group" aria-label="Dashboard theme" style={{ display: "inline-flex", borderRadius: 10, background: "rgba(255,255,255,0.04)", border: "1px solid rgba(255,255,255,0.06)", padding: 3, gap: 2 }}>
            {(["dark", "light"] as const).map(t => (
              <button key={t} aria-pressed={showcaseTheme === t} onClick={() => setShowcaseTheme(t)} style={{
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
        </Anim>

        {/* Stacked dashboard viewer */}
        <Anim dir="left" delay={200} style={{ maxWidth: 1140, margin: "0 auto", position: "relative", padding: "0 24px" }}>
          {/* Browser frame with active screenshot */}
          <div style={{
            borderRadius: 16, overflow: "hidden",
            border: `1px solid ${showcaseTheme === "dark" ? "rgba(255,255,255,0.08)" : "rgba(0,0,0,0.08)"}`,
            boxShadow: `0 40px 100px rgba(0,0,0,0.6), 0 0 60px ${B}04`,
          }}>
            {/* Chrome bar */}
            <div style={{
              display: "flex", alignItems: "center", gap: 8, padding: "10px 16px",
              background: showcaseTheme === "dark" ? "rgba(255,255,255,0.03)" : "#f3f4f6",
              borderBottom: `1px solid ${showcaseTheme === "dark" ? "rgba(255,255,255,0.04)" : "#e5e7eb"}`,
            }}>
              <div style={{ display: "flex", gap: 6 }}>
                <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#ff5f57" }} />
                <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#febc2e" }} />
                <div style={{ width: 10, height: 10, borderRadius: "50%", background: "#28c840" }} />
              </div>
              <div style={{ flex: 1, textAlign: "center" }}>
                <span style={{
                  fontSize: 11, padding: "3px 14px", borderRadius: 6,
                  background: showcaseTheme === "dark" ? "rgba(255,255,255,0.04)" : "#e5e7eb",
                  color: showcaseTheme === "dark" ? "#555" : "#999",
                }}>kaveon.vercel.app</span>
              </div>
            </div>
            {/* Screenshot — crossfade, active image sets height */}
            <div style={{ position: "relative", overflow: "hidden" }}>
              {slides.map((slide, i) => (
                <Image
                  key={slide.title}
                  src={showcaseTheme === "dark" ? slide.dark : slide.light}
                  alt={slide.title}
                  width={1600}
                  height={900}
                  sizes="(max-width: 768px) 100vw, 1140px"
                  aria-hidden={activeSlide !== i}
                  style={{
                    width: "100%", display: "block",
                    ...(activeSlide === i
                      ? { position: "relative", opacity: 1 }
                      : { position: "absolute", top: 0, left: 0, opacity: 0, pointerEvents: "none" }),
                    transition: "opacity 0.5s ease",
                  }}
                />
              ))}
            </div>
          </div>

          {/* Dashboard selector tabs */}
          <div className="about-slide-tabs" role="tablist" aria-label="Dashboard examples" style={{ display: "flex", justifyContent: "center", gap: 12, marginTop: 24 }}>
            {slides.map((slide, i) => (
              <button
                key={slide.title}
                role="tab"
                aria-selected={activeSlide === i}
                onClick={() => setActiveSlide(i)}
                style={{
                  display: "flex", alignItems: "center", gap: 10,
                  padding: "10px 20px", borderRadius: 12, cursor: "pointer",
                  background: activeSlide === i ? "rgba(255,255,255,0.08)" : "rgba(255,255,255,0.02)",
                  border: `1px solid ${activeSlide === i ? "rgba(255,255,255,0.12)" : "rgba(255,255,255,0.05)"}`,
                  transition: "all 0.2s",
                }}
              >
                <div style={{
                  width: 8, height: 8, borderRadius: "50%",
                  background: activeSlide === i ? B : "#444",
                  transition: "background 0.2s",
                }} />
                <div style={{ textAlign: "left" }}>
                  <div style={{ fontSize: 13, fontWeight: 600, color: activeSlide === i ? "#fff" : "#777" }}>{slide.title}</div>
                  <div style={{ fontSize: 11, color: "#555" }}>{slide.desc}</div>
                </div>
              </button>
            ))}
          </div>
        </Anim>
      </section>

      {/* ─── SQL Lab Showcase ─── */}
      <Section style={{ padding: "100px 24px", background: "linear-gradient(180deg, #0a0a0a 0%, #0f1520 50%, #0a0a0a 100%)" }}>
        <div className="about-grid-sql" style={{ maxWidth: 1100, margin: "0 auto", display: "grid", gridTemplateColumns: "1.4fr 1fr", gap: 48, alignItems: "center" }}>
          <Anim dir="left" style={{ background: "#111", borderRadius: 16, border: "1px solid rgba(255,255,255,0.05)", overflow: "hidden", boxShadow: "0 20px 60px rgba(0,0,0,0.5)" }}>
            <div style={{ padding: "10px 16px", borderBottom: "1px solid rgba(255,255,255,0.04)", display: "flex", gap: 12 }}>
              <span style={{ fontSize: 12, color: B, borderBottom: `2px solid ${B}`, paddingBottom: 8 }}>Query 1</span>
              <span style={{ fontSize: 12, color: "#555", paddingBottom: 8 }}>Query 2</span>
              <span style={{ fontSize: 12, color: "#555", paddingBottom: 8 }}>+ New</span>
            </div>
            <div style={{ padding: 16 }}>
              <div style={{ background: "#0a0a0a", borderRadius: 8, padding: "14px 16px", fontFamily: "'Fira Code', 'JetBrains Mono', monospace", fontSize: 12, lineHeight: 1.8, marginBottom: 12 }}>
                <span style={{ color: "#c678dd" }}>SELECT</span>{" "}
                <span style={{ color: "#e5c07b" }}>model_name</span>{", "}
                <span style={{ color: "#e5c07b" }}>provider</span>{", "}<br />
                {"       "}
                <span style={{ color: "#e5c07b" }}>arena_elo</span>{", "}
                <span style={{ color: "#e5c07b" }}>mmlu</span>{", "}
                <span style={{ color: "#e5c07b" }}>input_cost</span><br />
                <span style={{ color: "#c678dd" }}>FROM</span>{" "}
                <span style={{ color: "#98c379" }}>ai_benchmarks.leaderboard</span><br />
                <span style={{ color: "#c678dd" }}>WHERE</span>{" "}
                <span style={{ color: "#e5c07b" }}>arena_elo</span>{" "}
                <span style={{ color: "#c678dd" }}>IS NOT NULL</span><br />
                <span style={{ color: "#c678dd" }}>ORDER BY</span>{" "}
                <span style={{ color: "#e5c07b" }}>arena_elo</span>{" "}
                <span style={{ color: "#c678dd" }}>DESC</span><br />
                <span style={{ color: "#c678dd" }}>LIMIT</span>{" "}
                <span style={{ color: "#d19a66" }}>10</span>{";"}
              </div>
              <div style={{ fontSize: 10, color: "#555", marginBottom: 8, display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{ color: "#10b981" }}>&#x2713;</span> 22 rows &middot; 8ms &middot; cached
              </div>
              <div style={{ background: "#0a0a0a", borderRadius: 8, overflow: "hidden" }}>
                <div style={{ display: "grid", gridTemplateColumns: "1.5fr 1fr 1fr", fontSize: 10, borderBottom: "1px solid rgba(255,255,255,0.04)" }}>
                  <div style={{ padding: "6px 12px", color: "#666", fontWeight: 600 }}>model_name</div>
                  <div style={{ padding: "6px 12px", color: "#666", fontWeight: 600 }}>arena_elo</div>
                  <div style={{ padding: "6px 12px", color: "#666", fontWeight: 600 }}>input_cost</div>
                </div>
                {[["o3", "1402", "$10.00"], ["Gemini 2.5 Pro", "1388", "$1.25"], ["Claude Opus 4", "1380", "$15.00"], ["Grok 3", "1376", "$2.00"], ["DeepSeek R1", "1358", "$0.55"]].map(([c, v, p]) => (
                  <div key={c} style={{ display: "grid", gridTemplateColumns: "1.5fr 1fr 1fr", fontSize: 11, borderBottom: "1px solid rgba(255,255,255,0.02)" }}>
                    <div style={{ padding: "5px 12px", color: "#999" }}>{c}</div>
                    <div style={{ padding: "5px 12px", color: "#777" }}>{v}</div>
                    <div style={{ padding: "5px 12px", color: "#777" }}>{p}</div>
                  </div>
                ))}
              </div>
            </div>
          </Anim>
          <Anim dir="right" delay={200}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "3px", color: "#8b5cf6", marginBottom: 16 }}>SQL Lab</div>
            <h3 style={{ fontSize: 32, fontWeight: 700, lineHeight: 1.2, marginBottom: 16, letterSpacing: "-0.5px", color: "#e2e8f0" }}>VS Code in your browser</h3>
            <p style={{ fontSize: 15, color: "#666", lineHeight: 1.8, marginBottom: 24 }}>
              Monaco editor with SQL autocomplete, syntax highlighting, multi-tab sessions, query history, and result caching across supported connectors.
            </p>
            <Link href="/lab" style={{ fontSize: 14, color: B, textDecoration: "none", fontWeight: 500 }}>Open SQL Lab &rarr;</Link>
          </Anim>
        </div>
      </Section>

      {/* ─── Features Grid ─── */}
      <section id="features" ref={r4} style={{ padding: "100px 24px", background: "#0a0a0a" }}>
        <div style={{ maxWidth: 1100, margin: "0 auto" }}>
          <div style={{ textAlign: "center", marginBottom: 56 }}>
            <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "4px", color: B, marginBottom: 12 }}>Platform</div>
            <h2 style={{ fontSize: "clamp(28px, 4vw, 40px)", fontWeight: 700, letterSpacing: "-1px" }}>Everything you need</h2>
            <p style={{ fontSize: 16, color: "#718094", marginTop: 12 }}>One platform for governed data, deterministic reasoning, fast analytics, and reusable intelligence.</p>
          </div>
          <div className="about-grid-3" style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gridAutoRows: "auto", gap: 14 }}>
            <Anim dir="up" delay={0} style={{ gridColumn: "span 2" }}>
              <div className="about-card" style={{ padding: 40, borderRadius: 16, background: `linear-gradient(135deg, ${B}06 0%, transparent 100%)`, border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s", height: "100%" }}>
                <div style={{ fontSize: 11, fontWeight: 700, color: B, textTransform: "uppercase", letterSpacing: "2px", marginBottom: 12 }}>Core</div>
                <h3 style={{ fontSize: 24, fontWeight: 700, marginBottom: 12, lineHeight: 1.3 }}>Conversational Data Querying</h3>
                <p style={{ fontSize: 15, color: "#777", lineHeight: 1.8, maxWidth: 500 }}>
                  Type questions in plain English. A template-based NL&#x2192;SQL engine parses your words, matches schema metadata, generates SQL, and renders the answer as an interactive chart.
                </p>
              </div>
            </Anim>
            <Anim dir="up" delay={100}>
              <div className="about-card" style={{ padding: 36, borderRadius: 16, background: "linear-gradient(135deg, rgba(16,185,129,0.05) 0%, transparent 100%)", border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s", height: "100%" }}>
                <h3 style={{ fontSize: 18, fontWeight: 700, marginBottom: 10 }}>37 Chart Types</h3>
                <p style={{ fontSize: 13.5, color: "#777", lineHeight: 1.8 }}>Bar, line, pie, heatmap, treemap, scatter, funnel, gauge, world map, 3D globe. All dark-mode aware.</p>
              </div>
            </Anim>
            <Anim dir="up" delay={200}>
              <div className="about-card" style={{ padding: 36, borderRadius: 16, background: "linear-gradient(135deg, rgba(139,92,246,0.05) 0%, transparent 100%)", border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s", height: "100%" }}>
                <h3 style={{ fontSize: 18, fontWeight: 700, marginBottom: 10 }}>SQL Lab</h3>
                <p style={{ fontSize: 13.5, color: "#777", lineHeight: 1.8 }}>Monaco editor with autocomplete, multi-tab, query history, and caching.</p>
              </div>
            </Anim>
            <Anim dir="up" delay={300} style={{ gridColumn: "span 2" }}>
              <div className="about-card" style={{ padding: 40, borderRadius: 16, background: "linear-gradient(135deg, rgba(245,158,11,0.05) 0%, transparent 100%)", border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s", height: "100%" }}>
                <div style={{ fontSize: 11, fontWeight: 700, color: "#f59e0b", textTransform: "uppercase", letterSpacing: "2px", marginBottom: 12 }}>Build &amp; Share</div>
                <h3 style={{ fontSize: 24, fontWeight: 700, marginBottom: 12, lineHeight: 1.3 }}>Dashboards That Tell Stories</h3>
                <p style={{ fontSize: 15, color: "#777", lineHeight: 1.8, maxWidth: 500 }}>
                  Drag-and-drop canvas with cross-chart filtering, shared filter bar, auto-refresh, and one-click publishing.
                </p>
              </div>
            </Anim>
            {[
              { title: "Multi-Source", desc: "Register multiple supported sources; each query targets one selected source today.", color: "rgba(236,72,153,0.05)" },
              { title: "Semantic Datasets", desc: "Define dimensions, metrics, and joins once. Reuse everywhere.", color: "rgba(6,182,212,0.05)" },
              { title: "Governed Identity", desc: "Microsoft Entra ID, role-aware access, and customer-controlled data boundaries.", color: "rgba(99,102,241,0.05)" },
              { title: "Deploy Your Way", desc: "Run locally with Docker today; private-cloud and distributed topologies remain explicitly staged.", color: "rgba(245,158,11,0.05)" },
            ].map((f, idx) => (
              <Anim key={f.title} dir="up" delay={400 + idx * 100}>
                <div className="about-card" style={{ padding: 36, borderRadius: 16, background: `linear-gradient(135deg, ${f.color} 0%, transparent 100%)`, border: "1px solid rgba(255,255,255,0.05)", transition: "all 0.3s", height: "100%" }}>
                  <h3 style={{ fontSize: 18, fontWeight: 700, marginBottom: 10 }}>{f.title}</h3>
                  <p style={{ fontSize: 13.5, color: "#777", lineHeight: 1.8 }}>{f.desc}</p>
                </div>
              </Anim>
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
          <div className="about-tech" style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(130px, 1fr))", gap: 12 }}>
            {[
              { name: "Rust", version: "2024", color: "#f59e0b" },
              { name: "Apache Arrow", version: "54", color: B },
              { name: "Parquet", version: "Direct read", color: "#06b6d4" },
              { name: "Delta Lake", version: "Local alpha", color: "#10b981" },
              { name: "Next.js", version: "15", color: "#fff" },
              { name: "React", version: "19", color: "#61dafb" },
              { name: "TypeScript", version: "5.x", color: "#3178c6" },
              { name: "FastAPI", version: "0.115", color: "#009688" },
              { name: "Python", version: "3.12", color: "#ffd43b" },
              { name: "ECharts", version: "5.x", color: "#e43961" },
              { name: "PostgreSQL", version: "16", color: "#336791" },
              { name: "Azure", version: "Container Apps", color: "#0078d4" },
              { name: "Vercel", version: "Edge", color: "#fff" },
              { name: "Docker", version: "Local stack", color: "#2496ed" },
            ].map((t, idx) => (
              <Anim key={t.name} dir="up" delay={idx * 50}>
                <div className="about-card" style={{
                  padding: "20px 16px", borderRadius: 12,
                  background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.05)",
                  textAlign: "center", transition: "all 0.3s",
                }}>
                  <div style={{ fontSize: 15, fontWeight: 600, color: t.color, marginBottom: 4 }}>{t.name}</div>
                  <div style={{ fontSize: 11, color: "#444" }}>{t.version}</div>
                </div>
              </Anim>
            ))}
          </div>
        </div>
      </section>

      {/* ─── CTA ─── */}
      <section ref={r6} style={{ textAlign: "center", padding: "80px 24px 60px", position: "relative" }}>
        <Anim dir="scale">
          <div style={{ position: "relative" }}>
            <KaveonMark size={44} useDirectColor />
            <h2 style={{ fontSize: "clamp(28px, 4vw, 42px)", fontWeight: 700, letterSpacing: "-1px", margin: "20px 0 12px" }}>
              Ready to talk to your data?
            </h2>
            <p style={{ fontSize: 15, color: "#666", marginBottom: 36 }}>Open source &middot; Self-hosted &middot; MIT License</p>
            <Link href="/" className="about-btn" style={{ display: "inline-block", padding: "16px 44px", borderRadius: 12, background: B, color: "#fff", fontSize: 16, fontWeight: 600, textDecoration: "none", boxShadow: `0 4px 24px ${B}30`, transition: "all 0.2s" }}>
              Try Kaveon
            </Link>
          </div>
        </Anim>
      </section>

      {/* ─── Footer ─── */}
      <footer className="about-footer" style={{ borderTop: "1px solid rgba(255,255,255,0.06)", padding: "20px 48px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <KaveonMark size={16} useDirectColor />
          <span style={{ fontSize: 12, color: "#e2e8f0" }}>&copy; {new Date().getFullYear()} Kaveon</span>
        </div>
        <div style={{ display: "flex", gap: 24 }}>
          <Link href="/docs" className="about-link" style={{ fontSize: 12, color: "#e2e8f0", textDecoration: "none", transition: "color 0.2s" }}>Documentation</Link>
          <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" className="about-link" style={{ fontSize: 12, color: "#e2e8f0", textDecoration: "none", transition: "color 0.2s" }}>GitHub</a>
          <Link href="/" className="about-link" style={{ fontSize: 12, color: "#e2e8f0", textDecoration: "none", transition: "color 0.2s" }}>Launch App</Link>
        </div>
      </footer>
    </div>
  );
}
