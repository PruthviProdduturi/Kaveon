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
        <link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.7.2/css/all.min.css" />
        {/* Set dark background + theme BEFORE any CSS/JS loads — prevents white flash */}
        <script dangerouslySetInnerHTML={{ __html: `(function(){var t=localStorage.getItem('kaveon-theme')||'dark';document.documentElement.setAttribute('data-theme',t);var b=t==='dark'?'#171717':'#f5f5f7';document.documentElement.style.background=b;document.body&&(document.body.style.background=b)})();` }} />
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
