import React from "react";
import Link from "next/link";

export function PageHeader({ eyebrow, title, lead }: { eyebrow?: string; title: string; lead?: string }) {
  return (
    <header>
      {eyebrow && <div className="docs-eyebrow">{eyebrow}</div>}
      <h1>{title}</h1>
      {lead && <p className="docs-lead">{lead}</p>}
    </header>
  );
}

export function Callout({ type = "note", children }: { type?: "tip" | "note" | "warn"; children: React.ReactNode }) {
  const ico = type === "tip" ? "✓" : type === "warn" ? "!" : "i";
  return (
    <div className={`docs-callout ${type}`}>
      <span className="ico" aria-hidden="true">{ico}</span>
      <div>{children}</div>
    </div>
  );
}

export function Code({ lang, children }: { lang?: string; children: string }) {
  return (
    <div className="docs-code">
      {lang && <span className="docs-code-lang">{lang}</span>}
      <pre><code>{children}</code></pre>
    </div>
  );
}

export function Pager({
  prev,
  next,
}: {
  prev?: { href: string; title: string };
  next?: { href: string; title: string };
}) {
  return (
    <nav className="docs-pager">
      {prev ? (
        <Link href={prev.href}>
          <div className="dir">← Previous</div>
          <div className="ttl">{prev.title}</div>
        </Link>
      ) : <span style={{ flex: 1 }} />}
      {next ? (
        <Link href={next.href} className="next">
          <div className="dir">Next →</div>
          <div className="ttl">{next.title}</div>
        </Link>
      ) : <span style={{ flex: 1 }} />}
    </nav>
  );
}
