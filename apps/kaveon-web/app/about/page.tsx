"use client";

import { useRouter } from "next/navigation";
import { KaveonMark } from "../../components/KaveonMark";

function IconChat() {
  return (
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
    </svg>
  );
}

function IconBarChart() {
  return (
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <line x1="18" y1="20" x2="18" y2="10"/>
      <line x1="12" y1="20" x2="12" y2="4"/>
      <line x1="6" y1="20" x2="6" y2="14"/>
    </svg>
  );
}

function IconCode() {
  return (
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="16 18 22 12 16 6"/>
      <polyline points="8 6 2 12 8 18"/>
    </svg>
  );
}

function IconDatabase() {
  return (
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <ellipse cx="12" cy="5" rx="9" ry="3"/>
      <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/>
      <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/>
    </svg>
  );
}

const FEATURES = [
  {
    icon: <IconChat />,
    title: "AI Chat",
    description:
      "Ask questions in plain English. Kaveon writes the SQL, runs it, and shows you the answer — with charts inline.",
  },
  {
    icon: <IconBarChart />,
    title: "20+ Chart Types",
    description:
      "Lines, bars, pies, heatmaps, treemaps, scatter plots, gauges, and a 3D globe. All interactive, all themeable.",
  },
  {
    icon: <IconCode />,
    title: "SQL Lab",
    description:
      "Full Monaco editor with autocomplete, multi-tab, query history, and result caching. Write SQL like a pro.",
  },
  {
    icon: <IconDatabase />,
    title: "Multi-Source",
    description:
      "Connect Microsoft Fabric, Azure SQL, PostgreSQL, MySQL — query across all of them from one place.",
  },
];

const gradientButton: React.CSSProperties = {
  display: "inline-block",
  padding: "12px 28px",
  borderRadius: "8px",
  background: "linear-gradient(135deg, #0078D4 0%, #1a9de0 100%)",
  color: "#fff",
  fontWeight: 600,
  fontSize: "15px",
  border: "none",
  cursor: "pointer",
  textDecoration: "none",
};

const outlinedButton: React.CSSProperties = {
  display: "inline-block",
  padding: "12px 28px",
  borderRadius: "8px",
  background: "transparent",
  color: "var(--text-primary)",
  fontWeight: 600,
  fontSize: "15px",
  border: "1px solid var(--border, #e2e8f0)",
  cursor: "pointer",
  textDecoration: "none",
};

export default function AboutPage() {
  const router = useRouter();

  return (
    <div style={{ background: "var(--bg-primary)", minHeight: "100vh", fontFamily: "'Inter', 'Segoe UI', system-ui, sans-serif" }}>
      {/* Hero */}
      <section style={{ textAlign: "center", paddingTop: "100px", paddingBottom: "80px", paddingLeft: "24px", paddingRight: "24px" }}>
        <div style={{ marginBottom: "20px" }}>
          <KaveonMark size={80} useDirectColor />
        </div>
        <h1 style={{ fontSize: "42px", fontWeight: 700, color: "var(--text-primary)", margin: "0 0 12px" }}>
          Kaveon
        </h1>
        <p style={{ fontSize: "20px", fontWeight: 300, color: "var(--text-secondary)", margin: "0 0 40px" }}>
          Talk to your data.
        </p>
        <div style={{ display: "flex", gap: "16px", justifyContent: "center", flexWrap: "wrap" }}>
          <button style={gradientButton} onClick={() => router.push("/")}>
            Get Started
          </button>
          <a
            href="https://github.com/PruthviProdduturi/Kaveon"
            target="_blank"
            rel="noopener noreferrer"
            style={outlinedButton}
          >
            GitHub →
          </a>
        </div>
      </section>

      {/* Features */}
      <section style={{ maxWidth: "1000px", margin: "0 auto", padding: "0 24px 80px" }}>
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(2, 1fr)",
            gap: "20px",
          }}
        >
          {FEATURES.map((f) => (
            <div
              key={f.title}
              style={{
                background: "var(--bg-surface)",
                border: "1px solid var(--border, #e2e8f0)",
                borderRadius: "12px",
                padding: "28px",
              }}
            >
              <div style={{ marginBottom: "14px" }}>{f.icon}</div>
              <h3 style={{ fontSize: "16px", fontWeight: 600, color: "var(--text-primary)", margin: "0 0 8px" }}>
                {f.title}
              </h3>
              <p style={{ fontSize: "14px", color: "var(--text-secondary)", lineHeight: 1.6, margin: 0 }}>
                {f.description}
              </p>
            </div>
          ))}
        </div>
      </section>

      {/* Footer CTA */}
      <section
        style={{
          borderTop: "1px solid var(--border, #e2e8f0)",
          textAlign: "center",
          padding: "60px 24px",
        }}
      >
        <p style={{ fontSize: "14px", color: "var(--text-muted)", margin: "0 0 24px" }}>
          Open source · Self-hosted · MIT License
        </p>
        <button style={gradientButton} onClick={() => router.push("/")}>
          Get Started
        </button>
      </section>
    </div>
  );
}
