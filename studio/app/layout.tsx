import type { Metadata } from "next";
import "../styles/globals.css";
import "../styles/dashboard.css";
import "./globals.css";
import { Providers } from "./providers";
import { ClientLayout } from "../components/ClientLayout";
import { Analytics } from "@vercel/analytics/next";

export const metadata: Metadata = {
  title: {
    default: "Kaveon",
    template: "%s — Kaveon",
  },
  description: "Kaveon — the intelligent data platform. Talk to your data.",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" suppressHydrationWarning style={{ background: "#171717" }}>
      <head>
        {/* Readable with `curl -s <host> | grep kaveon-build`, which is the
            question asked of a deployment most often and the one that was
            hardest to answer: what is actually serving here? */}
        <meta name="kaveon-build" content={process.env.NEXT_PUBLIC_KAVEON_BUILD ?? 'unknown'} />
        <link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.7.2/css/all.min.css" />
        {/* Set the ground and the theme BEFORE any CSS/JS loads — prevents a flash.
            The product page at / is a theme-independent #0a0a0a surface and renders
            client-side, so taking the theme ground there left a #171717 band (or, in
            light theme, a near-white one) on screen until the page painted over it.
            It reads as a grey header, and only a warm cache hides it. */}
        <script dangerouslySetInnerHTML={{ __html: `(function(){var t=localStorage.getItem('kaveon-theme')||'dark';document.documentElement.setAttribute('data-theme',t);var b=location.pathname==='/'?'#0a0a0a':(t==='dark'?'#171717':'#f5f5f7');document.documentElement.style.background=b;document.body&&(document.body.style.background=b)})();` }} />
      </head>
      <body suppressHydrationWarning style={{ background: "#171717" }}>
        <Providers>
          <ClientLayout>{children}</ClientLayout>
        </Providers>
        <Analytics />
      </body>
    </html>
  );
}
