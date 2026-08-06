import type { Metadata } from "next";
import "../styles/globals.css";
import "../styles/dashboard.css";
import { Providers } from "./providers";
import { ClientLayout } from "../components/ClientLayout";

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
    <html lang="en" suppressHydrationWarning>
      <head>
        <link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.7.2/css/all.min.css" />
        {/* Load theme preference before hydration — defaults to dark */}
        <script dangerouslySetInnerHTML={{ __html: `(function(){var t=localStorage.getItem('kaveon-theme');document.documentElement.setAttribute('data-theme',t||'dark')})();` }} />
      </head>
      <body suppressHydrationWarning>
        <Providers>
          <ClientLayout>{children}</ClientLayout>
        </Providers>
      </body>
    </html>
  );
}
