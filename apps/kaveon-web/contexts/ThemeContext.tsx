"use client";

import React, { createContext, useContext, useState, useEffect, ReactNode } from "react";

type Theme = "light" | "dark";

interface ThemeContextType {
  theme: Theme;
  toggleTheme: () => void;
  setTheme: (theme: Theme | string) => void;
  primaryColor: string;
  gradientColors: { lighter: string; light: string; base: string; dark: string };
  isLoading: boolean;
  resetTheme: () => Promise<void>;
}

const BRAND_BLUE = "#4A9EE8";
const STORAGE_KEY = "kaveon-theme";

const ThemeContext = createContext<ThemeContextType | undefined>(undefined);

export function useTheme(): ThemeContextType {
  const context = useContext(ThemeContext);
  if (!context) throw new Error("useTheme must be used within ThemeProvider");
  return context;
}

// A ?forceTheme=light|dark URL param pins the theme for that document only
// (used by the thumbnail-refresh iframe to render a dashboard in a specific
// theme). It never persists to localStorage, so it can't change the user's
// real preference — the iframe is a separate document with its own provider.
function getForcedTheme(): Theme | null {
  if (typeof window === "undefined") return null;
  const p = new URLSearchParams(window.location.search).get("forceTheme");
  return p === "light" || p === "dark" ? p : null;
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(() => {
    const forced = getForcedTheme();
    if (forced) return forced;
    if (typeof window !== "undefined") {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved === "dark" || saved === "light") return saved;
    }
    return "dark";
  });
  const forced = typeof window !== "undefined" && getForcedTheme() !== null;

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    if (!forced) localStorage.setItem(STORAGE_KEY, theme);
  }, [theme, forced]);

  const toggleTheme = () => { if (!forced) setThemeState((t) => (t === "light" ? "dark" : "light")); };

  // Accept both Theme type and legacy color strings (ignore color strings)
  const setTheme = (t: Theme | string) => {
    if (forced) return;
    if (t === "light" || t === "dark") setThemeState(t);
  };

  const value: ThemeContextType = {
    theme,
    toggleTheme,
    setTheme,
    primaryColor: BRAND_BLUE,
    gradientColors: { lighter: "#90c7f2", light: "#6db3ed", base: BRAND_BLUE, dark: "#2d7dd2" },
    isLoading: false,
    resetTheme: async () => setThemeState("light"),
  };

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}
