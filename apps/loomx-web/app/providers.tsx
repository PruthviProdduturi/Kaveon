"use client";

import "@fortawesome/fontawesome-free/css/all.min.css";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";

import { AuthProvider } from "../auth/useAuth";
import { ThemeProvider } from "../contexts/ThemeContext";
import { ReactNode } from "react";

export function Providers({ children }: { children: ReactNode }) {
  return (
    <AuthProvider>
      <ThemeProvider>
        {children}
      </ThemeProvider>
    </AuthProvider>
  );
}
