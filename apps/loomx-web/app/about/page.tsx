"use client";

import React, { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { useTheme } from "../../contexts/ThemeContext";
import { useRole } from "../../hooks/useRole";

// ── force a hex color to at least minL% lightness (so text is always visible on dark bg) ──
function forceLightHex(hex: string, minL = 72): string {
  const m = hex.match(/^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i);
  if (!m) return "#ffffff";
  const r = parseInt(m[1], 16) / 255, g = parseInt(m[2], 16) / 255, b = parseInt(m[3], 16) / 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  let h = 0, s = 0, l = (max + min) / 2;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    h = max === r ? ((g - b) / d + (g < b ? 6 : 0)) / 6
      : max === g ? ((b - r) / d + 2) / 6
      : ((r - g) / d + 4) / 6;
  }
  l = Math.max(l * 100, minL) / 100;
  const hue2rgb = (p: number, q: number, t: number) => {
    if (t < 0) t += 1; if (t > 1) t -= 1;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
    return p;
  };
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  const hex2 = (v: number) => Math.round(hue2rgb(p, q, v) * 255).toString(16).padStart(2, "0");
  return `#${hex2(h + 1 / 3)}${hex2(h)}${hex2(h - 1 / 3)}`;
}

// ── tiny helpers ──────────────────────────────────────────────────────────────

function Section({ id, children, bg }: { id?: string; children: React.ReactNode; bg?: string }) {
  return (
    <section id={id} style={{ background: bg ?? "white", padding: "80px 0" }}>
      <div style={{ maxWidth: 1140, margin: "0 auto", padding: "0 24px" }}>
        {children}
      </div>
    </section>
  );
}

function SectionLabel({ text, color }: { text: string; color: string }) {
  return (
    <div style={{
      display: "inline-flex", alignItems: "center", gap: 7,
      padding: "5px 14px", borderRadius: 20,
      background: `${color}14`, border: `1px solid ${color}30`,
      fontSize: 12, fontWeight: 700, color, letterSpacing: "0.08em",
      textTransform: "uppercase", marginBottom: 14,
    }}>
      {text}
    </div>
  );
}

function SectionHeading({ children, center }: { children: React.ReactNode; center?: boolean }) {
  return (
    <h2 style={{
      fontSize: "clamp(1.75rem, 3vw, 2.25rem)", fontWeight: 800, color: "#0f172a",
      lineHeight: 1.2, margin: "0 0 16px", textAlign: center ? "center" : undefined,
    }}>
      {children}
    </h2>
  );
}

function SectionSub({ children, center }: { children: React.ReactNode; center?: boolean }) {
  return (
    <p style={{
      fontSize: 17, color: "#475569", lineHeight: 1.7, margin: "0 0 48px",
      maxWidth: center ? 640 : undefined, textAlign: center ? "center" : undefined,
      marginLeft: center ? "auto" : undefined, marginRight: center ? "auto" : undefined,
    }}>
      {children}
    </p>
  );
}

function FeatureCard({ icon, color, title, body }: { icon: string; color: string; title: string; body: string }) {
  return (
    <div style={{
      background: "white", border: "1px solid #e5e7eb", borderRadius: 16,
      padding: "28px 24px", transition: "box-shadow 0.2s, transform 0.2s",
      boxShadow: "0 1px 4px rgba(15,23,42,0.05)",
    }}
      onMouseEnter={e => { (e.currentTarget as HTMLDivElement).style.boxShadow = "0 8px 28px rgba(15,23,42,0.12)"; (e.currentTarget as HTMLDivElement).style.transform = "translateY(-2px)"; }}
      onMouseLeave={e => { (e.currentTarget as HTMLDivElement).style.boxShadow = "0 1px 4px rgba(15,23,42,0.05)"; (e.currentTarget as HTMLDivElement).style.transform = ""; }}
    >
      <div style={{
        width: 44, height: 44, borderRadius: 11, marginBottom: 16, flexShrink: 0,
        background: `${color}15`, display: "flex", alignItems: "center", justifyContent: "center",
      }}>
        <i className={`fas ${icon}`} style={{ color, fontSize: 18 }} />
      </div>
      <div style={{ fontSize: 15.5, fontWeight: 700, color: "#0f172a", marginBottom: 8 }}>{title}</div>
      <div style={{ fontSize: 13.5, color: "#64748b", lineHeight: 1.65 }}>{body}</div>
    </div>
  );
}

function ChartPill({ icon, label, color }: { icon: string; label: string; color: string }) {
  return (
    <div style={{
      display: "flex", alignItems: "center", gap: 9,
      padding: "10px 16px", borderRadius: 10,
      background: `${color}0e`, border: `1px solid ${color}28`,
      fontSize: 13, fontWeight: 600, color: "#334155",
      whiteSpace: "nowrap",
    }}>
      <i className={`fas ${icon}`} style={{ color, fontSize: 14 }} />
      {label}
    </div>
  );
}

function DbCard({ icon, name, color, badge }: { icon: React.ReactNode; name: string; color: string; badge?: string }) {
  return (
    <div style={{
      display: "flex", flexDirection: "column", alignItems: "center", gap: 10,
      padding: "24px 20px", background: "white", border: "1px solid #e5e7eb",
      borderRadius: 14, boxShadow: "0 1px 4px rgba(15,23,42,0.04)",
      transition: "box-shadow 0.18s",
    }}
      onMouseEnter={e => { (e.currentTarget as HTMLDivElement).style.boxShadow = `0 6px 20px ${color}22`; }}
      onMouseLeave={e => { (e.currentTarget as HTMLDivElement).style.boxShadow = "0 1px 4px rgba(15,23,42,0.04)"; }}
    >
      <div style={{ width: 48, height: 48, display: "flex", alignItems: "center", justifyContent: "center" }}>{icon}</div>
      <div style={{ fontSize: 13, fontWeight: 700, color: "#0f172a" }}>{name}</div>
      {badge && (
        <span style={{ fontSize: 10, fontWeight: 700, color: "#f59e0b", background: "#fef9c3", border: "1px solid #fde68a", borderRadius: 5, padding: "2px 7px", letterSpacing: "0.04em" }}>
          {badge}
        </span>
      )}
    </div>
  );
}

function ApiRow({ method, path, desc, auth }: { method: string; path: string; desc: string; auth: string }) {
  const methodColors: Record<string, string> = {
    GET: "#059669", POST: "#2563eb", PUT: "#7c3aed", PATCH: "#d97706", DELETE: "#dc2626",
  };
  const authColors: Record<string, string> = {
    None: "#64748b", User: "#0ea5e9", "Analyst+": "#7c3aed", "Editor+": "#d97706", Admin: "#dc2626",
  };
  return (
    <tr style={{ borderBottom: "1px solid #f1f5f9" }}>
      <td style={{ padding: "10px 14px", whiteSpace: "nowrap" }}>
        <span style={{
          fontSize: 11, fontWeight: 800, color: methodColors[method] ?? "#374151",
          background: `${methodColors[method] ?? "#374151"}12`,
          borderRadius: 5, padding: "3px 8px", letterSpacing: "0.04em",
        }}>{method}</span>
      </td>
      <td style={{ padding: "10px 14px", fontFamily: "monospace", fontSize: 12.5, color: "#0f172a" }}>{path}</td>
      <td style={{ padding: "10px 14px", fontSize: 13, color: "#475569" }}>{desc}</td>
      <td style={{ padding: "10px 14px", whiteSpace: "nowrap" }}>
        <span style={{
          fontSize: 11, fontWeight: 700, color: authColors[auth] ?? "#64748b",
          background: `${authColors[auth] ?? "#64748b"}12`,
          borderRadius: 5, padding: "2px 8px",
        }}>{auth}</span>
      </td>
    </tr>
  );
}

// ── SVG brand icons (inline, no import needed) ────────────────────────────────

const FabricSvg = () => (
  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="36" height="36">
    <defs>
      <linearGradient id="ab-fi-b" x1="15.667" x2="8.644" y1="16.726" y2="9.087" gradientUnits="userSpaceOnUse">
        <stop offset=".042" stopColor="#ABE88E"/><stop offset=".549" stopColor="#2AAA92"/><stop offset=".906" stopColor="#117865"/>
      </linearGradient>
      <linearGradient id="ab-fi-d" x1="3.507" x2="21.297" y1="7.61" y2="7.61" gradientUnits="userSpaceOnUse">
        <stop offset=".043" stopColor="#25FFD4"/><stop offset=".874" stopColor="#55DDB9"/>
      </linearGradient>
      <linearGradient id="ab-fi-e" x1="3.507" x2="19.532" y1="5.124" y2="12.565" gradientUnits="userSpaceOnUse">
        <stop stopColor="#6AD6F9"/><stop offset=".23" stopColor="#60E9D0"/>
        <stop offset=".651" stopColor="#6DE9BB"/><stop offset=".994" stopColor="#ABE88E"/>
      </linearGradient>
    </defs>
    <path fill="url(#ab-fi-b)" d="M5.07 16.078c-2.431.376-2.93 2.211-2.93 2.211l2.328-8.556 12.168-1.646-1.66 6.027a.85.85 0 0 1-.693.622l-.068.011-9.213 1.342z"/>
    <path fill="url(#ab-fi-d)" d="m6.45 10.619 13.47-1.99a.8.8 0 0 0 .662-.586l1.39-5.03a.797.797 0 0 0-.87-1.006L8.25 3.905a3.59 3.59 0 0 0-2.89 2.597L3.507 13.22c.372-1.36.6-2.178 2.943-2.602Z"/>
    <path fill="url(#ab-fi-e)" d="m6.45 10.619 13.47-1.99a.8.8 0 0 0 .662-.586l1.39-5.03a.797.797 0 0 0-.87-1.006L8.25 3.905a3.59 3.59 0 0 0-2.89 2.597L3.507 13.22c.372-1.36.6-2.178 2.943-2.602Z"/>
  </svg>
);

