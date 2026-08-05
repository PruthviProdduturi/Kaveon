"use client";

import React from "react";
import Link from "next/link";
import { LensLogo } from "../../components/LensLogo";
import { useTheme } from "../../contexts/ThemeContext";

const CYAN = "#0078D4"; // Default Microsoft blue — matches Forge

// ─── Layout primitives ───────────────────────────────────────────────────────

function Section({
  id,
  children,
  bg = "#fff",
  style,
}: {
  id?: string;
  children: React.ReactNode;
  bg?: string;
  style?: React.CSSProperties;
}) {
  return (
    <section
      id={id}
      style={{
        background: bg,
        padding: "52px 0",
        ...style,
      }}
    >
      <div style={{ maxWidth: 1400, margin: "0 auto", padding: "0 1.5rem" }}>
        {children}
      </div>
    </section>
  );
}

function SectionLabel({
  text,
  color = CYAN,
  light = false,
}: {
  text: string;
  color?: string;
  light?: boolean;
}) {
  return (
    <div
      style={{
        display: "inline-block",
        padding: "4px 14px",
        borderRadius: 20,
        background: light ? "rgba(255,255,255,0.15)" : `${color}18`,
        border: `1px solid ${light ? "rgba(255,255,255,0.25)" : `${color}30`}`,
        fontSize: 11,
        fontWeight: 700,
        letterSpacing: "0.09em",
        textTransform: "uppercase",
        color: light ? "#e8f0fb" : color,
        marginBottom: 10,
      }}
    >
      {text}
    </div>
  );
}

function SectionHeading({
  children,
  center = false,
  light = false,
}: {
  children: React.ReactNode;
  center?: boolean;
  light?: boolean;
}) {
  return (
    <h2
      style={{
        fontSize: "clamp(1.6rem, 3vw, 2.25rem)",
        fontWeight: 800,
        color: light ? "#fff" : "#0f172a",
        lineHeight: 1.2,
        letterSpacing: "-0.02em",
        textAlign: center ? "center" : undefined,
        marginBottom: 8,
      }}
    >
      {children}
    </h2>
  );
}

function SectionSub({
  children,
  center = false,
  light = false,
}: {
  children: React.ReactNode;
  center?: boolean;
  light?: boolean;
}) {
  return (
    <p
      style={{
        fontSize: "1rem",
        color: light ? "rgba(255,255,255,0.75)" : "#475569",
        lineHeight: 1.75,
        maxWidth: center ? 560 : 640,
        margin: center ? "0 auto" : undefined,
        marginBottom: 0,
      }}
    >
      {children}
    </p>
  );
}

function FeatureCard({
  icon,
  color,
  title,
  body,
}: {
  icon: string;
  color: string;
  title: string;
  body: string;
}) {
  return (
    <div
      style={{
        background: "rgba(255,255,255,0.85)",
        backdropFilter: "blur(10px)",
        border: "1px solid rgba(30,58,95,0.08)",
        borderRadius: 12,
        padding: "1.375rem 1.25rem",
        display: "flex",
        flexDirection: "column",
        gap: 10,
        transition: "box-shadow 0.2s",
        boxShadow: "0 2px 12px rgba(30,58,95,0.05)",
      }}
    >
      <div
        style={{
          width: 40,
          height: 40,
          borderRadius: 10,
          background: `${color}18`,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          flexShrink: 0,
        }}
      >
        <i className={`fas ${icon}`} style={{ color, fontSize: 17 }} aria-hidden="true" />
      </div>
      <div>
        <div
          style={{
            fontSize: 14,
            fontWeight: 700,
            color: "#0f172a",
            marginBottom: 4,
          }}
        >
          {title}
        </div>
        <div style={{ fontSize: 13.5, color: "#64748b", lineHeight: 1.65 }}>
          {body}
        </div>
      </div>
    </div>
  );
}

function StepCard({
  number,
  title,
  code,
}: {
  number: number;
  title: string;
  code: string;
}) {
  return (
    <div
      style={{
        background: "rgba(255,255,255,0.1)",
        backdropFilter: "blur(10px)",
        border: "1px solid rgba(255,255,255,0.18)",
        borderRadius: 12,
        padding: "1.5rem",
        flex: 1,
        minWidth: 0,
      }}
    >
      <div
        style={{
          width: 32,
          height: 32,
          borderRadius: "50%",
          background: "rgba(255,255,255,0.2)",
          color: "#fff",
          fontWeight: 800,
          fontSize: 15,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          marginBottom: 12,
          flexShrink: 0,
        }}
      >
        {number}
      </div>
      <div
        style={{
          fontSize: 15,
          fontWeight: 700,
          color: "#fff",
          marginBottom: 10,
        }}
      >
        {title}
      </div>
      <pre
        style={{
          background: "rgba(0,0,0,0.35)",
          borderRadius: 8,
          padding: "0.75rem 1rem",
          fontSize: 12,
          fontFamily: "'Cascadia Code', 'Fira Code', Consolas, monospace",
          color: "#86efac",
          overflowX: "auto",
          whiteSpace: "pre-wrap",
          wordBreak: "break-all",
          margin: 0,
          lineHeight: 1.6,
        }}
      >
        {code}
      </pre>
    </div>
  );
}

