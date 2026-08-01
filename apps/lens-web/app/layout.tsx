import type { Metadata } from "next";
import "../styles/globals.css";
import "../styles/dashboard.css";
import { Providers } from "./providers";
import { ClientLayout } from "../components/ClientLayout";

export const metadata: Metadata = {
  title: "LENS",
  description: "Live Operational Outcomes & Metrics eXperience",
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
        {/* Load theme IMMEDIATELY to prevent flash */}
        <script
          dangerouslySetInnerHTML={{
            __html: `
              (function() {
                try {
                  var savedColor = localStorage.getItem('lens-theme-color');
                  if (savedColor) {
                    // Generate gradients
                    function hexToHSL(hex) {
                      hex = hex.replace(/^#/, '');
                      var r = parseInt(hex.substring(0, 2), 16) / 255;
                      var g = parseInt(hex.substring(2, 4), 16) / 255;
                      var b = parseInt(hex.substring(4, 6), 16) / 255;
                      var max = Math.max(r, g, b), min = Math.min(r, g, b);
                      var h = 0, s = 0, l = (max + min) / 2;
                      if (max !== min) {
                        var d = max - min;
                        s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
                        switch (max) {
                          case r: h = ((g - b) / d + (g < b ? 6 : 0)) / 6; break;
                          case g: h = ((b - r) / d + 2) / 6; break;
                          case b: h = ((r - g) / d + 4) / 6; break;
                        }
                      }
                      return { h: Math.round(h * 360), s: Math.round(s * 100), l: Math.round(l * 100) };
                    }
                    function hslToHex(h, s, l) {
                      h = h / 360; s = s / 100; l = l / 100;
                      var r, g, b;
                      if (s === 0) { r = g = b = l; }
                      else {
                        var hue2rgb = function(p, q, t) {
                          if (t < 0) t += 1; if (t > 1) t -= 1;
                          if (t < 1/6) return p + (q - p) * 6 * t;
                          if (t < 1/2) return q;
                          if (t < 2/3) return p + (q - p) * (2/3 - t) * 6;
                          return p;
                        };
                        var q = l < 0.5 ? l * (1 + s) : l + s - l * s;
                        var p = 2 * l - q;
                        r = hue2rgb(p, q, h + 1/3);
                        g = hue2rgb(p, q, h);
                        b = hue2rgb(p, q, h - 1/3);
                      }
                      var toHex = function(x) {
                        var hex = Math.round(x * 255).toString(16);
                        return hex.length === 1 ? '0' + hex : hex;
                      };
                      return '#' + toHex(r) + toHex(g) + toHex(b);
                    }
                    var hsl = hexToHSL(savedColor);
                    var lighter = hslToHex(hsl.h, hsl.s, Math.min(hsl.l + 40, 100));
                    var light = hslToHex(hsl.h, hsl.s, Math.min(hsl.l + 20, 100));
                    var dark = hslToHex(hsl.h, hsl.s, Math.max(hsl.l - 20, 0));

                    // Helper: convert hex to "R, G, B" string for rgba() CSS vars
                    function hexToRgbStr(h) {
                      h = h.replace(/^#/, '');
                      return parseInt(h.substring(0,2),16) + ',' + parseInt(h.substring(2,4),16) + ',' + parseInt(h.substring(4,6),16);
                    }

                    // Set CSS variables
                    document.documentElement.style.setProperty('--theme-primary', savedColor);
                    document.documentElement.style.setProperty('--theme-light', light);
                    document.documentElement.style.setProperty('--theme-dark', dark);
                    document.documentElement.style.setProperty('--lens-primary', savedColor);
                    document.documentElement.style.setProperty('--lens-secondary', light);
                    document.documentElement.style.setProperty('--lens-accent', lighter);
                    document.documentElement.style.setProperty('--lens-dark', dark);
                    document.documentElement.style.setProperty('--lens-light', lighter);
                    // RGB component vars for rgba() usage in LensLoading (no hydration mismatch)
                    document.documentElement.style.setProperty('--lens-primary-rgb', hexToRgbStr(savedColor));
                    document.documentElement.style.setProperty('--lens-secondary-rgb', hexToRgbStr(light));
                    document.documentElement.style.setProperty('--lens-accent-rgb', hexToRgbStr(lighter));
                    document.documentElement.style.setProperty('--lens-dark-rgb', hexToRgbStr(dark));
                  }
                } catch (e) { console.error('Theme preload error:', e); }
              })();
            `,
          }}
        />
      </head>
      <body suppressHydrationWarning>
        <Providers>
          <ClientLayout>{children}</ClientLayout>
        </Providers>
      </body>
    </html>
  );
}