const AzureSvg = () => (
  <svg viewBox="0 0 24 24" width="36" height="36" fill="#0078D4">
    <path d="M10.432.006L5.29 13.53l4.261 7.67h13.957L17.636 11.09 10.432.006zM5.054 15.233L.492 21.2H6.95l-1.896-5.967z"/>
  </svg>
);

const PgSvg = () => (
  <svg viewBox="0 0 24 24" width="36" height="36">
    <path fill="#336791" d="M23.5594 14.7228a.5269.5269 0 0 0-.0563-.1191c-.139-.2632-.4768-.3418-1.0074-.2321-1.6533.3411-2.2935.1312-2.5256-.0191 1.342-2.0482 2.445-4.522 3.0411-6.8297.2714-1.0507.7982-3.5237.1222-4.7316a1.5641 1.5641 0 0 0-.1509-.235C21.6931.9086 19.8007.0248 17.5099.0005c-1.4947-.0158-2.7705.3461-3.1161.4794a9.449 9.449 0 0 0-.5159-.0816 8.044 8.044 0 0 0-1.3114-.1278c-1.1822-.0184-2.2038.2642-3.0498.8406-.8573-.3211-4.7888-1.645-7.2219.0788C.9359 2.1526.3086 3.8733.4302 6.3043c.0409.818.5069 3.334 1.2423 5.7436.4598 1.5065.9387 2.7019 1.4334 3.582.553.9942 1.1259 1.5933 1.7143 1.7895.4474.1491 1.1327.1441 1.8581-.7279.8012-.9635 1.5903-1.8258 1.9446-2.2069.4351.2355.9064.3625 1.39.3772a.0569.0569 0 0 0 .0004.0041 11.0312 11.0312 0 0 0-.2472.3054c-.3389.4302-.4094.5197-1.5002.7443-.3102.064-1.1344.2339-1.1464.8115-.0025.1224.0329.2309.0919.3268.2269.4231.9216.6097 1.015.6331 1.3345.3335 2.5044.092 3.3714-.6787-.017 2.231.0775 4.4174.3454 5.0874.2212.5529.7618 1.9045 2.4692 1.9043.2505 0 .5263-.0291.8296-.0941 1.7819-.3821 2.5557-1.1696 2.855-2.9059.1503-.8707.4016-2.8753.5388-4.1012.0169-.0703.0357-.1207.057-.1362.0007-.0005.0697-.0471.4272.0307a.3673.3673 0 0 0 .0443.0068l.2539.0223.0149.001c.8468.0384 1.9114-.1426 2.5312-.4308.6438-.2988 1.8057-1.0323 1.5951-1.6698z"/>
  </svg>
);

const MySQLSvg = () => (
  <svg viewBox="0 0 24 24" width="36" height="36">
    <path fill="#4479A1" d="M16.405 5.501c-.115 0-.193.014-.274.033v.013h.014c.054.104.146.18.214.273.054.107.1.214.154.32l.014-.015c.094-.066.14-.172.14-.333-.04-.047-.046-.094-.08-.134-.04-.064-.14-.108-.18-.157zM12 0C5.373 0 0 5.373 0 12s5.373 12 12 12 12-5.373 12-12S18.627 0 12 0zm5.187 17.966c-.36.072-.744.072-1.08 0-.293-.065-.53-.228-.724-.47-.194-.242-.307-.554-.307-.915 0-.377.12-.68.328-.913.216-.227.49-.358.807-.358.3 0 .548.104.74.307.193.2.307.484.307.84h-1.604c.01.264.105.468.26.596.147.124.35.19.59.19.22 0 .41-.046.565-.137.155-.09.27-.217.336-.375l.38.15c-.09.23-.25.42-.465.55zm-8.5.234L7.5 14.2l-1.187 4h-.9L4 13.2h.92l.987 3.7 1.19-3.7h.81l1.19 3.7.987-3.7h.92l-1.413 5zm4.04 0h-.875v-5h.875v5zm3.14-4.15c-.25 0-.44.09-.57.27s-.19.43-.19.75v3.13h-.875v-5h.84v.75c.11-.26.27-.46.48-.6.22-.15.47-.22.77-.22l.09.01v.87c-.18-.02-.36-.01-.545.04z"/>
  </svg>
);

const TrinoSvg = () => (
  <svg viewBox="0 0 24 24" width="36" height="36" fill="#DD1C1C">
    <path d="M14.124 16.8529a.1615.1615 0 1 1 .1576.1614.1577.1577 0 0 1-.1576-.1614zm-5.607-.1576a.1614.1614 0 1 0 0 .3228.1614.1614 0 0 0 0-.3228zm10.1341-.6648v1.9869c-.031.5788-.524 1.0237-1.1029.9954h-.3843a5.0596 5.0596 0 0 1-1.1298 1.7178.3192.3192 0 0 0 0 .465l.2382.2191a.3036.3036 0 0 1 .0385.4304c-1.126 1.3835-2.9669 2.1521-5.0498 2.1521a6.575 6.575 0 0 1-4.8192-1.8985c-.0029-.0032-.0059-.0063-.0087-.0096a.6302.6302 0 0 1 .0548-.8896c.137-.1265.1371-.3462 0-.4727a4.944 4.944 0 0 1-1.126-1.714h-.3497c-.5797.0284-1.0737-.416-1.1068-.9954v-1.9869c.0351-.5779.5286-1.02 1.1068-.9915h.2728a5.7648 5.7648 0 0 1 2.0791-3.0936c-.4227-1.0991-1.1529-3.2551-1.226-5.0075C6.0229 4.4705 6.2189.078 7.8253.001c1.6064-.0768 1.3719 4.0275 1.0991 6.6946a32.732 32.732 0 0 0-.123 4.4503 6.994 6.994 0 0 1 2.4826-.4304 7.2414 7.2414 0 0 1 1.7371.2075c.2614-1.2682.8762-3.574 2.0292-5.1958c1.6717-2.352 3.4357-4.7808 4.6116-4.1006c1.176.6802-.3074 3.1398-1.3297 4.4272c-1.0222 1.2874-2.7862 3.2089-3.3742 4.2274c-.2114.3843-.4304.8032-.5956 1.1529a5.7375 5.7375 0 0 1 2.9169 3.6125h.073v-2.3058a.3075.3075 0 0 0-.1806-.2844a.9148.9148 0 0 1-.5573-.8148a1.0184 1.0184 0 0 1 .9045-.9044c.5593-.0598 1.061.3452 1.1208.9044a.9187.9187 0 0 1-.5534.8148a.3074.3074 0 0 0-.1691.2844v2.1522a.3113.3113 0 0 0 .1691.2805a.9724.9724 0 0 1 .5648.857z"/>
  </svg>
);

const StarRocksSvg = () => (
  <svg viewBox="0 0 100 100" width="36" height="36">
    <path fill="#01808F" d="M11.8,26.4c-0.1,2.2,0.9,3.4,2.3,4.5c9,7.4,18,14.8,27,22.2c2.5,2.1,2.7,3.5,1,6.2c-4.4,6.7-8.8,13.4-13.2,20.1c-1.7,2.5-3.2,2.9-5.9,1.4c-2.8-1.6-5.6-3.2-8.4-4.8c-3.2-1.8-4.8-4.6-4.8-8.3c0-11.8,0-23.6,0-35.5C9.8,30.2,10.3,28.4,11.8,26.4z"/>
    <path fill="#01808F" d="M87.9,73.8c0.6-2.3-0.5-3.5-1.9-4.6c-9-7.4-17.9-14.7-26.8-22.1c-2.9-2.4-3.1-3.6-1.1-6.7c4.3-6.6,8.7-13.2,13-19.8c1.7-2.6,3.3-2.9,6-1.4c2.8,1.6,5.6,3.2,8.3,4.8c3.2,1.8,4.7,4.5,4.7,8.2c0,11.9,0,23.7,0,35.6C90.2,69.9,89.6,71.9,87.9,73.8z"/>
    <path fill="#FEBD02" d="M32.9,43.2c-0.6-0.4-17.3-14.1-17.5-14.3c-2.4-2.2-2.2-4.1,0.6-5.8C23.2,19,42.7,7.9,45.2,6.4c3.2-1.9,6.4-1.9,9.6,0c4.6,2.7,9.2,5.3,13.8,8c2.2,1.3,2.2,2.7,0,3.9C57.7,24.5,46.9,30.8,36,37C33.5,38.4,31.8,39.9,32.9,43.2z"/>
  </svg>
);

// ── Main page ─────────────────────────────────────────────────────────────────