// ─── Data ────────────────────────────────────────────────────────────────────

const FEATURES = [
  { icon: "fa-flask", color: CYAN, title: "SQL Lab", body: "Monaco editor with multi-tab, query history, async execution, and result caching. Write SQL against any connected source." },
  { icon: "fa-wand-magic-sparkles", color: "#8b5cf6", title: "AI, built in", body: "Natural-language to SQL via Anthropic Claude, OpenAI GPT, or GitHub Models. Bring your own key; no data leaves your network except the prompt." },
  { icon: "fa-chart-column", color: "#f59e0b", title: "20+ chart types", body: "Lines, bars, pies, heatmaps, treemaps, scatter plots, 3D WebGL globe, and cross-filtering between charts." },
  { icon: "fa-table-columns", color: "#059669", title: "Dashboards", body: "Drag-and-drop grid layout with rows, tabs, text blocks, and shared filters that slice every chart at once." },
  { icon: "fa-database", color: "#0078d4", title: "Multi-source", body: "Microsoft Fabric, Azure SQL, PostgreSQL, MySQL live today. Trino, StarRocks, Snowflake, and BigQuery on the roadmap." },
  { icon: "fa-shield-halved", color: "#dc2626", title: "Enterprise auth", body: "GitHub, Google, and Microsoft sign-in with 4-tier RBAC and per-object visibility controls." },
];

const JOURNEYS = [
  {
    num: 1, title: "Run a SQL Query", color: CYAN,
    desc: "From SQL Lab to paginated results in seconds.",
    steps: [
      { text: "Open SQL Lab from the sidebar" },
      { text: "Select a data source from the dropdown" },
      { text: "Write or paste your query in Monaco" },
      { text: "Hit Run — results stream into a paginated table" },
    ],
  },
  {
    num: 2, title: "Build a Chart", color: "#f59e0b",
    desc: "Pick a dataset, choose a chart type, drag metrics and dimensions, save.",
    steps: [
      { text: "Pick a dataset or save a query result as one" },
      { text: "Choose a chart type — bar, line, pie, heatmap, globe..." },
      { text: "Drag metrics and dimensions into the configuration panel" },
      { text: "Customize colors, labels, and axes — then save" },
    ],
  },
  {
    num: 3, title: "Ask AI a Question", color: "#8b5cf6",
    desc: "Type a plain-English question, get SQL, review, and execute.",
    steps: [
      { text: "Click the AI icon in SQL Lab" },
      { text: "Type a question in plain English" },
      { text: "The AI generates a SQL query against your schema" },
      { text: "Review, edit if needed, and execute" },
    ],
  },
  {
    num: 4, title: "Build a Dashboard", color: "#059669",
    desc: "Assemble charts into a shareable dashboard with shared filters.",
    steps: [
      { text: "Create a new dashboard from the Dashboards page" },
      { text: "Drag saved charts onto a flexible grid layout" },
      { text: "Add shared filters that slice every chart at once" },
      { text: "Publish and share the URL with your team" },
    ],
  },
];

const DBS = [
  { name: "Microsoft Fabric", icon: "fa-database", status: "live" as const },
  { name: "Azure SQL", icon: "fa-database", status: "live" as const },
  { name: "PostgreSQL", icon: "fa-database", status: "live" as const },
  { name: "MySQL", icon: "fa-database", status: "live" as const },
  { name: "Trino", icon: "fa-table-columns", status: "soon" as const },
  { name: "StarRocks", icon: "fa-bolt", status: "soon" as const },
  { name: "Snowflake", icon: "fa-snowflake", status: "planned" as const },
  { name: "BigQuery", icon: "fa-cloud", status: "planned" as const },
];

const COMPARE_ROWS: [string, boolean | "~", boolean | "~", boolean | "~", boolean | "~"][] = [
  ["Modern, fast UI", true, "~", true, "~"],
  ["AI natural-language to SQL", true, false, true, false],
  ["Built-in SQL Lab", true, "~", false, true],
  ["Self-hosted + open source (MIT)", true, true, false, true],
  ["20+ chart types incl. 3D globe", true, "~", true, "~"],
  ["Microsoft Fabric-native", true, "~", true, false],
];

