import type { Metadata } from "next";
import { DocsShell } from "../../components/docs/DocsShell";
import { PublicHeader } from "../../components/PublicHeader";
import "./docs.css";
import "./visual-polish.css";

export const metadata: Metadata = {
  title: { default: "Kaveon Docs", template: "%s — Kaveon Docs" },
  description: "Documentation for Kaveon — the self-hosted analytics platform. Talk to your data.",
};

export default function DocsLayout({ children }: { children: React.ReactNode }) {
  return <><PublicHeader active="docs" /><DocsShell>{children}</DocsShell></>;
}