export default function AboutPage() {
  const { primaryColor, gradientColors } = useTheme();
  const { isAdmin } = useRole();
  const heroRef = useRef<HTMLDivElement>(null);
  const [scrolled, setScrolled] = useState(false);

  useEffect(() => {
    const handler = () => setScrolled(window.scrollY > 60);
    window.addEventListener("scroll", handler);
    return () => window.removeEventListener("scroll", handler);
  }, []);

  // ── nav structure ──
  const [openNav, setOpenNav] = useState<string | null>(null);
  const navGroups: Array<{ label: string; href?: string; items?: { label: string; href: string }[]; dividerBefore?: boolean }> = [
    { label: "Features", href: "#features" },
    { label: "Platform", dividerBefore: true, items: [
      { label: "Charts", href: "#charts" },
      { label: "Dashboards", href: "#dashboards" },
      { label: "Data Sources", href: "#datasources" },
      { label: "SQL Lab", href: "#sqllab" },
      { label: "AI", href: "#ai" },
    ]},
    { label: "Security", href: "#security" },
    { label: "Developers", items: [
      { label: "API Reference", href: "#api" },
      { label: "Tech Stack", href: "#stack" },
    ]},
    { label: "Resources", dividerBefore: true, items: [
      { label: "Getting Started", href: "#start" },
      { label: "What's New", href: "#whatsnew" },
      { label: "FAQ", href: "#faq" },
    ]},
  ];

  return (
    <div style={{ fontFamily: "'Inter', system-ui, -apple-system, sans-serif", color: "#0f172a" }}>

      {/* ── Sticky page-internal nav ── */}
      <div style={{
        position: "sticky", top: 0, zIndex: 40, background: "rgba(255,255,255,0.95)",
        backdropFilter: "blur(12px)", borderBottom: scrolled ? "1px solid #e5e7eb" : "1px solid transparent",
        transition: "border-color 0.2s, box-shadow 0.2s",
        boxShadow: scrolled ? "0 1px 12px rgba(0,0,0,0.08)" : "none",
      }}>
        <div style={{ maxWidth: 1140, margin: "0 auto", padding: "0 24px", display: "flex", alignItems: "center", gap: 32, height: 52 }}>
          <span
            style={{ fontWeight: 800, fontSize: 15, color: primaryColor, letterSpacing: "-0.3px", flexShrink: 0, cursor: "pointer" }}
            onClick={() => window.scrollTo({ top: 0, behavior: "smooth" })}
          >
            LooMX
          </span>
          <div style={{ display: "flex", alignItems: "center" }}>
            {navGroups.map(g => (
              <React.Fragment key={g.label}>
                {g.dividerBefore && (
                  <div style={{ width: 1, height: 16, background: "#e2e8f0", margin: "0 4px", flexShrink: 0 }} />
                )}
                {g.href ? (
                  <a href={g.href} style={{
                    display: "flex", alignItems: "center", height: 32,
                    padding: "0 11px", borderRadius: 7, fontSize: 13, fontWeight: 500,
                    color: "#475569", textDecoration: "none", whiteSpace: "nowrap",
                    transition: "background 0.15s, color 0.15s",
                  }}
                    onMouseEnter={e => { (e.currentTarget as HTMLAnchorElement).style.background = `${primaryColor}12`; (e.currentTarget as HTMLAnchorElement).style.color = primaryColor; }}
                    onMouseLeave={e => { (e.currentTarget as HTMLAnchorElement).style.background = ""; (e.currentTarget as HTMLAnchorElement).style.color = "#475569"; }}
                  >
                    {g.label}
                  </a>
                ) : (
                  <div style={{ position: "relative" }}
                    onMouseEnter={() => setOpenNav(g.label)}
                    onMouseLeave={() => setOpenNav(null)}
                  >
                    <button style={{
                      display: "flex", alignItems: "center", height: 32, gap: 5,
                      padding: "0 11px", borderRadius: 7, fontSize: 13, fontWeight: 500,
                      color: openNav === g.label ? primaryColor : "#475569",
                      background: openNav === g.label ? `${primaryColor}12` : "none",
                      border: "none", cursor: "pointer", whiteSpace: "nowrap",
                      lineHeight: 1, transition: "background 0.15s, color 0.15s",
                    }}>
                      {g.label}
                      <i className="fas fa-caret-down" style={{ fontSize: 10, opacity: 0.6 }} />
                    </button>
                    {openNav === g.label && (
                      <div style={{
                        position: "absolute", top: "calc(100% + 4px)", left: 0, zIndex: 50,
                        background: "white", border: "1px solid #e5e7eb", borderRadius: 10,
                        boxShadow: "0 8px 24px rgba(0,0,0,0.1)", padding: "6px", minWidth: 180,
                      }}>
                        {g.items!.map(item => (
                          <a key={item.href} href={item.href} style={{
                            display: "flex", alignItems: "center", height: 34,
                            padding: "0 12px", borderRadius: 7,
                            fontSize: 13, fontWeight: 500, color: "#334155",
                            textDecoration: "none", whiteSpace: "nowrap",
                            transition: "background 0.12s, color 0.12s",
                          }}
                            onMouseEnter={e => { (e.currentTarget as HTMLAnchorElement).style.background = `${primaryColor}10`; (e.currentTarget as HTMLAnchorElement).style.color = primaryColor; }}
                            onMouseLeave={e => { (e.currentTarget as HTMLAnchorElement).style.background = ""; (e.currentTarget as HTMLAnchorElement).style.color = "#334155"; }}
                            onClick={() => setOpenNav(null)}
                          >
                            {item.label}
                          </a>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </React.Fragment>
            ))}
          </div>
          <div style={{ marginLeft: "auto", flexShrink: 0 }}>
            <Link href="/" style={{
              padding: "7px 18px", borderRadius: 8, fontSize: 13, fontWeight: 600,
              background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
              color: "white", textDecoration: "none",
              boxShadow: `0 2px 8px ${primaryColor}30`,
            }}>
              Open App
            </Link>
          </div>
        </div>
      </div>

      {/* ── Hero ── */}
      <div ref={heroRef} style={{
        background: "linear-gradient(160deg, #0f172a 0%, #1e293b 95%, #1e293b 100%)",
        padding: "100px 24px 90px", textAlign: "center", position: "relative", overflow: "hidden",
      }}>
        {/* Background grid pattern */}
        <div style={{
          position: "absolute", inset: 0, opacity: 0.04,
          backgroundImage: "linear-gradient(#fff 1px, transparent 1px), linear-gradient(90deg, #fff 1px, transparent 1px)",
          backgroundSize: "40px 40px",
        }} />

        <div style={{ position: "relative", maxWidth: 800, margin: "0 auto" }}>
          <div style={{
            display: "inline-flex", alignItems: "center", gap: 8,
            background: `${primaryColor}20`, border: `1px solid ${primaryColor}40`,
            borderRadius: 20, padding: "6px 16px", marginBottom: 28,
            fontSize: 12.5, fontWeight: 700, color: primaryColor, letterSpacing: "0.06em",
            textTransform: "uppercase",
          }}>
            <i className="fas fa-bolt" style={{ fontSize: 11 }} />
            Enterprise Analytics Platform
          </div>

          <h1 style={{
            fontSize: "clamp(2.5rem, 6vw, 4rem)", fontWeight: 900, color: "white",
            lineHeight: 1.15, margin: "0 0 20px", letterSpacing: "-1.5px",
          }}>
            Live Operational Outcomes &amp; Metrics<br />
            <span style={{
              backgroundImage: `linear-gradient(135deg, ${forceLightHex(primaryColor, 72)}, ${forceLightHex(primaryColor, 90)})`,
              WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent",
              backgroundClip: "text",
            }}>
              eXperience
            </span>
          </h1>

          <p style={{
            fontSize: "clamp(1rem, 2vw, 1.2rem)", color: "#94a3b8", lineHeight: 1.7,
            margin: "0 auto 40px", maxWidth: 600,
          }}>
            A self-hosted, enterprise-grade analytics platform with multi-provider authentication —
            Local login, Azure AD / Entra ID, or Google OAuth2. Connect any data source, build
            visualisations, assemble dashboards, and generate SQL with AI. Everything configurable from the UI.
          </p>

          <div style={{ display: "flex", gap: 14, justifyContent: "center", flexWrap: "wrap" }}>
            <Link href="/" style={{
              padding: "13px 28px", borderRadius: 10, fontSize: 15, fontWeight: 700,
              background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
              color: "white", textDecoration: "none",
              boxShadow: `0 4px 18px ${primaryColor}40`,
            }}>
              <i className="fas fa-arrow-right" style={{ marginRight: 8 }} />Open LooMX
            </Link>
            <a href="#features" style={{
              padding: "13px 28px", borderRadius: 10, fontSize: 15, fontWeight: 600,
              background: "rgba(255,255,255,0.08)", border: "1px solid rgba(255,255,255,0.15)",
              color: "white", textDecoration: "none",
            }}>
              Explore Features
            </a>
          </div>

          {/* Stats row */}
          <div style={{ display: "flex", justifyContent: "center", gap: "clamp(20px, 4vw, 60px)", marginTop: 60, flexWrap: "wrap" }}>
            {[
              { num: "20+", label: "Chart Types" },
              { num: "6",   label: "Data Sources" },
              { num: "4",   label: "RBAC Roles" },
              { num: "3",   label: "AI Providers" },
            ].map(s => (
              <div key={s.label} style={{ textAlign: "center" }}>
                <div style={{ fontSize: "clamp(1.6rem, 3vw, 2.2rem)", fontWeight: 900, color: "white", lineHeight: 1 }}>{s.num}</div>
                <div style={{ fontSize: 12.5, color: "#64748b", marginTop: 5, letterSpacing: "0.04em" }}>{s.label}</div>
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* ── Features ── */}
      <Section id="features" bg="#f8fafc">
        <div style={{ textAlign: "center", marginBottom: 56 }}>
          <SectionLabel text="Everything You Need" color={primaryColor} />
          <SectionHeading center>Built for modern data teams</SectionHeading>
          <SectionSub center>
            Every feature designed around real analytics workflows — from ad-hoc SQL exploration to
            published dashboards shared across your entire organisation.
          </SectionSub>
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", gap: 20 }}>
          <FeatureCard icon="fa-shield-alt" color="#dc2626"
            title="Azure AD Security"
            body="Full JWT RS256 signature verification against your tenant's JWKS endpoint. Delegated user tokens — every query runs as the authenticated user. No service accounts, no shared credentials." />
          <FeatureCard icon="fa-users-cog" color="#7c3aed"
            title="Role-Based Access Control"
            body="Four roles: Viewer, Analyst, Editor, Admin. Assigned via Azure AD App Roles or the built-in user management UI. Content visibility: private, internal, published — enforced at the query level." />
          <FeatureCard icon="fa-flask" color="#2563eb"
            title="SQL Lab"
            body="Monaco Editor (VS Code engine) with SQL syntax highlighting, multi-tab sessions, real-time results, column sorting, full history with audit trail, and instant save-to-library." />
          <FeatureCard icon="fa-wand-magic-sparkles" color="#7c3aed"
            title="Inline AI"
            body="Press the AI wand in SQL Lab, describe what you need in plain English, and the generated SQL drops straight into your active tab. Powered by Claude, GPT-4o, or GitHub Models." />
          <FeatureCard icon="fa-chart-bar" color="#059669"
            title="20+ Chart Types"
            body="Bar, line, area, pie, scatter, heatmap, funnel, gauge, treemap, waterfall, calendar heat map, world map globe (3D WebGL), KPI cards, and more. All powered by Apache ECharts." />
          <FeatureCard icon="fa-layer-group" color="#0ea5e9"
            title="Semantic Datasets"
            body="Define dimensions, metrics, and filter columns once. LooMX auto-generates optimised SQL with multi-table JOINs. Change the dataset — every chart and dashboard updates automatically." />
          <FeatureCard icon="fa-th-large" color="#d97706"
            title="Dashboard Builder"
            body="Drag-and-drop canvas with resizable chart tiles, row/column/tab containers, rich markdown text blocks, headers, and dividers. Cross-chart filtering with one click." />
          <FeatureCard icon="fa-database" color="#0ea5e9"
            title="Multi-Source"
            body="Register Fabric SQL, Azure SQL, PostgreSQL, MySQL, Trino, and StarRocks data sources from the UI. No .env changes, no restarts. Per-database connection pooling with startup warmup." />
          <FeatureCard icon="fa-cog" color="#64748b"
            title="Admin Controls"
            body="Full admin UI: user role management, data source CRUD, AI provider keys, and live metadata server reconfiguration — all without touching the server." />
        </div>
      </Section>

      {/* ── Chart types ── */}
      <Section id="charts" bg="white">
        <div style={{ textAlign: "center", marginBottom: 48 }}>
          <SectionLabel text="Visualisations" color="#059669" />
          <SectionHeading center>20+ chart types out of the box</SectionHeading>
          <SectionSub center>
            From simple bar charts to 3D globe maps powered by WebGL — all driven by Apache ECharts 5
            and configured through an intuitive point-and-click builder.
          </SectionSub>
        </div>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 10, justifyContent: "center" }}>
          {[
            ["fa-chart-bar",       "#2563eb", "Bar Chart"],
            ["fa-chart-line",      "#059669", "Line Chart"],
            ["fa-chart-area",      "#7c3aed", "Area Chart"],
            ["fa-chart-pie",       "#d97706", "Pie / Donut"],
            ["fa-circle-dot",      "#0ea5e9", "Scatter Plot"],
            ["fa-fire",            "#ef4444", "Heatmap"],
            ["fa-filter",          "#059669", "Funnel"],
            ["fa-tachometer-alt",  "#7c3aed", "Gauge"],
            ["fa-sitemap",         "#0ea5e9", "Treemap"],
            ["fa-water",           "#2563eb", "Waterfall"],
            ["fa-calendar",        "#d97706", "Calendar Heat"],
            ["fa-globe",           "#059669", "World Map Globe"],
            ["fa-table",           "#64748b", "Table"],
            ["fa-hashtag",         "#7c3aed", "KPI / Big Number"],
            ["fa-chart-column",    "#2563eb", "Grouped Bar"],
            ["fa-layer-group",     "#0ea5e9", "Stacked Bar"],
            ["fa-wind",            "#059669", "Candlestick"],
            ["fa-diagram-project", "#d97706", "Sankey Diagram"],
            ["fa-chart-gantt",     "#ef4444", "Box Plot"],
            ["fa-radar",           "#7c3aed", "Radar Chart"],
          ].map(([icon, color, label]) => (
            <ChartPill key={label as string} icon={icon as string} color={color as string} label={label as string} />
          ))}
        </div>

        {/* Advanced chart options */}
        <div style={{ marginTop: 48, display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))", gap: 16 }}>
          {[
            { icon: "fa-pen-ruler", color: "#2563eb", title: "Advanced Options", body: "Reference lines, goal markers, annotation layers, custom colour schemes, and axis configuration." },
            { icon: "fa-filter", color: "#059669", title: "Live Filter Dropdowns", body: "Filter values sourced directly from your data at query time — always up to date, never stale." },
            { icon: "fa-link", color: "#7c3aed", title: "Cross-Chart Filtering", body: "Click any bar, slice, or data point to instantly filter every connected chart on the dashboard." },
            { icon: "fa-expand", color: "#d97706", title: "Full-Screen Mode", body: "Expand any chart to full screen. Download as PNG or export the underlying data as CSV." },
          ].map(c => (
            <div key={c.title} style={{ background: "white", border: "1px solid #e5e7eb", borderRadius: 12, padding: "20px" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 8 }}>
                <i className={`fas ${c.icon}`} style={{ color: c.color, fontSize: 15 }} />
                <span style={{ fontWeight: 700, fontSize: 14, color: "#0f172a" }}>{c.title}</span>
              </div>
              <p style={{ margin: 0, fontSize: 13.5, color: "#64748b", lineHeight: 1.6 }}>{c.body}</p>
            </div>
          ))}
        </div>
      </Section>

      {/* ── Dashboard builder ── */}
      <Section id="dashboards" bg="#f8fafc">
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 60, alignItems: "center" }}>
          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
            {[
              { icon: "fa-grip-vertical", color: "#2563eb", title: "Drag-and-Drop Canvas", body: "Resize, rearrange, and nest chart tiles with react-grid-layout." },
              { icon: "fa-filter", color: "#059669", title: "Filter Bar", body: "Dashboard-level filters slice every chart simultaneously with a single selection." },
              { icon: "fa-file-alt", color: "#7c3aed", title: "Rich Text Blocks", body: "Full Markdown support with bold, italic, links, code blocks, and blockquotes." },
              { icon: "fa-tabs", color: "#d97706", title: "Tab Containers", body: "Group charts into tabbed views to keep dashboards clean and focused." },
              { icon: "fa-arrows-to-dot", color: "#dc2626", title: "Cross-Chart Filters", body: "Click any data point to filter all connected charts on the page instantly." },
              { icon: "fa-expand-alt", color: "#0ea5e9", title: "Full-Screen Charts", body: "Expand any chart tile for a focused deep-dive view without leaving the dashboard." },
            ].map(c => (
              <div key={c.title} style={{ background: "white", border: "1px solid #e5e7eb", borderRadius: 10, padding: "16px" }}>
                <i className={`fas ${c.icon}`} style={{ color: c.color, fontSize: 16, marginBottom: 8, display: "block" }} />
                <div style={{ fontWeight: 700, fontSize: 13, color: "#0f172a", marginBottom: 4 }}>{c.title}</div>
                <div style={{ fontSize: 12.5, color: "#64748b", lineHeight: 1.55 }}>{c.body}</div>
              </div>
            ))}
          </div>
          <div>
            <SectionLabel text="Dashboards" color="#d97706" />
            <SectionHeading>Compose stories from your data</SectionHeading>
            <p style={{ fontSize: 15.5, color: "#475569", lineHeight: 1.75 }}>
              The LooMX dashboard builder gives you a pixel-precise drag-and-drop canvas. Combine charts,
              KPI cards, rich text, headers, and tab containers into polished operational dashboards.
              Publish to your team, set visibility to internal, or keep it private while you build.
            </p>
          </div>
        </div>
      </Section>

      {/* ── Data Sources ── */}
      <Section id="datasources" bg="white">
        <div style={{ textAlign: "center", marginBottom: 48 }}>
          <SectionLabel text="Data Sources" color="#0ea5e9" />
          <SectionHeading center>Connect any modern data platform</SectionHeading>
          <SectionSub center>
            Register data sources from the UI — no .env changes, no API restarts.
            LooMX creates a dedicated connection pool for each source at registration time.
          </SectionSub>
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))", gap: 16 }}>
          <DbCard icon={<FabricSvg />} name="Microsoft Fabric SQL" color="#0E6961" />
          <DbCard icon={<AzureSvg />} name="Azure SQL Database" color="#0078D4" />
          <DbCard icon={<PgSvg />} name="PostgreSQL" color="#336791" badge="BETA" />
          <DbCard icon={<MySQLSvg />} name="MySQL / MariaDB" color="#4479A1" badge="BETA" />
          <DbCard icon={<TrinoSvg />} name="Trino" color="#DD1C1C" badge="BETA" />
          <DbCard icon={<StarRocksSvg />} name="StarRocks" color="#01808F" badge="BETA" />
        </div>

        <div style={{ marginTop: 40, display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: 20 }}>
          {[
            { icon: "fa-plug", color: "#0ea5e9", title: "Connection Testing", body: "Built-in connection tester at registration time — verify before you commit." },
            { icon: "fa-swimming-pool", color: "#059669", title: "Per-DB Connection Pool", body: "Each data source gets its own pyodbc pool, warmed at startup and kept alive by a 5-minute heartbeat." },
            { icon: "fa-lock", color: "#7c3aed", title: "No Credential Storage", body: "Fabric and Azure SQL use delegated Azure AD tokens. The connection string is stored encrypted and never returned to the browser." },
          ].map(c => (
            <div key={c.title} style={{ background: "#f8fafc", border: "1px solid #e5e7eb", borderRadius: 12, padding: "20px" }}>
              <i className={`fas ${c.icon}`} style={{ color: c.color, fontSize: 18, marginBottom: 10, display: "block" }} />
              <div style={{ fontWeight: 700, fontSize: 14, color: "#0f172a", marginBottom: 6 }}>{c.title}</div>
              <p style={{ margin: 0, fontSize: 13.5, color: "#64748b", lineHeight: 1.6 }}>{c.body}</p>
            </div>
          ))}
        </div>
      </Section>

      {/* ── SQL Lab deep-dive ── */}
      <Section id="sqllab" bg="#f8fafc">
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 60, alignItems: "center" }}>
          <div>
            <SectionLabel text="SQL Lab" color="#2563eb" />
            <SectionHeading>VS Code-grade SQL editor in the browser</SectionHeading>
            <p style={{ fontSize: 15.5, color: "#475569", lineHeight: 1.75, marginBottom: 28 }}>
              The SQL Lab is built on Monaco Editor — the same engine that powers VS Code. Write SQL with
              full syntax highlighting, run it against any registered data source, and see paginated results
              with sortable columns and full-text search — all in your browser.
            </p>
            <ul style={{ listStyle: "none", padding: 0, margin: 0, display: "flex", flexDirection: "column", gap: 12 }}>
              {[
                ["fa-columns", "#2563eb", "Multi-tab sessions — switch between queries without losing context"],
                ["fa-history", "#2563eb", "Full query history with execution time, source, and result counts"],
                ["fa-bookmark", "#2563eb", "Save queries to your personal library with name and description"],
                ["fa-wand-magic-sparkles", "#7c3aed", "Inline AI bar — describe what you want, get SQL instantly"],
                ["fa-download", "#059669", "Export results as CSV or copy directly to clipboard"],
              ].map(([icon, color, text]) => (
                <li key={text as string} style={{ display: "flex", alignItems: "flex-start", gap: 12 }}>
                  <div style={{ width: 28, height: 28, borderRadius: 7, background: `${color as string}12`, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0, marginTop: 1 }}>
                    <i className={`fas ${icon}`} style={{ color: color as string, fontSize: 12 }} />
                  </div>
                  <span style={{ fontSize: 14, color: "#334155", lineHeight: 1.6 }}>{text as string}</span>
                </li>
              ))}
            </ul>
          </div>
          <div style={{
            background: "#0f172a", borderRadius: 16, padding: "24px",
            boxShadow: "0 24px 60px rgba(0,0,0,0.25)",
            fontFamily: "monospace", fontSize: 13, lineHeight: 1.7, color: "#94a3b8",
          }}>
            <div style={{ display: "flex", gap: 7, marginBottom: 20 }}>
              {["#ef4444","#f59e0b","#22c55e"].map(c => (
                <div key={c} style={{ width: 12, height: 12, borderRadius: "50%", background: c }} />
              ))}
              <span style={{ marginLeft: 8, fontSize: 11, color: "#475569" }}>SQL Lab — Tab 1</span>
            </div>
            <div><span style={{ color: "#6366f1" }}>SELECT</span><span style={{ color: "#f1f5f9" }}> </span></div>
            <div style={{ paddingLeft: 16 }}><span style={{ color: "#94a3b8" }}>  customer_region,</span></div>
            <div style={{ paddingLeft: 16 }}><span style={{ color: "#94a3b8" }}>  product_category,</span></div>
            <div style={{ paddingLeft: 16 }}><span style={{ color: "#34d399" }}>  SUM</span><span style={{ color: "#f1f5f9" }}>(revenue) </span><span style={{ color: "#6366f1" }}>AS</span><span style={{ color: "#f1f5f9" }}> total_revenue,</span></div>
            <div style={{ paddingLeft: 16 }}><span style={{ color: "#34d399" }}>  COUNT</span><span style={{ color: "#f1f5f9" }}>(DISTINCT order_id) </span><span style={{ color: "#6366f1" }}>AS</span><span style={{ color: "#f1f5f9" }}> orders</span></div>
            <div><span style={{ color: "#6366f1" }}>FROM</span><span style={{ color: "#f1f5f9" }}> sales_fact sf</span></div>
            <div><span style={{ color: "#6366f1" }}>JOIN</span><span style={{ color: "#f1f5f9" }}> customers c </span><span style={{ color: "#6366f1" }}>ON</span><span style={{ color: "#f1f5f9" }}> sf.customer_id = c.id</span></div>
            <div><span style={{ color: "#6366f1" }}>WHERE</span><span style={{ color: "#f1f5f9" }}> sf.order_date </span><span style={{ color: "#f59e0b" }}>&gt;=</span><span style={{ color: "#a3e635" }}> &apos;2024-01-01&apos;</span></div>
            <div><span style={{ color: "#6366f1" }}>GROUP BY</span><span style={{ color: "#f1f5f9" }}> </span><span style={{ color: "#94a3b8" }}>1, 2</span></div>
            <div><span style={{ color: "#6366f1" }}>ORDER BY</span><span style={{ color: "#f1f5f9" }}> total_revenue </span><span style={{ color: "#6366f1" }}>DESC</span><span style={{ color: "#f1f5f9" }}>;</span></div>
            <div style={{ marginTop: 16, padding: "8px 12px", background: "#7c3aed18", borderRadius: 7, border: "1px solid #7c3aed30", fontSize: 12, color: "#a78bfa" }}>
              <i className="fas fa-wand-magic-sparkles" style={{ marginRight: 6 }} />
              AI: "Show me revenue and order counts by region and category for this year"
            </div>
          </div>
        </div>
      </Section>

      {/* ── AI ── */}
      <Section id="ai" bg="white">
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 60, alignItems: "center" }}>
          <div>
            <SectionLabel text="AI Assistant" color="#7c3aed" />
            <SectionHeading>Natural language to SQL,<br />right where you work</SectionHeading>
            <p style={{ fontSize: 15.5, color: "#475569", lineHeight: 1.75, marginBottom: 28 }}>
              The LooMX AI Assistant understands your data context — pass the active SQL, the data source,
              and a plain-English description. Get a production-ready query back in seconds.
            </p>
            <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
              {[
                { icon: "fa-comments", color: "#7c3aed", title: "Full conversation mode at /ai", body: "Multi-turn conversation with SQL context injection, explain query, and optimise query modes." },
                { icon: "fa-wand-magic-sparkles", color: "#7c3aed", title: "Inline AI bar in SQL Lab", body: "One click opens a prompt bar in the editor. Describe what you need, the SQL replaces the active tab content." },
                { icon: "fa-key", color: "#d97706", title: "Multi-provider key management", body: "Admins set global keys. Users can override with their own. Keys are AES-256 encrypted at rest." },
              ].map(c => (
                <div key={c.title} style={{ display: "flex", gap: 14 }}>
                  <div style={{ width: 36, height: 36, borderRadius: 9, background: `${c.color}12`, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0 }}>
                    <i className={`fas ${c.icon}`} style={{ color: c.color, fontSize: 14 }} />
                  </div>
                  <div>
                    <div style={{ fontWeight: 700, fontSize: 14, color: "#0f172a", marginBottom: 3 }}>{c.title}</div>
                    <div style={{ fontSize: 13.5, color: "#64748b", lineHeight: 1.55 }}>{c.body}</div>
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* AI provider cards */}
          <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
            {[
              { name: "Anthropic (Claude)", models: "claude-sonnet-4-6, claude-opus-4-6, claude-haiku-4-5", color: "#c17b3f", bg: "#fdf8f0", border: "#f0d5a8", icon: "A" },
              { name: "OpenAI", models: "gpt-4o, gpt-4o-mini, gpt-4-turbo", color: "#0d9488", bg: "#f0fdfa", border: "#99f6e4", icon: "O" },
              { name: "GitHub Models (Copilot)", models: "gpt-4o, claude-3-5-sonnet, mistral-large", color: "#0f172a", bg: "#f8fafc", border: "#cbd5e1", icon: "G" },
            ].map(p => (
              <div key={p.name} style={{
                background: p.bg, border: `1px solid ${p.border}`, borderRadius: 12, padding: "16px 18px",
                display: "flex", gap: 14, alignItems: "flex-start",
              }}>
                <div style={{ width: 36, height: 36, borderRadius: 9, background: p.color, display: "flex", alignItems: "center", justifyContent: "center", color: "white", fontWeight: 900, fontSize: 15, flexShrink: 0 }}>
                  {p.icon}
                </div>
                <div>
                  <div style={{ fontWeight: 700, fontSize: 14, color: p.color, marginBottom: 3 }}>{p.name}</div>
                  <div style={{ fontSize: 12, fontFamily: "monospace", color: "#64748b" }}>{p.models}</div>
                </div>
              </div>
            ))}
          </div>
        </div>
      </Section>

      {/* ── Security ── */}
      <Section id="security" bg="#f8fafc">
        <div style={{ textAlign: "center", marginBottom: 56 }}>
          <SectionLabel text="Security" color="#dc2626" />
          <SectionHeading center>Enterprise-grade security by design</SectionHeading>
          <SectionSub center>
            Multi-provider OAuth2 / OIDC authentication, cryptographic JWT verification, zero plaintext secrets,
            and role-based access control — all configurable from the UI with no server restart.
          </SectionSub>
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(300px, 1fr))", gap: 20 }}>
          {[
            { icon: "fa-key", color: "#dc2626", title: "Multi-Provider Auth", body: "Local login (bcrypt + HS256 JWT), Azure AD / Entra ID (OAuth2 OIDC, RS256 JWKS), and Google OAuth2 (RS256 JWKS). Switch providers live from Settings — no restart, no .env changes." },
            { icon: "fa-user-shield", color: "#7c3aed", title: "Cryptographic JWT Verification", body: "Azure AD and Google tokens are verified against live JWKS endpoints (RS256). Local tokens are HS256-signed with an auto-generated secret stored encrypted in the DB." },
            { icon: "fa-layer-group", color: "#2563eb", title: "4-Role RBAC", body: "Viewer → Analyst → Editor → Admin. Resolved from JWT claims first, DB assignments second. Content visibility (private / internal / published) enforced at query time." },
            { icon: "fa-shield-halved", color: "#059669", title: "Encrypted Secrets", body: "Google client secrets and JWT signing keys are encrypted at rest with Fernet (AES-128-CBC). Connection strings are stored encrypted and never returned to the browser." },
            { icon: "fa-lock", color: "#d97706", title: "Parameterised Queries", body: "All metadata DB operations use parameterised queries (@param0, @param1…). User data SQL passes through the ODBC driver without string interpolation." },
            { icon: "fa-ban", color: "#dc2626", title: "Security Headers", body: "Hardened CORS, X-Content-Type-Options, X-Frame-Options, Referrer-Policy, and Content-Security-Policy headers on all API responses." },
          ].map(c => (
            <div key={c.title} style={{ background: "#f8fafc", border: "1px solid #e5e7eb", borderRadius: 14, padding: "24px" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 12 }}>
                <div style={{ width: 38, height: 38, borderRadius: 9, background: `${c.color}12`, display: "flex", alignItems: "center", justifyContent: "center" }}>
                  <i className={`fas ${c.icon}`} style={{ color: c.color, fontSize: 16 }} />
                </div>
                <div style={{ fontWeight: 700, fontSize: 14.5, color: "#0f172a" }}>{c.title}</div>
              </div>
              <p style={{ margin: 0, fontSize: 13.5, color: "#475569", lineHeight: 1.65 }}>{c.body}</p>
            </div>
          ))}
        </div>
      </Section>

      {/* ── API Reference ── */}
      <Section id="api" bg="white">
        <div style={{ textAlign: "center", marginBottom: 48 }}>
          <SectionLabel text="API Reference" color="#0ea5e9" />
          <SectionHeading center>Full REST API — auto-documented with Swagger</SectionHeading>
          <SectionSub center>
            The FastAPI backend auto-generates interactive Swagger UI at{" "}
            <code style={{ fontFamily: "monospace", fontSize: 14, background: "#f1f5f9", padding: "2px 7px", borderRadius: 5 }}>
              http://localhost:8080/docs
            </code>
          </SectionSub>
        </div>

        {([
          {
            group: "Authentication", color: "#dc2626",
            rows: [
              { method: "GET",  path: "/api/auth/provider",                      desc: "Active auth provider (local / azure_ad / google)",  auth: "None"   },
              { method: "POST", path: "/api/auth/login",                          desc: "Local login — returns HS256 JWT",                   auth: "None"   },
              { method: "POST", path: "/api/auth/change-password",               desc: "Change own local password",                         auth: "User"   },
              { method: "GET",  path: "/api/v1/auth/me",                          desc: "Current user email and resolved role",              auth: "User"   },
              { method: "GET",  path: "/api/v1/auth/roles",                       desc: "List of all valid RBAC roles",                      auth: "User"   },
              { method: "GET",  path: "/api/v1/admin/auth",                       desc: "Get auth provider config (secrets masked)",         auth: "Admin"  },
              { method: "POST", path: "/api/v1/admin/auth",                       desc: "Update auth provider config",                       auth: "Admin"  },
              { method: "GET",  path: "/api/v1/admin/local-users",               desc: "List local users",                                  auth: "Admin"  },
              { method: "POST", path: "/api/v1/admin/local-users",               desc: "Create a local user",                               auth: "Admin"  },
              { method: "DELETE", path: "/api/v1/admin/local-users/{id}",        desc: "Deactivate a local user",                           auth: "Admin"  },
              { method: "POST", path: "/api/v1/admin/local-users/{id}/reset-password", desc: "Reset local user password",                  auth: "Admin"  },
            ],
          },
          {
            group: "Setup & Admin", color: "#7c3aed",
            rows: [
              { method: "GET",  path: "/api/v1/setup/status",          desc: "Is metadata DB configured?",          auth: "None"   },
              { method: "POST", path: "/api/v1/setup/test",             desc: "Test connection (setup mode only)",    auth: "None"   },
              { method: "POST", path: "/api/v1/setup/initialize",       desc: "Init schema + write .env",            auth: "None"   },
              { method: "GET",  path: "/api/v1/admin/metadata",         desc: "Get current metadata server config",  auth: "Admin"  },
              { method: "POST", path: "/api/v1/admin/metadata/test",    desc: "Test new metadata connection",        auth: "Admin"  },
              { method: "POST", path: "/api/v1/admin/metadata/update",  desc: "Reconfigure + restart API",           auth: "Admin"  },
            ],
          },
          {
            group: "Data Sources", color: "#0ea5e9",
            rows: [
              { method: "GET",    path: "/api/v1/data-sources",          desc: "List all with favourite flag",   auth: "User"   },
              { method: "POST",   path: "/api/v1/data-sources",          desc: "Create data source",             auth: "Admin"  },
              { method: "PATCH",  path: "/api/v1/data-sources/{id}",     desc: "Update data source",             auth: "Admin"  },
              { method: "DELETE", path: "/api/v1/data-sources/{id}",     desc: "Delete data source",             auth: "Admin"  },
            ],
          },
          {
            group: "Datasets", color: "#059669",
            rows: [
              { method: "GET",    path: "/api/v1/datasets",             desc: "List (filtered by visibility)", auth: "User"      },
              { method: "POST",   path: "/api/v1/datasets",             desc: "Create dataset",               auth: "Analyst+"  },
              { method: "PUT",    path: "/api/v1/datasets/{id}",        desc: "Update dataset",               auth: "Analyst+"  },
              { method: "DELETE", path: "/api/v1/datasets/{id}",        desc: "Delete dataset",               auth: "Editor+"   },
              { method: "POST",   path: "/api/v1/datasets/{id}/preview", desc: "Run preview query",           auth: "Analyst+"  },
            ],
          },
          {
            group: "Charts & Dashboards", color: "#d97706",
            rows: [
              { method: "GET",    path: "/api/v1/charts",          desc: "List charts",           auth: "User"      },
              { method: "POST",   path: "/api/v1/charts",          desc: "Create chart",          auth: "Analyst+"  },
              { method: "POST",   path: "/api/v1/charts/{id}/data", desc: "Run chart query",      auth: "User"      },
              { method: "GET",    path: "/api/v1/dashboards",      desc: "List dashboards",       auth: "User"      },
              { method: "POST",   path: "/api/v1/dashboards",      desc: "Create dashboard",      auth: "Analyst+"  },
              { method: "PUT",    path: "/api/v1/dashboards/{id}", desc: "Update / save layout",  auth: "Analyst+"  },
            ],
          },
          {
            group: "SQL Lab", color: "#2563eb",
            rows: [
              { method: "POST", path: "/api/v1/lab/execute",          desc: "Execute SQL + log to history", auth: "Analyst+"  },
              { method: "GET",  path: "/api/v1/lab/saved-queries",    desc: "List saved queries",           auth: "Analyst+"  },
              { method: "POST", path: "/api/v1/lab/saved-queries",    desc: "Save a query",                 auth: "Analyst+"  },
              { method: "GET",  path: "/api/v1/lab/history",          desc: "Full query history",           auth: "Analyst+"  },
              { method: "GET",  path: "/api/v1/sql/tables",           desc: "List tables in a database",    auth: "Analyst+"  },
              { method: "GET",  path: "/api/v1/sql/columns",          desc: "List columns for a table",     auth: "Analyst+"  },
            ],
          },
          {
            group: "AI", color: "#7c3aed",
            rows: [
              { method: "POST",   path: "/api/v1/ai/chat",            desc: "Send prompt with SQL/DS context",  auth: "Analyst+"  },
              { method: "GET",    path: "/api/v1/ai/providers",       desc: "List global AI providers",         auth: "User"      },
              { method: "POST",   path: "/api/v1/ai/providers",       desc: "Add global provider key",          auth: "Admin"     },
              { method: "DELETE", path: "/api/v1/ai/providers/{id}",  desc: "Remove global provider key",       auth: "Admin"     },
              { method: "PUT",    path: "/api/v1/ai/my-keys",         desc: "Set personal AI key",              auth: "User"      },
              { method: "DELETE", path: "/api/v1/ai/my-keys/{prov}",  desc: "Remove personal AI key",           auth: "User"      },
            ],
          },
          {
            group: "Users", color: "#dc2626",
            rows: [
              { method: "GET", path: "/api/v1/users/me", desc: "Current user's email + role", auth: "Any" },
            ],
          },
        ] as Array<{ group: string; color: string; rows: Array<{ method: string; path: string; desc: string; auth: string }> }>)
          .map(({ group, color, rows }) => (
            <div key={group} style={{ marginBottom: 32 }}>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                <div style={{ width: 10, height: 10, borderRadius: "50%", background: color }} />
                <span style={{ fontWeight: 700, fontSize: 14, color: "#0f172a" }}>{group}</span>
              </div>
              <div style={{ border: "1px solid #e5e7eb", borderRadius: 10, overflow: "hidden" }}>
                <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13 }}>
                  <thead>
                    <tr style={{ background: "#f8fafc" }}>
                      <th style={{ padding: "9px 14px", textAlign: "left", fontWeight: 700, color: "#64748b", fontSize: 11, letterSpacing: "0.05em", textTransform: "uppercase" }}>Method</th>
                      <th style={{ padding: "9px 14px", textAlign: "left", fontWeight: 700, color: "#64748b", fontSize: 11, letterSpacing: "0.05em", textTransform: "uppercase" }}>Endpoint</th>
                      <th style={{ padding: "9px 14px", textAlign: "left", fontWeight: 700, color: "#64748b", fontSize: 11, letterSpacing: "0.05em", textTransform: "uppercase" }}>Description</th>
                      <th style={{ padding: "9px 14px", textAlign: "left", fontWeight: 700, color: "#64748b", fontSize: 11, letterSpacing: "0.05em", textTransform: "uppercase" }}>Auth</th>
                    </tr>
                  </thead>
                  <tbody>
                    {rows.map(r => <ApiRow key={r.path + r.method} {...r} />)}
                  </tbody>
                </table>
              </div>
            </div>
          ))
        }
      </Section>

      {/* ── Tech stack ── */}
      <Section id="stack" bg="#f8fafc">
        <div style={{ textAlign: "center", marginBottom: 48 }}>
          <SectionLabel text="Tech Stack" color="#64748b" />
          <SectionHeading center>Built on battle-tested open-source foundations</SectionHeading>
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 24 }}>
          {[
            {
              title: "Frontend", icon: "fa-browser", color: "#2563eb",
              items: [
                ["Next.js 15", "React framework, App Router, SSR"],
                ["React 19", "UI component library"],
                ["TypeScript 5.x", "End-to-end type safety"],
                ["MSAL Browser 5.x", "Azure AD authentication (PKCE)"],
                ["Apache ECharts 5.x", "20+ chart types"],
                ["ECharts-GL 2.x", "3D WebGL globe visualisation"],
                ["Monaco Editor 0.52", "VS Code-grade SQL editor"],
                ["react-grid-layout 2.x", "Drag-and-drop dashboard canvas"],
              ],
            },
            {
              title: "Backend", icon: "fa-server", color: "#059669",
              items: [
                ["FastAPI 0.115+", "ASGI HTTP framework with DI"],
                ["Python 3.11+", "Runtime"],
                ["Uvicorn / Gunicorn", "ASGI server + process manager"],
                ["pyodbc 5.x", "ODBC Driver 18 for Fabric/Azure SQL"],
                ["azure-identity 1.x", "Managed Identity / DefaultAzureCredential"],
                ["PyJWT 2.x", "JWT RS256 verification"],
                ["cryptography 42+", "AES-256 encryption for AI keys"],
                ["pydantic-settings 2.x", "Typed config from environment"],
              ],
            },
          ].map(({ title, icon, color, items }) => (
            <div key={title} style={{ background: "white", border: "1px solid #e5e7eb", borderRadius: 14, padding: "24px" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 20 }}>
                <div style={{ width: 36, height: 36, borderRadius: 9, background: `${color}12`, display: "flex", alignItems: "center", justifyContent: "center" }}>
                  <i className={`fas ${icon}`} style={{ color, fontSize: 15 }} />
                </div>
                <span style={{ fontWeight: 800, fontSize: 16, color: "#0f172a" }}>{title}</span>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 0 }}>
                {items.map(([name, desc]) => (
                  <div key={name} style={{ display: "flex", alignItems: "baseline", gap: 10, padding: "9px 0", borderBottom: "1px solid #f1f5f9" }}>
                    <span style={{ fontWeight: 700, fontSize: 13, color: "#0f172a", minWidth: 180, flexShrink: 0 }}>{name}</span>
                    <span style={{ fontSize: 12.5, color: "#64748b" }}>{desc}</span>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </Section>

      {/* ── Getting Started ── */}
      <Section id="start" bg="white">
        <div style={{ textAlign: "center", marginBottom: 56 }}>
          <SectionLabel text="Getting Started" color="#2563eb" />
          <SectionHeading center>Up and running in under 10 minutes</SectionHeading>
          <SectionSub center>
            Four steps from zero to your first published dashboard.
            No CLI, no config files — everything happens in the UI.
          </SectionSub>
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(260px, 1fr))", gap: 24, counterReset: "steps" }}>
          {[
            {
              step: "01", color: "#2563eb", icon: "fa-sign-in-alt",
              title: "Sign In with Azure AD",
              body: "Open LooMX in your browser and click Sign In. Authenticate with your Microsoft work account — no separate username or password needed. Your Azure AD App Role determines your access level automatically.",
              links: [{ label: "Go to sign-in", href: "/" }],
            },
            {
              step: "02", color: "#0ea5e9", icon: "fa-database",
              title: "Register a Data Source",
              body: "Navigate to Settings → Data Sources and click Add Data Source. Choose your engine (Fabric SQL, Azure SQL, PostgreSQL, MySQL, Trino, or StarRocks), enter the connection details, and hit Test Connection. Save when green.",
              links: [{ label: "Data Sources →", href: "/data-sources" }],
            },
            {
              step: "03", color: "#059669", icon: "fa-layer-group",
              title: "Create a Dataset",
              body: "Go to Datasets → New Dataset. Pick your data source and table (or write a custom SQL query as a virtual dataset). Define which columns are dimensions, metrics, and filters. Save — every chart built on this dataset stays in sync automatically.",
              links: [{ label: "Datasets →", href: "/datasets" }],
            },
            {
              step: "04", color: "#d97706", icon: "fa-tachometer-alt",
              title: "Build a Chart & Publish",
              body: "Open Charts → New Chart, pick your dataset and chart type, configure axes and metrics in the builder, then Save. Head to Dashboards → New Dashboard, drag your chart onto the canvas, arrange tiles, add filters, and Publish.",
              links: [{ label: "Charts →", href: "/charts" }, { label: "Dashboards →", href: "/dashboards" }],
            },
          ].map(({ step, color, icon, title, body, links }) => (
            <div key={step} style={{ background: "#f8fafc", border: "1px solid #e5e7eb", borderRadius: 16, padding: "28px 24px", position: "relative", overflow: "hidden" }}>
              <div style={{ position: "absolute", top: 16, right: 20, fontSize: 48, fontWeight: 900, color: `${color}0d`, lineHeight: 1, userSelect: "none" }}>{step}</div>
              <div style={{ width: 44, height: 44, borderRadius: 12, background: `${color}12`, display: "flex", alignItems: "center", justifyContent: "center", marginBottom: 16 }}>
                <i className={`fas ${icon}`} style={{ color, fontSize: 18 }} />
              </div>
              <div style={{ fontWeight: 800, fontSize: 15, color: "#0f172a", marginBottom: 10 }}>{title}</div>
              <p style={{ margin: "0 0 16px", fontSize: 13.5, color: "#475569", lineHeight: 1.7 }}>{body}</p>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                {links.map(l => (
                  <Link key={l.href} href={l.href} style={{
                    fontSize: 12.5, fontWeight: 600, color, textDecoration: "none",
                    padding: "4px 10px", borderRadius: 6, background: `${color}10`,
                    border: `1px solid ${color}20`,
                  }}>{l.label}</Link>
                ))}
              </div>
            </div>
          ))}
        </div>

        {/* Admin setup callout */}
        {isAdmin && (
          <div style={{ marginTop: 40, background: "#f0fdf4", border: "1px solid #bbf7d0", borderRadius: 14, padding: "24px 28px", display: "flex", gap: 20, alignItems: "flex-start" }}>
            <div style={{ width: 40, height: 40, borderRadius: 10, background: "#dcfce7", display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0 }}>
              <i className="fas fa-shield-alt" style={{ color: "#059669", fontSize: 16 }} />
            </div>
            <div>
              <div style={{ fontWeight: 700, fontSize: 14, color: "#166534", marginBottom: 6 }}>Admin first-run checklist</div>
              <ul style={{ margin: 0, padding: "0 0 0 16px", fontSize: 13.5, color: "#166534", lineHeight: 2 }}>
                <li>Settings → <Link href="/settings/metadata" style={{ color: "#059669" }}>Metadata Server</Link> — confirm the metadata DB is connected and schema is initialised</li>
                <li>Settings → <Link href="/data-sources" style={{ color: "#059669" }}>Data Sources</Link> — register at least one data source for your team</li>
                <li>Settings → <Link href="/settings/ai" style={{ color: "#059669" }}>AI Providers</Link> — add an API key to enable the AI Assistant for all users</li>
                <li>Azure AD → Enterprise Applications → LoomX → Users and groups — assign Azure AD App Roles (Viewer / Analyst / Editor / Admin) to team members</li>
              </ul>
            </div>
          </div>
        )}
      </Section>

      {/* ── What's New ── */}
      <Section id="whatsnew" bg="#f8fafc">
        <div style={{ textAlign: "center", marginBottom: 56 }}>
          <SectionLabel text="What's New" color="#7c3aed" />
          <SectionHeading center>Recent releases &amp; highlights</SectionHeading>
          <SectionSub center>LooMX ships improvements continuously. Here are the most recent highlights.</SectionSub>
        </div>
        <div style={{ maxWidth: 760, margin: "0 auto", display: "flex", flexDirection: "column", gap: 0 }}>
          {[
            {
              version: "v2.4", date: "Mar 2025", color: "#7c3aed",
              badge: "Latest",
              items: [
                "World Map Globe — 3D WebGL country-level heatmap powered by ECharts-GL",
                "Trino & StarRocks data source support (Beta)",
                "Inline AI bar in SQL Lab — natural language to SQL without leaving the editor",
                "Dashboard cross-chart filtering — click any data point to filter the whole page",
                "Admin metadata server reconfiguration from the UI — no server restart needed",
              ],
            },
            {
              version: "v2.3", date: "Feb 2025", color: "#2563eb",
              badge: null,
              items: [
                "PostgreSQL & MySQL / MariaDB data source support (Beta)",
                "Real brand SVG icons for all data source types",
                "AI Providers settings page with per-user key override",
                "Workspace Activity feed on the home page",
                "Saved queries page with full history and audit trail",
              ],
            },
            {
              version: "v2.2", date: "Jan 2025", color: "#059669",
              badge: null,
              items: [
                "Dashboard filter bar with live dropdowns sourced from data",
                "Semantic datasets — define dimensions & metrics once, reuse everywhere",
                "Role-based content visibility (private / internal / published)",
                "Setup wizard for first-run metadata database configuration",
                "Full API reference and About page",
              ],
            },
            {
              version: "v2.1", date: "Dec 2024", color: "#d97706",
              badge: null,
              items: [
                "Monaco Editor (VS Code engine) for SQL Lab",
                "20+ chart types via Apache ECharts 5",
                "Azure AD JWT RS256 authentication with PKCE flow",
                "4-role RBAC: Viewer, Analyst, Editor, Admin",
                "Fabric SQL & Azure SQL delegated token authentication",
              ],
            },
          ].map(({ version, date, color, badge, items }, i, arr) => (
            <div key={version} style={{ display: "flex", gap: 24 }}>
              {/* Timeline spine */}
              <div style={{ display: "flex", flexDirection: "column", alignItems: "center", width: 40, flexShrink: 0 }}>
                <div style={{ width: 14, height: 14, borderRadius: "50%", background: color, border: "3px solid white", boxShadow: `0 0 0 3px ${color}30`, marginTop: 4, flexShrink: 0 }} />
                {i < arr.length - 1 && <div style={{ width: 2, flex: 1, background: "#e5e7eb", marginTop: 4 }} />}
              </div>
              {/* Content */}
              <div style={{ paddingBottom: i < arr.length - 1 ? 40 : 0, flex: 1 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 12 }}>
                  <span style={{ fontWeight: 800, fontSize: 16, color: "#0f172a" }}>{version}</span>
                  <span style={{ fontSize: 12, color: "#94a3b8" }}>{date}</span>
                  {badge && (
                    <span style={{ fontSize: 11, fontWeight: 700, padding: "2px 8px", borderRadius: 8, background: `${color}15`, color, border: `1px solid ${color}30` }}>{badge}</span>
                  )}
                </div>
                <ul style={{ margin: 0, padding: "0 0 0 16px", display: "flex", flexDirection: "column", gap: 6 }}>
                  {items.map(item => (
                    <li key={item} style={{ fontSize: 13.5, color: "#475569", lineHeight: 1.6 }}>{item}</li>
                  ))}
                </ul>
              </div>
            </div>
          ))}
        </div>
      </Section>

      {/* ── FAQ ── */}
      <Section id="faq" bg="white">
        <div style={{ textAlign: "center", marginBottom: 56 }}>
          <SectionLabel text="FAQ" color="#0ea5e9" />
          <SectionHeading center>Frequently asked questions</SectionHeading>
          <SectionSub center>Common questions from users and admins. Can&apos;t find your answer? Open SQL Lab and ask the AI.</SectionSub>
        </div>
        <div style={{ maxWidth: 820, margin: "0 auto", display: "grid", gridTemplateColumns: "1fr 1fr", gap: 20 }}>
          {[
            {
              q: "Do I need an Azure subscription?",
              a: "Yes — LooMX uses Azure AD for authentication. Your organisation needs an Azure AD tenant and an App Registration with the four App Roles defined. The data sources themselves can be on any supported engine.",
            },
            {
              q: "Which browsers are supported?",
              a: "Any modern Chromium-based browser (Chrome, Edge, Arc) or Firefox. The 3D globe visualisation requires WebGL 2.0 support. Safari has limited WebGL support and may show a fallback.",
            },
            {
              q: "Can I use my own AI API key?",
              a: "Yes. Admins set a global key that applies to all users. Any user can override it with their own key in the AI Providers settings. Keys are AES-256 encrypted at rest and never returned to the browser.",
            },
            {
              q: "How do I add a new team member?",
              a: "Assign the appropriate Azure AD App Role (Viewer / Analyst / Editor / Admin) to the user in Azure portal → Enterprise Applications → LoomX → Users and groups. They can sign in immediately.",
            },
            {
              q: "What happens if the metadata DB goes down?",
              a: "The LooMX API will return 503 for endpoints that require metadata. The UI shows a connection error. Existing browser sessions are preserved — as soon as the DB recovers, the API reconnects automatically.",
            },
            {
              q: "Can I connect multiple data sources of the same type?",
              a: "Yes. Each data source registration is independent with its own connection pool. You can have multiple Fabric SQL databases, multiple PostgreSQL instances, and so on — all available from a single LooMX instance.",
            },
            {
              q: "How are queries executed? Does LooMX cache results?",
              a: "Queries run on-demand through the registered ODBC connection pool directly against your data source. There is no result cache by default — each chart load executes a fresh query to ensure data is always live.",
            },
            {
              q: "Can I self-host without Azure AD?",
              a: "Not currently. Azure AD is the only supported identity provider. Support for OIDC-compatible providers (Okta, Auth0, Entra External ID) is on the roadmap.",
            },
            {
              q: "What SQL dialects are supported?",
              a: "Each data source uses its native dialect — T-SQL for Fabric / Azure SQL, standard SQL for PostgreSQL and MySQL, Trino SQL for Trino, and StarRocks SQL for StarRocks. LooMX passes queries through without rewriting.",
            },
            {
              q: "How do I back up my dashboards and charts?",
              a: "All metadata (dashboards, charts, datasets, users, saved queries) is stored in the configured metadata database. Back up that database using your standard DB backup process. No additional export step is needed.",
            },
          ].map(({ q, a }) => (
            <div key={q} style={{ background: "#f8fafc", border: "1px solid #e5e7eb", borderRadius: 14, padding: "22px 24px" }}>
              <div style={{ fontWeight: 700, fontSize: 14, color: "#0f172a", marginBottom: 8, display: "flex", gap: 8 }}>
                <span style={{ color: "#0ea5e9", flexShrink: 0 }}>Q.</span>
                <span>{q}</span>
              </div>
              <div style={{ fontSize: 13.5, color: "#475569", lineHeight: 1.7, paddingLeft: 22 }}>{a}</div>
            </div>
          ))}
        </div>
      </Section>

      {/* ── Getting started CTA ── */}
      <Section bg="linear-gradient(135deg, #0f172a 0%, #1e293b 95%, #1e293b 100%)"  >
        <div style={{ textAlign: "center" }}>
          <h2 style={{ fontSize: "clamp(1.75rem, 3vw, 2.5rem)", fontWeight: 900, color: "white", marginBottom: 16 }}>
            Ready to explore your data?
          </h2>
          <p style={{ fontSize: 17, color: "#94a3b8", lineHeight: 1.65, maxWidth: 520, margin: "0 auto 36px" }}>
            Sign in with your Azure AD account to get started. The setup wizard will guide you through
            connecting your first data source in under five minutes.
          </p>
          <div style={{ display: "flex", gap: 14, justifyContent: "center", flexWrap: "wrap" }}>
            <Link href="/" style={{
              padding: "13px 32px", borderRadius: 10, fontSize: 15, fontWeight: 700,
              background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
              color: "white", textDecoration: "none",
              boxShadow: `0 4px 18px ${primaryColor}40`,
            }}>
              <i className="fas fa-sign-in-alt" style={{ marginRight: 8 }} />Sign In with Microsoft
            </Link>
            <Link href="/lab" style={{
              padding: "13px 28px", borderRadius: 10, fontSize: 15, fontWeight: 600,
              background: "rgba(255,255,255,0.08)", border: "1px solid rgba(255,255,255,0.15)",
              color: "white", textDecoration: "none",
            }}>
              <i className="fas fa-flask" style={{ marginRight: 8 }} />Open SQL Lab
            </Link>
          </div>

          {/* Navigation grid */}
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))", gap: 12, maxWidth: 860, margin: "48px auto 0" }}>
            {[
              { href: "/dashboards", icon: "fa-tachometer-alt", label: "Dashboards" },
              { href: "/charts",     icon: "fa-chart-bar",      label: "Charts" },
              { href: "/datasets",   icon: "fa-layer-group",    label: "Datasets" },
              { href: "/lab",        icon: "fa-flask",          label: "SQL Lab" },
              { href: "/ai",         icon: "fa-magic",          label: "AI Assistant" },
              { href: "/data-sources", icon: "fa-database",    label: "Data Sources" },
              { href: "/settings/ai",  icon: "fa-key",         label: "AI Providers" },
              ...(isAdmin ? [
                { href: "/settings/metadata", icon: "fa-database",   label: "Metadata Server" },
              ] : []),
            ].map(({ href, icon, label }) => (
              <Link key={href} href={href} style={{
                display: "flex", alignItems: "center", gap: 9,
                padding: "10px 14px", borderRadius: 9,
                background: "rgba(255,255,255,0.06)", border: "1px solid rgba(255,255,255,0.1)",
                color: "#cbd5e1", textDecoration: "none", fontSize: 13, fontWeight: 500,
                transition: "background 0.15s",
              }}
                onMouseEnter={e => { (e.currentTarget as HTMLAnchorElement).style.background = "rgba(255,255,255,0.12)"; }}
                onMouseLeave={e => { (e.currentTarget as HTMLAnchorElement).style.background = "rgba(255,255,255,0.06)"; }}
              >
                <i className={`fas ${icon}`} style={{ fontSize: 12, color: primaryColor }} />
                {label}
              </Link>
            ))}
          </div>
        </div>
      </Section>

      {/* ── Footer ── */}
      <div style={{
        background: "#0f172a", borderTop: "1px solid #1e293b",
        padding: "28px 24px", textAlign: "center",
      }}>
        <div style={{ fontSize: 13, color: "#475569" }}>
          <strong style={{ color: "#94a3b8" }}>LooMX</strong> — Live Operational Outcomes &amp; Metrics eXperience
          {" · "}Built for Advanced Analytics. Secured by Azure AD.
          {" · "}Owned by Pruthvi Prodduturi.
        </div>
      </div>
    </div>
  );
}
