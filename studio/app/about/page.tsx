"use client";

import Image from "next/image";
import Link from "next/link";
import { useState } from "react";
import { KaveonWordmark } from "../../components/KaveonMark";

const BLUE = "#4A9EE8";

const pillars = [
  { name: "Kaveon Studio", status: "Available today", tone: "live", description: "The intelligence surface for questions, SQL analysis, governed dashboards, and operational data exploration.", capabilities: ["Conversational analysis", "SQL Lab", "Dashboards and chart builder"] },
  { name: "Data Language Model", status: "Available today", tone: "live", description: "A deterministic semantic runtime that compiles dataset vocabulary and resolves supported questions without token prediction.", capabilities: ["Dataset context compilation", "Deterministic NL→SQL", "Freshness-aware resolution"] },
  { name: "Kaveon Engine", status: "Engine Alpha", tone: "alpha", description: "A vectorized Rust analytical engine designed to query lake data directly and evolve into a distributed execution substrate.", capabilities: ["Arrow batch execution", "Local Parquet reads", "Distributed lake execution — target"] },
];

const slides = [
  { image: "/showcase/climate-dark.png", title: "Climate × Energy", description: "Cross-domain climate and energy analysis" },
  { image: "/showcase/ai-arena-dark.png", title: "AI Model Arena", description: "Model rankings, benchmarks, and pricing" },
  { image: "/showcase/energy-dark.png", title: "Global Energy", description: "Energy mix, intensity, and emissions trends" },
  { image: "/showcase/taxi-dark.png", title: "NYC Taxi", description: "Geospatial, revenue, and trip analysis" },
];

const principles = [
  ["One governed surface", "Questions, SQL, visual analysis, and administration share one product model instead of separate tools."],
  ["Deterministic where it matters", "The DLM maps supported language to governed dataset semantics; optional hosted AI remains a separate, explicit capability."],
  ["Compute comes to the data", "The Engine direction is direct lake access. Local Parquet is implemented; cloud object stores and table formats remain roadmap work."],
  ["Evidence before claims", "Performance is published only with the dataset, hardware, version, cache state, concurrency, and query definition."],
];

