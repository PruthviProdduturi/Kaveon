import React from "react";
import Link from "next/link";
import { CopyCode } from "./CopyCode";

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
      <CopyCode value={children} />
      <pre><code>{children}</code></pre>
    </div>
  );
}

export function Diagram({ src, alt, caption }: { src: string; alt: string; caption: React.ReactNode }) {
  return (
    <figure className="docs-diagram">
      <a href={src} target="_blank" rel="noopener noreferrer" aria-label={`${alt} — open full-size diagram`}>
        {/* Architecture assets are authored SVGs whose intrinsic geometry must remain intact. */}
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={src} alt={alt} />
      </a>
      <figcaption>{caption}</figcaption>
    </figure>
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