const ROLES = [
  {
    role: "Viewer", color: "#94a3b8", icon: "fa-eye",
    perms: ["View published dashboards", "View published charts", "Browse datasets (read-only)", "Export chart data"],
  },
  {
    role: "Analyst", color: "#60a5fa", icon: "fa-flask",
    perms: ["Everything in Viewer", "SQL Lab — write and execute queries", "Create and save charts", "Create datasets from queries"],
  },
  {
    role: "Editor", color: "#fbbf24", icon: "fa-pen-to-square",
    perms: ["Everything in Analyst", "Publish dashboards", "Manage dataset definitions", "Edit shared filters and layouts"],
  },
  {
    role: "Admin", color: "#f87171", icon: "fa-shield-halved",
    perms: ["Full platform access", "Add/remove data sources", "Manage users and roles", "Auth config, system settings"],
  },
];

// ─── About page ──────────────────────────────────────────────────────────────

export default function AboutPage() {
  const { primaryColor } = useTheme();
  const color = primaryColor; // theme-aware accent for hero + nav

  return (
    <div style={{ minHeight: "100%" }}>

      {/* ── Sticky section nav ──────────────────────────────────────────────── */}
      <nav
        aria-label="Page sections"
        style={{
          position: "sticky",
          top: 0,
          zIndex: 50,
          background: "var(--lens-light, #ecfeff)",
          borderBottom: "1px solid rgba(30,58,95,0.08)",
          padding: "0 1.5rem",
          display: "flex",
          justifyContent: "center",
          gap: 4,
          overflowX: "auto",
        }}
      >
        {[
          { href: "#architecture", label: "Architecture" },
          { href: "#features", label: "Features" },
          { href: "#journeys", label: "Journeys" },
          { href: "#databases", label: "Databases" },
          { href: "#compare", label: "Compare" },
          { href: "#security", label: "Security" },
          { href: "#start", label: "Get Started" },
        ].map(({ href, label }) => (
          <a
            key={href}
            href={href}
            style={{
              display: "inline-flex",
              alignItems: "center",
              padding: "10px 14px",
              fontSize: 13,
              fontWeight: 500,
              color: "#64748b",
              textDecoration: "none",
              whiteSpace: "nowrap",
              borderBottom: "2px solid transparent",
              transition: "color 0.15s, border-color 0.15s",
            }}
            onMouseEnter={(e) => {
              (e.currentTarget as HTMLAnchorElement).style.color = color;
              (e.currentTarget as HTMLAnchorElement).style.borderBottomColor = color;
            }}
            onMouseLeave={(e) => {
              (e.currentTarget as HTMLAnchorElement).style.color = "#64748b";
              (e.currentTarget as HTMLAnchorElement).style.borderBottomColor = "transparent";
            }}
          >
            {label}
          </a>
        ))}
      </nav>

      {/* ── Hero ────────────────────────────────────────────────────────────── */}
      <section
        style={{
          background: `linear-gradient(135deg, ${color} 0%, #0f1e2e 100%)`,
          padding: "56px 1.5rem 48px",
          textAlign: "center",
        }}
      >
        <div style={{ maxWidth: 760, margin: "0 auto" }}>
          <div style={{ display: "flex", justifyContent: "center", marginBottom: 10 }}>
            <LensLogo size={80} animate="none" />
          </div>
          <h1
            style={{
              fontSize: "clamp(1.1rem, 3vw, 1.5rem)",
              fontWeight: 700,
              color: "rgba(255,255,255,0.92)",
              marginBottom: 4,
              letterSpacing: "-0.01em",
            }}
          >
            The Analytics Platform
          </h1>
          <p
            style={{
              fontSize: "1rem",
              color: "rgba(255,255,255,0.65)",
              marginBottom: 24,
              lineHeight: 1.5,
            }}
          >
            See the pattern. Query live data, build charts, assemble dashboards.
          </p>
          <div
            style={{
              display: "flex",
              gap: 12,
              justifyContent: "center",
              flexWrap: "wrap",
            }}
          >
            <Link
              href="/"
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 8,
                padding: "12px 24px",
                borderRadius: 8,
                background: "#fff",
                color: CYAN,
                fontWeight: 700,
                fontSize: 14.5,
                textDecoration: "none",
                transition: "opacity 0.15s",
              }}
              onMouseEnter={(e) => ((e.currentTarget as HTMLAnchorElement).style.opacity = "0.9")}
              onMouseLeave={(e) => ((e.currentTarget as HTMLAnchorElement).style.opacity = "1")}
            >
              <i className="fas fa-rocket" aria-hidden="true" />
              Get Started
            </Link>
            <a
              href="https://github.com/PruthviProdduturi/Lens"
              target="_blank"
              rel="noreferrer"
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 8,
                padding: "12px 24px",
                borderRadius: 8,
                background: "rgba(255,255,255,0.12)",
                border: "1.5px solid rgba(255,255,255,0.3)",
                color: "#fff",
                fontWeight: 700,
                fontSize: 14.5,
                textDecoration: "none",
                transition: "background 0.15s",
              }}
              onMouseEnter={(e) => ((e.currentTarget as HTMLAnchorElement).style.background = "rgba(255,255,255,0.2)")}
              onMouseLeave={(e) => ((e.currentTarget as HTMLAnchorElement).style.background = "rgba(255,255,255,0.12)")}
            >
              <i className="fab fa-github" aria-hidden="true" />
              View on GitHub
            </a>
          </div>
        </div>
      </section>

      {/* ── Architecture ────────────────────────────────────────────────────── */}
      <Section
        id="architecture"
        bg="linear-gradient(135deg, #f8faff 0%, #eef2f7 100%)"
      >
        <div style={{ textAlign: "center", marginBottom: 28 }}>
          <SectionLabel text="Architecture" color={CYAN} />
          <SectionHeading center>Platform Architecture</SectionHeading>
          <SectionSub center>
            Two processes, zero vendor lock-in, your infrastructure. A Next.js frontend talks to a FastAPI backend over REST/JWT.
          </SectionSub>
        </div>

        {/* Architecture diagram */}
        <div style={{ background: "#fff", border: "1px solid #e2e8f0", borderRadius: 14, overflow: "hidden", marginBottom: 28, boxShadow: "0 4px 24px rgba(15,23,42,0.07)" }}>
          <div style={{ padding: 22 }}>
            <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>

              {/* Tier: Browser */}
              <div style={{ display: "flex", justifyContent: "center" }}>
                <div style={{ background: "#fff", border: "1px solid #e2e8f0", borderRadius: 8, padding: "7px 9px", display: "flex", alignItems: "center", gap: 8, boxShadow: "0 1px 2px rgba(0,0,0,.05)", minWidth: 220 }}>
                  <div style={{ width: 26, height: 26, borderRadius: 6, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0, background: "#64748b15" }}>
                    <i className="fas fa-globe" style={{ color: "#64748b", fontSize: 11 }} />
                  </div>
                  <div>
                    <div style={{ fontSize: 11, fontWeight: 600, color: "#1e293b" }}>Browser</div>
                    <div style={{ fontSize: 9, color: "#94a3b8", fontFamily: "monospace", marginTop: 1 }}>Any modern browser</div>
                  </div>
                </div>
              </div>

              {/* Connector */}
              <div style={{ display: "flex", justifyContent: "center", alignItems: "center", gap: 6 }}>
                <svg width="44" height="28" viewBox="0 0 44 28" style={{ transform: "rotate(90deg)" }}>
                  <line x1="2" y1="14" x2="34" y2="14" stroke="#94a3b8" strokeWidth="1.5" strokeDasharray="3,2" />
                  <polygon points="30,10 42,14 30,18" fill="#94a3b8" />
                </svg>
                <span style={{ fontSize: 9, fontWeight: 700, padding: "2px 8px", borderRadius: 4, background: `${CYAN}12`, color: CYAN, fontFamily: "monospace" }}>HTTPS</span>
              </div>

              {/* Tier: lens-web */}
              <div style={{ display: "flex", justifyContent: "center" }}>
                <div style={{ background: "#fff", border: `2px solid ${CYAN}40`, borderTop: `4px solid ${CYAN}`, borderRadius: 8, padding: "7px 9px", display: "flex", alignItems: "center", gap: 8, boxShadow: "0 1px 2px rgba(0,0,0,.05)", minWidth: 340 }}>
                  <div style={{ width: 26, height: 26, borderRadius: 6, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0, background: `${CYAN}15` }}>
                    <i className="fas fa-layer-group" style={{ color: CYAN, fontSize: 11 }} />
                  </div>
                  <div style={{ flex: 1 }}>
                    <div style={{ fontSize: 11, fontWeight: 600, color: "#1e293b" }}>lens-web</div>
                    <div style={{ fontSize: 9, color: "#94a3b8", fontFamily: "monospace", marginTop: 1 }}>Next.js 15 · React 19 · ECharts · Monaco</div>
                  </div>
                  <span style={{ fontSize: 8, fontWeight: 700, padding: "2px 5px", borderRadius: 4, background: "#dcfce7", color: "#15803d" }}>:3000</span>
                </div>
              </div>

              {/* Connector */}
              <div style={{ display: "flex", justifyContent: "center", alignItems: "center", gap: 6 }}>
                <svg width="44" height="28" viewBox="0 0 44 28" style={{ transform: "rotate(90deg)" }}>
                  <line x1="2" y1="14" x2="34" y2="14" stroke="#94a3b8" strokeWidth="1.5" strokeDasharray="3,2" />
                  <polygon points="30,10 42,14 30,18" fill="#94a3b8" />
                </svg>
                <span style={{ fontSize: 9, fontWeight: 700, padding: "2px 8px", borderRadius: 4, background: `${CYAN}12`, color: CYAN, fontFamily: "monospace" }}>REST / JWT</span>
              </div>

              {/* Tier: lens-api */}
              <div style={{ display: "flex", justifyContent: "center" }}>
                <div style={{ background: "#fff", border: `2px solid ${CYAN}40`, borderTop: `4px solid ${CYAN}`, borderRadius: 8, padding: "7px 9px", display: "flex", alignItems: "center", gap: 8, boxShadow: "0 1px 2px rgba(0,0,0,.05)", minWidth: 340 }}>
                  <div style={{ width: 26, height: 26, borderRadius: 6, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0, background: `${CYAN}15` }}>
                    <i className="fas fa-server" style={{ color: CYAN, fontSize: 11 }} />
                  </div>
                  <div style={{ flex: 1 }}>
                    <div style={{ fontSize: 11, fontWeight: 600, color: "#1e293b" }}>lens-api</div>
                    <div style={{ fontSize: 9, color: "#94a3b8", fontFamily: "monospace", marginTop: 1 }}>FastAPI · Python 3.11 · async drivers</div>
                  </div>
                  <span style={{ fontSize: 8, fontWeight: 700, padding: "2px 5px", borderRadius: 4, background: "#dcfce7", color: "#15803d" }}>:8000</span>
                </div>
              </div>

              {/* Connector */}
              <div style={{ display: "flex", justifyContent: "center", alignItems: "center", gap: 6 }}>
                <svg width="44" height="28" viewBox="0 0 44 28" style={{ transform: "rotate(90deg)" }}>
                  <line x1="2" y1="14" x2="34" y2="14" stroke="#94a3b8" strokeWidth="1.5" strokeDasharray="3,2" />
                  <polygon points="30,10 42,14 30,18" fill="#94a3b8" />
                </svg>
                <span style={{ fontSize: 9, fontWeight: 700, padding: "2px 8px", borderRadius: 4, background: `${CYAN}12`, color: CYAN, fontFamily: "monospace" }}>SQL / ODBC</span>
              </div>

              {/* Tier: Data Sources */}
              <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))", gap: 8 }}>
                {[
                  { name: "Metadata DB", meta: "App data, users, dashboards", icon: "fa-gear", color: "#64748b" },
                  { name: "Fabric SQL", meta: "Lakehouse endpoints", icon: "fa-database", color: "#0078d4" },
                  { name: "Azure SQL", meta: "Database or Managed Instance", icon: "fa-database", color: "#0078d4" },
                  { name: "PostgreSQL", meta: "PostgreSQL 12+", icon: "fa-database", color: "#336791" },
                  { name: "MySQL", meta: "MySQL 8+ / MariaDB", icon: "fa-database", color: "#00758f" },
                ].map(s => (
                  <div key={s.name} style={{ background: "#fff", border: "1px solid #e2e8f0", borderRadius: 8, padding: "7px 9px", display: "flex", alignItems: "center", gap: 8, boxShadow: "0 1px 2px rgba(0,0,0,.05)" }}>
                    <div style={{ width: 26, height: 26, borderRadius: 6, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0, background: `${s.color}15` }}>
                      <i className={`fas ${s.icon}`} style={{ color: s.color, fontSize: 11 }} />
                    </div>
                    <div>
                      <div style={{ fontSize: 11, fontWeight: 600, color: "#1e293b" }}>{s.name}</div>
                      <div style={{ fontSize: 9, color: "#94a3b8", fontFamily: "monospace", marginTop: 1 }}>{s.meta}</div>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </div>

        {/* AI providers note */}
        <div style={{
          padding: "12px 18px",
          background: "rgba(30,58,95,0.04)",
          borderRadius: 10,
          border: "1px solid rgba(30,58,95,0.08)",
          display: "flex",
          gap: 10,
          alignItems: "flex-start",
        }}>
          <i className="fas fa-wand-magic-sparkles" style={{ color: CYAN, fontSize: 13, flexShrink: 0, marginTop: 1 }} />
          <span style={{ fontSize: 13, color: "#475569", lineHeight: 1.6 }}>
            <strong>AI providers</strong> — the API proxies natural-language queries to
            Anthropic Claude, OpenAI GPT, or GitHub Models. Bring your own key; no data leaves your network except the prompt.
          </span>
        </div>
      </Section>

      {/* ── Features ────────────────────────────────────────────────────────── */}
      <Section
        id="features"
        bg="linear-gradient(135deg, #f0f6ff 0%, #eef2f7 100%)"
      >
        <div style={{ textAlign: "center", marginBottom: 28 }}>
          <SectionLabel text="Capabilities" color={CYAN} />
          <SectionHeading center>Everything to explore, visualize, and share</SectionHeading>
          <SectionSub center>
            A modern command center for your data — not a dashboard bolted onto a warehouse.
          </SectionSub>
        </div>

        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))",
            gap: 16,
          }}
        >
          {FEATURES.map((f) => (
            <FeatureCard
              key={f.title}
              icon={f.icon}
              color={f.color}
              title={f.title}
              body={f.body}
            />
          ))}
        </div>
      </Section>

      {/* ── User Journeys ───────────────────────────────────────────────────── */}
      <Section id="journeys" bg="#fff">
        <div style={{ marginBottom: 40 }}>
          <SectionLabel text="User Journeys" color={CYAN} />
          <SectionHeading>End-to-end flows</SectionHeading>
          <SectionSub>From raw SQL to published dashboards — four flows, four minutes.</SectionSub>
        </div>

        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(480px, 1fr))", gap: 18 }}>
          {JOURNEYS.map((flow) => (
            <div key={flow.num} style={{ border: "1px solid #e2e8f0", borderRadius: 12, overflow: "hidden", display: "flex", flexDirection: "column" }}>
              <div style={{
                padding: "12px 16px", borderBottom: "1px solid #e2e8f0",
                background: `${flow.color}08`, display: "flex", alignItems: "flex-start", gap: 10,
              }}>
                <div style={{
                  width: 26, height: 26, borderRadius: "50%", background: flow.color,
                  color: "#fff", fontWeight: 800, fontSize: 12,
                  display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0, marginTop: 1,
                }}>
                  {flow.num}
                </div>
                <div>
                  <div style={{ fontSize: 13, fontWeight: 700, color: flow.color }}>{flow.title}</div>
                  <div style={{ fontSize: 11, color: "#64748b", marginTop: 3 }}>{flow.desc}</div>
                </div>
              </div>
              <div style={{ padding: 16, flex: 1 }}>
                <div style={{ display: "flex", flexDirection: "column", gap: 0 }}>
                  {flow.steps.map((step, i) => (
                    <div key={i} style={{ display: "flex", gap: 11, alignItems: "flex-start", position: "relative", marginTop: i > 0 ? 8 : 0 }}>
                      {i < flow.steps.length - 1 && (
                        <div style={{ position: "absolute", left: 13, top: 28, bottom: -4, width: 2, background: "#e2e8f0" }} />
                      )}
                      <div style={{
                        width: 26, height: 26, borderRadius: "50%", border: `2px solid ${flow.color}`,
                        background: "#fff", display: "flex", alignItems: "center", justifyContent: "center",
                        fontSize: 10, fontWeight: 700, color: flow.color, flexShrink: 0, zIndex: 1,
                      }}>
                        {i + 1}
                      </div>
                      <div style={{ paddingBottom: 4, flex: 1 }}>
                        <div style={{ fontSize: 12, fontWeight: 600, color: "#0f172a" }}>{step.text}</div>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            </div>
          ))}
        </div>
      </Section>

      {/* ── Databases ───────────────────────────────────────────────────────── */}
      <Section id="databases" bg="#f8fafc">
        <div style={{ textAlign: "center", marginBottom: 28 }}>
          <SectionLabel text="Connectors" color={CYAN} />
          <SectionHeading center>Connect to anything</SectionHeading>
          <SectionSub center>
            One platform, every database. More connectors shipping fast.
          </SectionSub>
        </div>

        <div style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
          gap: 12,
          maxWidth: 960,
          margin: "0 auto",
        }}>
          {DBS.map((db) => (
            <div key={db.name} style={{
              background: "rgba(255,255,255,0.85)",
              backdropFilter: "blur(10px)",
              border: "1px solid rgba(30,58,95,0.08)",
              borderRadius: 12,
              padding: "14px 16px",
              display: "flex",
              alignItems: "center",
              gap: 10,
              boxShadow: "0 2px 12px rgba(30,58,95,0.05)",
            }}>
              <div style={{
                width: 36, height: 36, borderRadius: 9,
                background: db.status === "live" ? `${CYAN}18` : "#f1f5f918",
                display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0,
              }}>
                <i className={`fas ${db.icon}`} style={{ color: db.status === "live" ? CYAN : "#94a3b8", fontSize: 15 }} />
              </div>
              <div style={{ flex: 1 }}>
                <div style={{ fontSize: 14, fontWeight: 700, color: "#0f172a" }}>{db.name}</div>
              </div>
              <span style={{
                fontSize: 9, fontWeight: 700, padding: "3px 8px", borderRadius: 6, textTransform: "uppercase",
                ...(db.status === "live" ? {
                  color: "#15803d", background: "#dcfce7", border: "1px solid #bbf7d0",
                } : db.status === "soon" ? {
                  color: "#92400e", background: "#fef3c7", border: "1px solid #fde68a",
                } : {
                  color: "#64748b", background: "#f1f5f9", border: "1px solid #e2e8f0",
                }),
              }}>
                {db.status === "live" ? "Live" : db.status === "soon" ? "Soon" : "Planned"}
              </span>
            </div>
          ))}
        </div>
      </Section>

      {/* ── Compare ─────────────────────────────────────────────────────────── */}
      <Section id="compare" bg="#fff">
        <div style={{ textAlign: "center", marginBottom: 28 }}>
          <SectionLabel text="Compare" color={CYAN} />
          <SectionHeading center>How Lens compares</SectionHeading>
          <SectionSub center>
            An honest look next to Superset, Power BI, and Redash.
          </SectionSub>
        </div>

        <div style={{ overflowX: "auto", maxWidth: 900, margin: "0 auto" }}>
          <div style={{ borderRadius: 12, border: "1px solid #e2e8f0", background: "#fff", overflow: "hidden" }}>
            <table style={{ width: "100%", borderCollapse: "collapse", minWidth: 600 }}>
              <thead>
                <tr style={{ background: "#f8fafc" }}>
                  <th style={{ padding: "10px 16px", fontSize: 12.5, color: "#475569", fontWeight: 700, textAlign: "left" }}>Capability</th>
                  <th style={{ padding: "10px 10px", fontSize: 12.5, color: CYAN, fontWeight: 700, textAlign: "center" }}>Lens</th>
                  <th style={{ padding: "10px 10px", fontSize: 12.5, color: "#475569", fontWeight: 700, textAlign: "center" }}>Superset</th>
                  <th style={{ padding: "10px 10px", fontSize: 12.5, color: "#475569", fontWeight: 700, textAlign: "center" }}>Power BI</th>
                  <th style={{ padding: "10px 10px", fontSize: 12.5, color: "#475569", fontWeight: 700, textAlign: "center" }}>Redash</th>
                </tr>
              </thead>
              <tbody>
                {COMPARE_ROWS.map((r) => (
                  <tr key={r[0]}>
                    <td style={{ padding: "9px 16px", borderTop: "1px solid #f1f5f9", color: "#0f172a", fontSize: 13, fontWeight: 600 }}>{r[0]}</td>
                    {[r[1], r[2], r[3], r[4]].map((v, i) => (
                      <td key={i} style={{ padding: "9px 10px", textAlign: "center", borderTop: "1px solid #f1f5f9", background: i === 0 ? `${CYAN}06` : undefined }}>
                        {v === "~"
                          ? <span style={{ color: "#c79a3a" }}>~</span>
                          : v
                            ? <i className="fas fa-check" style={{ color: CYAN }} />
                            : <i className="fas fa-minus" style={{ color: "#cbd5e1" }} />
                        }
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </Section>

      {/* ── RBAC & Security ─────────────────────────────────────────────────── */}
      <Section id="security" bg="linear-gradient(135deg, #f0f6ff 0%, #eef2f7 100%)">
        <div style={{ marginBottom: 40 }}>
          <SectionLabel text="RBAC & Security" color={CYAN} />
          <SectionHeading>Four roles, least privilege</SectionHeading>
          <SectionSub>
            Every permission is enforced at the API layer. Four tiers from read-only viewers to full platform admins.
          </SectionSub>
        </div>

        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))", gap: 14 }}>
          {ROLES.map((r) => (
            <div key={r.role} style={{ background: "#fff", border: "1px solid #e2e8f0", borderTop: `4px solid ${r.color}`, borderRadius: 10, overflow: "hidden" }}>
              <div style={{
                padding: "12px 16px", borderBottom: "1px solid #e2e8f0",
                display: "flex", alignItems: "center", gap: 10,
              }}>
                <div style={{ width: 32, height: 32, borderRadius: 8, background: `${r.color}18`, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0 }}>
                  <i className={`fas ${r.icon}`} style={{ color: r.color, fontSize: 14 }} />
                </div>
                <div style={{ fontSize: 15, fontWeight: 700, color: "#0f172a" }}>{r.role}</div>
              </div>
              <div style={{ padding: "12px 16px" }}>
                <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  {r.perms.map((p, i) => (
                    <div key={i} style={{ display: "flex", alignItems: "flex-start", gap: 7 }}>
                      <i className="fas fa-check" style={{ color: r.color, fontSize: 10, marginTop: 3, flexShrink: 0 }} />
                      <span style={{ fontSize: 12.5, color: "#64748b", lineHeight: 1.4 }}>{p}</span>
                    </div>
                  ))}
                </div>
              </div>
            </div>
          ))}
        </div>

        {/* Security notes */}
        <div style={{ marginTop: 24, display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(280px, 1fr))", gap: 12 }}>
          {[
            { icon: "fa-lock", label: "OAuth 2.0 / OIDC", desc: "GitHub, Google, Microsoft identity providers" },
            { icon: "fa-key", label: "JWT sessions", desc: "Short-lived tokens, refresh rotation, no server-side state" },
            { icon: "fa-user-shield", label: "Per-object ACLs", desc: "Dashboards and charts scoped to role or individual user" },
            { icon: "fa-server", label: "Self-hosted", desc: "Your infrastructure, your network, your rules" },
          ].map((n) => (
            <div key={n.label} style={{
              padding: "12px 18px",
              background: "rgba(30,58,95,0.04)",
              borderRadius: 10,
              border: "1px solid rgba(30,58,95,0.08)",
              display: "flex",
              gap: 10,
              alignItems: "center",
            }}>
              <i className={`fas ${n.icon}`} style={{ color: CYAN, fontSize: 13, width: 16, textAlign: "center", flexShrink: 0 }} />
              <div>
                <div style={{ fontSize: 13, fontWeight: 600, color: "#0f172a" }}>{n.label}</div>
                <div style={{ fontSize: 11, color: "#64748b", marginTop: 1 }}>{n.desc}</div>
              </div>
            </div>
          ))}
        </div>
      </Section>

      {/* ── Get Started ─────────────────────────────────────────────────────── */}
      <section
        id="start"
        style={{
          background: `linear-gradient(135deg, ${color} 0%, #0f1e2e 100%)`,
          padding: "80px 1.5rem",
        }}
      >
        <div style={{ maxWidth: 1400, margin: "0 auto" }}>
          <div style={{ textAlign: "center", marginBottom: 28 }}>
            <SectionLabel text="Get Started" color={CYAN} light />
            <SectionHeading center light>
              Get started in minutes
            </SectionHeading>
            <SectionSub center light>
              Three steps. No Docker required. No vendor accounts.
            </SectionSub>
          </div>

          <div style={{ display: "flex", gap: 16, flexWrap: "wrap" }}>
            <StepCard
              number={1}
              title="Clone & install"
              code={`git clone https://github.com/PruthviProdduturi/Lens.git
cd Lens

# API
cd apps/lens-api
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
cp .env.example .env          # fill in DB + AI keys

# Web
cd ../lens-web
npm install`}
            />
            <StepCard
              number={2}
              title="Start the API + web"
              code={`# Terminal 1 — API (port 8000)
cd apps/lens-api
uvicorn main:app --reload

# Terminal 2 — Web (port 3000)
cd apps/lens-web
npm run dev`}
            />
            <StepCard
              number={3}
              title="Sign in and explore"
              code={`# Open your browser
http://localhost:3000

# Sign in with GitHub, Google, or Microsoft
# Add a data source under Settings > Databases
# Run your first query in SQL Lab`}
            />
          </div>
        </div>
      </section>

      {/* ── Footer ──────────────────────────────────────────────────────────── */}
      <footer style={{
        background: "#0f172a",
        padding: "20px 1.5rem",
        textAlign: "center",
        color: "#64748b",
        fontSize: 12,
      }}>
        &copy; {new Date().getFullYear()} Lens — a Kaveon platform module
      </footer>
    </div>
  );
}