export default function AboutPage() {
  const [activeSlide, setActiveSlide] = useState(0);
  const slide = slides[activeSlide];

  return (
    <div className="about-shell">
      <style>{`
        :root { color-scheme: dark; }
        .about-shell { min-height:100vh; overflow-x:hidden; color:#f8fafc; background:#0b0f15; font-family:Inter,ui-sans-serif,system-ui,sans-serif; }
        .about-nav { position:sticky; top:0; z-index:50; height:64px; display:flex; align-items:center; justify-content:space-between; padding:0 max(24px,calc((100vw - 1180px)/2)); border-bottom:1px solid #202a38; background:rgba(11,15,21,.86); backdrop-filter:blur(18px); }
        .nav-links,.hero-actions,.status-row,.slide-tabs { display:flex; align-items:center; gap:12px; }
        .nav-link { color:#9aa7b8; text-decoration:none; font-size:13px; font-weight:600; }
        .nav-link:hover,.nav-link:focus-visible { color:#f8fafc; }
        .button { display:inline-flex; align-items:center; justify-content:center; min-height:44px; padding:0 20px; border:1px solid #2a3748; border-radius:10px; color:#dbe7f5; background:#101620; text-decoration:none; font-size:14px; font-weight:700; transition:transform .18s,border-color .18s,background .18s; }
        .button:hover,.button:focus-visible { transform:translateY(-1px); border-color:#4A9EE8; }
        .button.primary { color:#07111d; border-color:${BLUE}; background:${BLUE}; }
        .hero { position:relative; padding:112px 24px 96px; text-align:center; border-bottom:1px solid #182231; background:radial-gradient(ellipse 70% 65% at 50% 0%,rgba(74,158,232,.16),transparent 68%); }
        .hero-inner,.section { max-width:1180px; margin:0 auto; }
        .hero-brand { display:inline-flex; padding:18px 24px; border:1px solid rgba(74,158,232,.2); border-radius:16px; background:rgba(16,22,32,.7); box-shadow:0 24px 80px rgba(0,0,0,.28); }
        .eyebrow { margin:0 0 14px; color:${BLUE}; font-size:12px; font-weight:800; letter-spacing:.16em; text-transform:uppercase; }
        h1 { max-width:980px; margin:34px auto 20px; font-size:clamp(46px,7vw,82px); line-height:1.02; letter-spacing:-.055em; font-weight:760; }
        .hero-copy { max-width:760px; margin:0 auto; color:#a8b5c5; font-size:clamp(17px,2vw,21px); line-height:1.65; }
        .hero-actions { justify-content:center; margin-top:34px; }
        .status-row { justify-content:center; flex-wrap:wrap; margin-top:42px; color:#8290a3; font-size:12px; }
        .status-key { display:flex; align-items:center; gap:7px; padding:7px 10px; }
        .dot { width:7px; height:7px; border-radius:50%; background:#4ade80; box-shadow:0 0 14px rgba(74,222,128,.5); }
        .dot.alpha { background:#fbbf24; box-shadow:0 0 14px rgba(251,191,36,.4); }
        .section-wrap { padding:96px 24px; }
        .section-wrap.alt { background:#0e141d; border-top:1px solid #182231; border-bottom:1px solid #182231; }
        .section-head { max-width:720px; margin-bottom:42px; }
        h2 { margin:0 0 14px; font-size:clamp(32px,4.5vw,52px); line-height:1.08; letter-spacing:-.04em; }
        .section-copy { margin:0; color:#96a4b6; font-size:17px; line-height:1.7; }
        .pillar-grid { display:grid; grid-template-columns:repeat(3,1fr); gap:16px; }
        .pillar { min-height:340px; padding:28px; border:1px solid #263244; border-radius:16px; background:#101620; box-shadow:0 18px 55px rgba(0,0,0,.16); }
        .pillar-top { display:flex; align-items:center; justify-content:space-between; gap:12px; margin-bottom:34px; }
        .pillar-index { color:#526176; font:700 12px ui-monospace,SFMono-Regular,monospace; }
        .badge { padding:6px 9px; border:1px solid rgba(74,222,128,.25); border-radius:999px; color:#86efac; background:rgba(74,222,128,.07); font-size:10px; font-weight:800; letter-spacing:.06em; text-transform:uppercase; }
        .badge.alpha { color:#fde68a; border-color:rgba(251,191,36,.25); background:rgba(251,191,36,.07); }
        .pillar h3 { margin:0 0 14px; font-size:24px; letter-spacing:-.025em; }
        .pillar p { min-height:94px; margin:0; color:#98a6b8; font-size:14px; line-height:1.7; }
        .capabilities { margin:24px 0 0; padding:20px 0 0; border-top:1px solid #202a38; list-style:none; }
        .capabilities li { display:flex; gap:9px; margin:9px 0; color:#c6d1df; font-size:13px; }
        .capabilities li::before { content:'—'; color:${BLUE}; }
        .flow { display:grid; grid-template-columns:1fr 56px 1fr 56px 1fr; align-items:stretch; }
        .flow-card { padding:24px; border:1px solid #263244; border-radius:14px; background:#101620; }
        .flow-card strong { display:block; margin-bottom:8px; font-size:17px; }
        .flow-card span { color:#8f9db0; font-size:13px; line-height:1.6; }
        .arrow { display:flex; align-items:center; justify-content:center; color:${BLUE}; font-size:22px; }
        .proof-frame { overflow:hidden; border:1px solid #263244; border-radius:18px; background:#101620; box-shadow:0 30px 90px rgba(0,0,0,.35); }
        .window-bar { height:42px; display:flex; align-items:center; gap:7px; padding:0 16px; border-bottom:1px solid #202a38; }
        .window-dot { width:9px; height:9px; border-radius:50%; background:#344155; }
        .proof-image { position:relative; aspect-ratio:16/9; background:#090d12; }
        .slide-tabs { justify-content:center; flex-wrap:wrap; margin-top:22px; }
        .slide-tab { padding:10px 14px; border:1px solid #263244; border-radius:10px; color:#8492a6; background:#101620; cursor:pointer; font:700 12px inherit; }
        .slide-tab[aria-selected='true'] { color:#f8fafc; border-color:${BLUE}; background:rgba(74,158,232,.1); }
        .slide-caption { margin-top:16px; text-align:center; color:#8796aa; font-size:14px; }
        .slide-caption strong { color:#e6edf6; }
        .principles { display:grid; grid-template-columns:repeat(2,1fr); gap:16px; }
        .principle { padding:25px; border-left:2px solid ${BLUE}; background:linear-gradient(90deg,rgba(74,158,232,.08),transparent); }
        .principle h3 { margin:0 0 8px; font-size:18px; }
        .principle p { margin:0; color:#96a4b6; font-size:14px; line-height:1.7; }
        .about-footer { display:flex; justify-content:space-between; align-items:center; gap:20px; max-width:1180px; margin:0 auto; padding:32px 24px; color:#718096; font-size:12px; }
        .footer-links { display:flex; gap:20px; }
        :focus-visible { outline:2px solid ${BLUE}; outline-offset:3px; }
        @media(max-width:820px){ .nav-links .nav-link{display:none}.hero{padding-top:84px}.pillar-grid,.principles{grid-template-columns:1fr}.flow{grid-template-columns:1fr}.arrow{height:48px;transform:rotate(90deg)}.pillar p{min-height:0}.about-footer{align-items:flex-start;flex-direction:column}.hero-actions{flex-wrap:wrap}.button{flex:1;min-width:150px} }
        @media(max-width:480px){ .about-nav{padding:0 16px}.section-wrap{padding:72px 18px}.hero{padding-left:18px;padding-right:18px}.hero-brand{padding:14px 16px}.status-row{align-items:flex-start;flex-direction:column}.slide-tab{flex:1 1 42%} }
        @media(prefers-reduced-motion:reduce){*{scroll-behavior:auto!important;transition:none!important}}
      `}</style>

      <nav className="about-nav" aria-label="About navigation">
        <Link href="/about" aria-label="Kaveon about"><KaveonWordmark height={24} /></Link>
        <div className="nav-links"><a className="nav-link" href="#platform">Platform</a><a className="nav-link" href="#architecture">Architecture</a><a className="nav-link" href="#studio">Studio</a><Link className="nav-link" href="/docs">Docs</Link><Link className="button primary" href="/">Open Studio</Link></div>
      </nav>

      <main>
        <section className="hero"><div className="hero-inner">
          <div className="hero-brand"><KaveonWordmark height={44} /></div>
          <h1>The unified data intelligence platform.</h1>
          <p className="hero-copy">One governed system for analytical execution, deterministic data language, and interactive intelligence—designed to bring questions to data without turning every answer into a model call.</p>
          <div className="hero-actions"><Link className="button primary" href="/">Explore Studio</Link><Link className="button" href="/docs">Read the architecture</Link><a className="button" href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer">View source</a></div>
          <div className="status-row" aria-label="Component maturity"><span className="status-key"><span className="dot" />Studio available</span><span className="status-key"><span className="dot" />DLM available</span><span className="status-key"><span className="dot alpha" />Engine alpha</span></div>
        </div></section>

        <section id="platform" className="section-wrap"><div className="section">
          <div className="section-head"><p className="eyebrow">Three pillars · one platform</p><h2>Built as a system, not a bundle.</h2><p className="section-copy">Studio, DLM, and Engine have distinct responsibilities. Their contracts converge into one platform while each remains independently useful.</p></div>
          <div className="pillar-grid">{pillars.map((pillar, index) => <article className="pillar" key={pillar.name}><div className="pillar-top"><span className="pillar-index">0{index + 1}</span><span className={`badge ${pillar.tone}`}>{pillar.status}</span></div><h3>{pillar.name}</h3><p>{pillar.description}</p><ul className="capabilities">{pillar.capabilities.map(item => <li key={item}>{item}</li>)}</ul></article>)}</div>
        </div></section>

        <section id="architecture" className="section-wrap alt"><div className="section">
          <div className="section-head"><p className="eyebrow">Request architecture</p><h2>Two governed paths to an answer.</h2><p className="section-copy">Natural-language requests use the deterministic DLM. SQL and BI workloads address analytical execution directly. Studio remains the common surface for both.</p></div>
          <div className="flow" aria-label="Kaveon request flow"><div className="flow-card"><strong>Studio request</strong><span>A question, SQL statement, dashboard interaction, or API request enters with identity and dataset context.</span></div><div className="arrow" aria-hidden="true">→</div><div className="flow-card"><strong>DLM or direct SQL</strong><span>Questions resolve against governed semantics. Explicit SQL follows the direct analytical path.</span></div><div className="arrow" aria-hidden="true">→</div><div className="flow-card"><strong>Answer in Studio</strong><span>Results return as explainable data, tables, charts, and reusable analytical artifacts.</span></div></div>
        </div></section>

        <section id="studio" className="section-wrap"><div className="section">
          <div className="section-head"><p className="eyebrow">Product surface</p><h2>Intelligence you can inspect.</h2><p className="section-copy">Kaveon Studio connects conversational exploration with the precision of SQL and the repeatability of governed dashboards.</p></div>
          <div className="proof-frame"><div className="window-bar" aria-hidden="true"><span className="window-dot" /><span className="window-dot" /><span className="window-dot" /></div><div className="proof-image"><Image key={slide.image} src={slide.image} alt={`${slide.title} dashboard in Kaveon Studio`} fill sizes="(max-width: 1180px) 100vw, 1180px" style={{ objectFit: "cover" }} priority={activeSlide === 0} /></div></div>
          <div className="slide-tabs" role="tablist" aria-label="Dashboard examples">{slides.map((item, index) => <button className="slide-tab" role="tab" aria-selected={activeSlide === index} key={item.title} onClick={() => setActiveSlide(index)}>{item.title}</button>)}</div>
          <p className="slide-caption"><strong>{slide.title}</strong> — {slide.description}</p>
        </div></section>

        <section className="section-wrap alt"><div className="section"><div className="section-head"><p className="eyebrow">Engineering principles</p><h2>Credibility is part of the architecture.</h2></div><div className="principles">{principles.map(([title, copy]) => <article className="principle" key={title}><h3>{title}</h3><p>{copy}</p></article>)}</div></div></section>
        <section className="section-wrap"><div className="section" style={{ textAlign: "center" }}><p className="eyebrow">Talk to your data.</p><h2 style={{ maxWidth: 760, marginLeft: "auto", marginRight: "auto" }}>Start with Studio. Follow the architecture as it becomes one platform.</h2><div className="hero-actions"><Link className="button primary" href="/">Open Studio</Link><Link className="button" href="/docs">Read the docs</Link></div></div></section>
      </main>

      <footer className="about-footer"><span>© {new Date().getFullYear()} Kaveon · MIT licensed</span><div className="footer-links"><Link className="nav-link" href="/docs">Documentation</Link><a className="nav-link" href="https://github.com/PruthviProdduturi/Kaveon">GitHub</a><Link className="nav-link" href="/docs/quickstart">Quickstart</Link></div></footer>
    </div>
  );
}
