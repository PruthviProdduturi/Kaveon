import type { Metadata } from "next";
import "../styles/globals.css";
import "../styles/dashboard.css";
import { Providers } from "./providers";
import { ClientLayout } from "../components/ClientLayout";

export const metadata: Metadata = {
  title: {
    default: "Kaveon",
    template: "Kaveon — %s",
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
        {/* Load dark mode preference before hydration to prevent flash */}
        <script dangerouslySetInnerHTML={{ __html: `(function(){var t=localStorage.getItem('kaveon-theme');if(t==='dark')document.documentElement.setAttribute('data-theme','dark')})();` }} />
      </head>
      <body suppressHydrationWarning>
        <Providers>
          <ClientLayout>{children}</ClientLayout>
        </Providers>
      </body>
    </html>
  );
}
