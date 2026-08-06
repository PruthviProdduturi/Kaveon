"use client";

import "@fortawesome/fontawesome-free/css/fontawesome.min.css";
import "@fortawesome/fontawesome-free/css/solid.min.css";
import "@fortawesome/fontawesome-free/css/regular.min.css";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";

import { SessionProvider } from "next-auth/react";
import { AuthProvider } from "../auth/useAuth";
import { ThemeProvider } from "../contexts/ThemeContext";
import { TabProvider } from "../contexts/TabContext";
import { ReactNode } from "react";

export function Providers({ children }: { children: ReactNode }) {
  return (
    <SessionProvider>
      <AuthProvider>
        <ThemeProvider>
          <TabProvider>
            {children}
          </TabProvider>
        </ThemeProvider>
      </AuthProvider>
    </SessionProvider>
  );
}
