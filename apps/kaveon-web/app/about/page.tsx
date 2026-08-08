"use client";

import { KaveonMark } from "../../components/KaveonMark";

export default function AboutPage() {
  return (
    <div style={{ background: "var(--bg-primary)", minHeight: "100vh", color: "var(--text-primary)" }}>

      {/* ─── Hero ─── */}
      <section style={{ textAlign: "center", padding: "120px 24px 80px", position: "relative", overflow: "hidden" }}>
        {/* Background glow */}
        <div style={{ position: "absolute", top: "30%", left: "50%", transform: "translate(-50%, -50%)", width: 800, height: 600, borderRadius: "50%", background: "radial-gradient(ellipse, rgba(74,158,232,0.06) 0%, transparent 70%)", pointerEvents: "none" }} />

        <div style={{ position: "relative" }}>
          <KaveonMark size={64} useDirectColor />
          <h1 style={{ fontSize: 48, fontWeight: 600, margin: "24px 0 12px", letterSpacing: "-1px" }}>
            Talk to your data
          </h1>
          <p style={{ fontSize: 20, color: "var(--text-secondary)", maxWidth: 560, margin: "0 auto 40px", lineHeight: 1.6, fontWeight: 400 }}>
            Ask a question. Get a chart. Kaveon connects to your databases, understands your schema, and answers instantly.
          </p>
          <div style={{ display: "flex", gap: 12, justifyContent: "center", flexWrap: "wrap" }}>
            <a href="/" style={{ padding: "14px 32px", borderRadius: 10, background: "var(--accent)", color: "#fff", fontSize: 15, fontWeight: 500, textDecoration: "none", transition: "all 0.15s" }}>
              Get Started
            </a>
            <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" style={{ padding: "14px 32px", borderRadius: 10, background: "transparent", color: "var(--text-secondary)", fontSize: 15, fontWeight: 500, textDecoration: "none", border: "1px solid var(--border)", transition: "all 0.15s" }}>
              View on GitHub
            </a>
          </div>
        </div>
      </section>

      {/* ─── How It Works ─── */}
      <section style={{ maxWidth: 900, margin: "0 auto", padding: "0 24px 100px" }}>
        <h2 style={{ fontSize: 14, fontWeight: 600, textTransform: "uppercase", letterSpacing: "2px", color: "var(--accent)", textAlign: "center", marginBottom: 48 }}>
          How it works
        </h2>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 0, position: "relative" }}>
          {[
            { step: "01", title: "Ask", desc: "Type a question in plain English. \"Show revenue by region\" or \"Top 10 customers by sales.\"" },
            { step: "02", title: "Understand", desc: "Kaveon scans your schema, matches columns and metrics, and generates SQL — no LLM, no API key." },
            { step: "03", title: "Answer", desc: "SQL executes, the right chart type is picked automatically, and you see data with an intelligent summary." },
          ].map((item, i) => (
            <div key={item.step} style={{ padding: "0 28px", textAlign: "center", position: "relative" }}>
              {i < 2 && <div style={{ position: "absolute", right: 0, top: "20%", height: "60%", width: 1, background: "var(--border)" }} />}
              <div style={{ fontSize: 32, fontWeight: 700, color: "var(--accent)", marginBottom: 12, opacity: 0.4 }}>{item.step}</div>
              <h3 style={{ fontSize: 20, fontWeight: 600, marginBottom: 8 }}>{item.title}</h3>
              <p style={{ fontSize: 14, color: "var(--text-secondary)", lineHeight: 1.7 }}>{item.desc}</p>
            </div>
          ))}
        </div>
      </section>

      {/* ─── Demo Block ─── */}
      <section style={{ maxWidth: 700, margin: "0 auto", padding: "0 24px 100px" }}>
        <div style={{ background: "var(--bg-surface)", border: "1px solid var(--border)", borderRadius: 16, padding: "32px", overflow: "hidden" }}>
          <div style={{ display: "flex", gap: 8, marginBottom: 20 }}>
            <div style={{ width: 12, height: 12, borderRadius: "50%", background: "#ef4444" }} />
            <div style={{ width: 12, height: 12, borderRadius: "50%", background: "#f59e0b" }} />
            <div style={{ width: 12, height: 12, borderRadius: "50%", background: "#10b981" }} />
          </div>
          <div style={{ marginBottom: 16 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 12 }}>
              <div style={{ width: 24, height: 24, borderRadius: "50%", background: "var(--accent)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 11, color: "#fff", fontWeight: 700 }}>P</div>
              <span style={{ fontSize: 14, color: "var(--text-secondary)" }}>Show confirmed cases by country</span>
            </div>
            <div style={{ display: "flex", alignItems: "flex-start", gap: 8 }}>
              <div style={{ width: 24, height: 24, borderRadius: "50%", background: "var(--bg-hover)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 11, fontWeight: 700, color: "var(--text-secondary)" }}>K</div>
              <div style={{ flex: 1 }}>
                <p style={{ fontSize: 14, color: "var(--text-primary)", lineHeight: 1.6, margin: "0 0 12px" }}>
                  Found <strong>195</strong> results for confirmed by country. Top 3: <strong>United States</strong> (103.8M), <strong>India</strong> (45.0M), <strong>France</strong> (39.9M).
                </p>
                <div style={{ height: 140, background: "var(--bg-elevated)", borderRadius: 10, border: "1px solid var(--border)", display: "flex", alignItems: "flex-end", padding: "16px 20px", gap: 8 }}>
                  {[103, 45, 39, 34, 32, 25, 24, 21].map((h, i) => (
                    <div key={i} style={{ flex: 1, height: `${h * 1.1}px`, background: i === 0 ? "var(--accent)" : "rgba(var(--accent-rgb), 0.4)", borderRadius: "4px 4px 0 0", transition: "height 0.3s" }} />
                  ))}
                </div>
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* ─── Features ─── */}
      <section style={{ maxWidth: 1000, margin: "0 auto", padding: "0 24px 100px" }}>
        <h2 style={{ fontSize: 14, fontWeight: 600, textTransform: "uppercase", letterSpacing: "2px", color: "var(--accent)", textAlign: "center", marginBottom: 48 }}>
          Built for data teams
        </h2>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 20 }}>
          {[
            { icon: <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>, title: "Conversational Querying", desc: "Type questions, get charts. No SQL required. The NL→SQL engine handles pattern matching, schema binding, and chart selection automatically." },
            { icon: <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/></svg>, title: "37 Chart Types", desc: "Bar, line, pie, heatmap, treemap, scatter, funnel, gauge, waterfall, calendar, 3D globe. All interactive, all dark-mode aware." },
            { icon: <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>, title: "SQL Lab", desc: "Monaco editor with autocomplete, multi-tab, query history, and caching. Inline AI bar for instant SQL generation." },
            { icon: <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>, title: "Dashboards", desc: "Drag-and-drop canvas with cross-chart filtering, shared filters, auto-refresh, and one-click publishing." },
            { icon: <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>, title: "Multi-Source", desc: "Microsoft Fabric, Azure SQL, PostgreSQL, MySQL, StarRocks. Connect them all, query across them from one place." },
            { icon: <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>, title: "Self-Hosted & Secure", desc: "Your infrastructure, your data. OAuth sign-in, role-based access, encrypted secrets. MIT licensed." },
          ].map((f) => (
            <div key={f.title} style={{ padding: 28, borderRadius: 14, border: "1px solid var(--border)", background: "var(--bg-surface)" }}>
              <div style={{ marginBottom: 16 }}>{f.icon}</div>
              <h3 style={{ fontSize: 17, fontWeight: 600, marginBottom: 8 }}>{f.title}</h3>
              <p style={{ fontSize: 14, color: "var(--text-secondary)", lineHeight: 1.7, margin: 0 }}>{f.desc}</p>
            </div>
          ))}
        </div>
      </section>

      {/* ─── Tech Stack ─── */}
      <section style={{ maxWidth: 700, margin: "0 auto", padding: "0 24px 100px", textAlign: "center" }}>
        <h2 style={{ fontSize: 14, fontWeight: 600, textTransform: "uppercase", letterSpacing: "2px", color: "var(--accent)", marginBottom: 32 }}>
          Tech Stack
        </h2>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 10, justifyContent: "center" }}>
          {["Next.js 15", "React 19", "TypeScript", "FastAPI", "Python 3.11", "ECharts", "Monaco Editor", "PostgreSQL", "Azure", "Vercel"].map((t) => (
            <span key={t} style={{ padding: "8px 18px", borderRadius: 999, border: "1px solid var(--border)", fontSize: 13, color: "var(--text-secondary)", background: "var(--bg-surface)" }}>{t}</span>
          ))}
        </div>
      </section>

      {/* ─── Bottom CTA ─── */}
      <section style={{ textAlign: "center", padding: "80px 24px", borderTop: "1px solid var(--border)" }}>
        <KaveonMark size={40} useDirectColor />
        <h2 style={{ fontSize: 28, fontWeight: 500, margin: "20px 0 12px" }}>Ready to talk to your data?</h2>
        <p style={{ fontSize: 15, color: "var(--text-muted)", marginBottom: 32 }}>Open source · Self-hosted · MIT License</p>
        <div style={{ display: "flex", gap: 12, justifyContent: "center", flexWrap: "wrap" }}>
          <a href="/" style={{ padding: "14px 32px", borderRadius: 10, background: "var(--accent)", color: "#fff", fontSize: 15, fontWeight: 500, textDecoration: "none" }}>
            Get Started
          </a>
          <a href="/docs" target="_blank" style={{ padding: "14px 32px", borderRadius: 10, border: "1px solid var(--border)", color: "var(--text-secondary)", fontSize: 15, fontWeight: 500, textDecoration: "none" }}>
            Documentation
          </a>
        </div>
      </section>

      {/* ─── Footer ─── */}
      <footer style={{ textAlign: "center", padding: "24px", borderTop: "1px solid var(--border)" }}>
        <p style={{ fontSize: 12, color: "var(--text-muted)" }}>
          © {new Date().getFullYear()} Kaveon
        </p>
      </footer>
    </div>
  );
}
