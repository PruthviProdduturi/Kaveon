"use client";

import React, { useEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { DOCS_ITEMS, DOCS_NAV, docsMeta } from "./manifest";

type Section = { title: string; page: string; href: string; text: string };
const slugify = (s: string) => s.toLowerCase().trim().replace(/[^a-z0-9\s-]/g, "").replace(/\s+/g, "-");

async function buildIndex(signal: AbortSignal): Promise<Section[]> {
  const pages = await Promise.all(DOCS_ITEMS.map(async (item) => {
    const response = await fetch(item.href, { signal });
    if (!response.ok) return [];
    const documentNode = new DOMParser().parseFromString(await response.text(), "text/html");
    const root = documentNode.querySelector(".docs-prose");
    if (!root) return [];
    return Array.from(root.querySelectorAll<HTMLElement>("h1, h2, h3")).map((heading) => {
      const parts = [heading.textContent ?? ""];
      let node = heading.nextElementSibling;
      while (node && !/^H[1-3]$/.test(node.tagName)) { parts.push(node.textContent ?? ""); node = node.nextElementSibling; }
      const id = heading.id || slugify(heading.textContent ?? "");
      return { title: heading.textContent ?? item.title, page: item.title, href: `${item.href}${id ? `#${id}` : ""}`, text: parts.join(" ") };
    });
  }));
  return pages.flat();
}

function excerpt(text: string, query: string) {
  const clean = text.replace(/\s+/g, " ").trim();
  const match = clean.toLowerCase().indexOf(query.toLowerCase());
  const start = Math.max(0, match < 0 ? 0 : match - 65);
  const end = Math.min(clean.length, start + 180);
  return `${start ? "…" : ""}${clean.slice(start, end)}${end < clean.length ? "…" : ""}`;
}

export function DocsShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  const contentRef = useRef<HTMLDivElement>(null);
  const indexRef = useRef<Section[]>([]);
  const [toc, setToc] = useState<{ id: string; text: string; level: number }[]>([]);
  const [activeId, setActiveId] = useState("");
  const [query, setQuery] = useState("");
  const [indexState, setIndexState] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [mobileOpen, setMobileOpen] = useState(false);
  const meta = docsMeta(pathname);

  useEffect(() => {
    const root = contentRef.current;
    if (!root) return;
    const headings = Array.from(root.querySelectorAll<HTMLHeadingElement>(".docs-prose h2, .docs-prose h3"));
    const anchors: HTMLAnchorElement[] = [];
    const items = headings.map((heading) => {
      if (!heading.id) heading.id = slugify(heading.textContent || "");
      const anchor = document.createElement("a");
      anchor.className = "docs-heading-anchor"; anchor.href = `#${heading.id}`; anchor.textContent = "#";
      anchor.setAttribute("aria-label", `Link to ${heading.textContent}`); heading.append(anchor); anchors.push(anchor);
      return { id: heading.id, text: heading.childNodes[0]?.textContent || "", level: heading.tagName === "H3" ? 3 : 2 };
    });
    setToc(items); setActiveId(items[0]?.id ?? "");
    const observer = new IntersectionObserver((entries) => {
      const visible = entries.filter((entry) => entry.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
      if (visible[0]) setActiveId((visible[0].target as HTMLElement).id);
    }, { rootMargin: "0px 0px -70% 0px" });
    headings.forEach((heading) => observer.observe(heading));
    return () => { observer.disconnect(); anchors.forEach((anchor) => anchor.remove()); };
  }, [pathname]);

  useEffect(() => {
    if (!query.trim() || indexState !== "idle") return;
    const controller = new AbortController(); setIndexState("loading");
    buildIndex(controller.signal).then((index) => { indexRef.current = index; setIndexState("ready"); })
      .catch((error: unknown) => { if (!(error instanceof DOMException && error.name === "AbortError")) setIndexState("error"); });
  }, [query, indexState]);

  useEffect(() => { setMobileOpen(false); setQuery(""); }, [pathname]);

  const results = useMemo(() => {
    const term = query.trim().toLowerCase(); if (!term || indexState !== "ready") return [];
    const tokens = term.split(/\s+/);
    return indexRef.current.map((section) => {
      const title = section.title.toLowerCase(), page = section.page.toLowerCase(), all = `${page} ${title} ${section.text}`.toLowerCase();
      return { ...section, score: tokens.reduce((n, token) => n + (title.includes(token) ? 5 : 0) + (page.includes(token) ? 3 : 0) + (all.includes(token) ? 1 : 0), 0) };
    }).filter((result) => result.score >= tokens.length).sort((a, b) => b.score - a.score).slice(0, 10);
  }, [query, indexState]);

  const Search = ({ mobile = false }: { mobile?: boolean }) => <div className="docs-search-wrap">
    <label className="docs-visually-hidden" htmlFor={mobile ? "docs-search-mobile" : "docs-search"}>Search all documentation</label>
    <input id={mobile ? "docs-search-mobile" : "docs-search"} className="docs-search" type="search" placeholder="Search all documentation…" value={query} onChange={(event) => setQuery(event.target.value)} autoComplete="off" />
    {query && <div className="docs-search-results" role="region" aria-label="Search results" aria-live="polite">
      {indexState === "loading" && <div className="docs-search-state">Indexing documentation…</div>}
      {indexState === "error" && <div className="docs-search-state">Search is temporarily unavailable.</div>}
      {indexState === "ready" && !results.length && <div className="docs-search-state">No matching sections</div>}
      {results.map((result) => <Link key={result.href} href={result.href} className="docs-search-result"><span>{result.title}</span><small>{result.page}</small><p>{excerpt(result.text, query)}</p></Link>)}
    </div>}
  </div>;

  const Nav = () => <nav aria-label="Documentation">{DOCS_NAV.map((group) => <div className="docs-navgroup" key={group.label}><div className="docs-navgroup-label">{group.label}</div>{group.items.map((item) => <Link key={item.href} href={item.href} title={item.description} aria-current={item.href === pathname ? "page" : undefined} className={`docs-navlink${item.href === pathname ? " active" : ""}`}>{item.title}</Link>)}</div>)}</nav>;

  return <div className="docs-root">
    <a className="docs-skip" href="#docs-content">Skip to documentation</a>
    <header className="docs-mobile-header"><Link href="/docs" className="docs-section-title">Documentation</Link><button type="button" aria-expanded={mobileOpen} aria-controls="docs-mobile-panel" onClick={() => setMobileOpen((open) => !open)}>{mobileOpen ? "Close" : "Menu"}</button></header>
    {mobileOpen && <div className="docs-mobile-panel" id="docs-mobile-panel"><Search mobile /><Nav /></div>}
    <div className="docs-grid">
      <aside className="docs-sidebar"><Link href="/docs" className="docs-section-title">Documentation</Link><Search /><Nav /></aside>
      <main className="docs-main" id="docs-content">
        {meta && <div className="docs-context"><nav className="docs-breadcrumb" aria-label="Breadcrumb"><Link href="/docs">Docs</Link><span aria-hidden="true">/</span><span>{meta.group}</span><span aria-hidden="true">/</span><span aria-current="page">{meta.title}</span></nav><div className="docs-page-meta"><span className={`status ${meta.status.toLowerCase()}`}>{meta.status}</span><span>Verified {meta.lastVerified}</span></div></div>}
        <div className="docs-content" ref={contentRef}>{children}</div>
      </main>
      <aside className="docs-toc" aria-label="On this page">{toc.length > 0 && <><div className="docs-toc-label">On this page</div>{toc.map((item) => <a key={item.id} href={`#${item.id}`} className={`${item.level === 3 ? "lvl3" : ""}${activeId === item.id ? " active" : ""}`}>{item.text}</a>)}</>}</aside>
    </div>
  </div>;
}
