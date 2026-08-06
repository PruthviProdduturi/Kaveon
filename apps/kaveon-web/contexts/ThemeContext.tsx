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

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(() => {
    if (typeof window !== "undefined") {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved === "dark" || saved === "light") return saved;
    }
    return "light";
  });

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem(STORAGE_KEY, theme);
  }, [theme]);

  const toggleTheme = () => setThemeState((t) => (t === "light" ? "dark" : "light"));

  // Accept both Theme type and legacy color strings (ignore color strings)
  const setTheme = (t: Theme | string) => {
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
