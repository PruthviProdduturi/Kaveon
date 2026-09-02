"use client";

import React, { useEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { KaveonWordmark } from "../KaveonMark";
import { DOCS_NAV } from "./manifest";

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
    if (!q.trim()) return DOCS_NAV;
    const t = q.toLowerCase();
    return DOCS_NAV.map((g) => ({
      ...g,
      items: g.items.filter((i) =>
        [i.title, i.description, ...(i.keywords ?? [])].join(" ").toLowerCase().includes(t)
      ),
    }))
      .filter((g) => g.items.length);
  }, [q]);

  return (
    <div className="docs-root">
      <a className="docs-skip" href="#docs-content">Skip to documentation</a>
      <div className="docs-grid">
        {/* Sidebar */}
        <aside className="docs-sidebar">
          <Link href="/" className="docs-brand">
            <KaveonWordmark height={18} />
            <span className="docs-brand-badge">Docs</span>
          </Link>
          <input
            className="docs-search"
            placeholder="Filter documentation…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            aria-label="Search documentation"
          />
          <nav>
            {filtered.map((group) => (
              <div className="docs-navgroup" key={group.label}>
                <div className="docs-navgroup-label">{group.label}</div>
                {group.items.map((item) => {
                  const active = item.href === pathname;
                  return (
                    <Link
                      key={item.title}
                      href={item.href}
                      title={item.description}
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
        <main className="docs-main" id="docs-content">
          <details className="docs-mobile-nav">
            <summary>Documentation menu</summary>
            <nav>
              {DOCS_NAV.map((group) => (
                <div className="docs-navgroup" key={group.label}>
                  <div className="docs-navgroup-label">{group.label}</div>
                  {group.items.map((item) => <Link key={item.href} href={item.href} className={"docs-navlink" + (item.href === pathname ? " active" : "")}>{item.title}</Link>)}
                </div>
              ))}
            </nav>
          </details>
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
