"use client";

import React, { useEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { KaveonMark } from "../KaveonMark";

type NavItem = { title: string; href?: string };
type NavGroup = { label: string; items: NavItem[] };

const NAV: NavGroup[] = [
  {
    label: "Getting Started",
    items: [
      { title: "Introduction", href: "/docs" },
      { title: "Quickstart", href: "/docs/quickstart" },
      { title: "Core concepts", href: "/docs/concepts" }, // soon
    ],
  },
  {
    label: "Features",
    items: [
      { title: "SQL Lab", href: "/docs/sql-lab" },
      { title: "AI · NL→SQL", href: "/docs/nl-to-sql" },
      { title: "Data Language Model", href: "/docs/dlm" },
      { title: "Freshness Algorithm", href: "/docs/freshness" },
      { title: "Chart Builder", href: "/docs/charts" }, // soon
      { title: "Dashboards", href: "/docs/dashboards" }, // soon
      { title: "Semantic Datasets", href: "/docs/datasets" }, // soon
      { title: "Data Sources", href: "/docs/data-sources" }, // soon
    ],
  },
  {
    label: "Platform",
    items: [
      { title: "Architecture", href: "/docs/architecture" }, // soon
      { title: "Auth & RBAC", href: "/docs/auth" }, // soon
      { title: "Deployment", href: "/docs/deployment" }, // soon
    ],
  },
];

// Pages that actually exist yet — everything else renders as "soon".
const BUILT = new Set([
  "/docs", "/docs/quickstart", "/docs/concepts",
  "/docs/sql-lab", "/docs/nl-to-sql", "/docs/dlm", "/docs/freshness",
  "/docs/charts", "/docs/dashboards",
  "/docs/datasets", "/docs/data-sources",
  "/docs/architecture", "/docs/auth", "/docs/deployment",
]);

function slugify(s: string) {
  return s.toLowerCase().trim().replace(/[^a-z0-9\s-]/g, "").replace(/\s+/g, "-");
}

export function DocsShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  const contentRef = useRef<HTMLDivElement>(null);
  const [toc, setToc] = useState<{ id: string; text: string; level: number }[]>([]);
  const [activeId, setActiveId] = useState<string>("");
  const [q, setQ] = useState("");

  // Build the on-page TOC from the rendered headings, and keep it in sync with scroll.
  useEffect(() => {
    const root = contentRef.current;
    if (!root) return;
    const heads = Array.from(root.querySelectorAll<HTMLHeadingElement>(".docs-prose h2, .docs-prose h3"));
    const items = heads.map((h) => {
      if (!h.id) h.id = slugify(h.textContent || "");
      return { id: h.id, text: h.textContent || "", level: h.tagName === "H3" ? 3 : 2 };
    });
    setToc(items);
    if (items[0]) setActiveId(items[0].id);

    const obs = new IntersectionObserver(
      (entries) => {
        const visible = entries.filter((e) => e.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
        if (visible[0]) setActiveId((visible[0].target as HTMLElement).id);
      },
      { rootMargin: "0px 0px -70% 0px", threshold: 0 }
    );
    heads.forEach((h) => obs.observe(h));
    return () => obs.disconnect();
  }, [pathname]);

  const filtered = useMemo(() => {
    if (!q.trim()) return NAV;
    const t = q.toLowerCase();
    return NAV.map((g) => ({ ...g, items: g.items.filter((i) => i.title.toLowerCase().includes(t)) }))
      .filter((g) => g.items.length);
  }, [q]);

  return (
    <div className="docs-root">
      <div className="docs-grid">
        {/* Sidebar */}
        <aside className="docs-sidebar">
          <Link href="/" className="docs-brand">
            <svg width="100" height="18" viewBox="60 50 1180 200" fill="none" xmlns="http://www.w3.org/2000/svg" style={{ shapeRendering: "geometricPrecision" }}>
              <g fill="var(--text-primary, #e2e8f0)">
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
            <span className="docs-brand-badge">Docs</span>
          </Link>
          <input
            className="docs-search"
            placeholder="Search docs…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            aria-label="Search documentation"
          />
          <nav>
            {filtered.map((group) => (
              <div className="docs-navgroup" key={group.label}>
                <div className="docs-navgroup-label">{group.label}</div>
                {group.items.map((item) => {
                  const built = item.href && BUILT.has(item.href);
                  const active = item.href === pathname;
                  if (!built) {
                    return (
                      <span className="docs-navlink soon" key={item.title}>{item.title}</span>
                    );
                  }
                  return (
                    <Link
                      key={item.title}
                      href={item.href!}
                      className={"docs-navlink" + (active ? " active" : "")}
                    >
                      {item.title}
                    </Link>
                  );
                })}
              </div>
            ))}
          </nav>
        </aside>

        {/* Content */}
        <main className="docs-main">
          <div className="docs-content" ref={contentRef}>
            {children}
          </div>
        </main>

        {/* On-page TOC */}
        <aside className="docs-toc">
          {toc.length > 0 && (
            <>
              <div className="docs-toc-label">On this page</div>
              {toc.map((t) => (
                <a
                  key={t.id}
                  href={`#${t.id}`}
                  className={(t.level === 3 ? "lvl3" : "") + (activeId === t.id ? " active" : "")}
                >
                  {t.text}
                </a>
              ))}
            </>
          )}
        </aside>
      </div>
    </div>
  );
}
