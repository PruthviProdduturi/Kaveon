"use client";

import { ReactNode, useState, useEffect, useRef, createContext, useContext } from "react";
import { usePathname } from "next/navigation";
import { useAuth } from "../auth/useAuth";
import { AuthScreen } from "./AuthScreen";
import { Layout } from "./Layout";
import { KaveonMark } from "./KaveonMark";
import { msalFetch } from "../utils/msalFetch";
import { API_BASE } from "../config";

interface SetupData { status: string; endpoint?: string; database?: string; }

// ── Setup context — pages consume this to skip data calls when not set up ──────
interface SetupContextType {
  /** true = setup complete, false = setup required, null = check in progress */
  isSetupOk: boolean | null;
}
const SetupContext = createContext<SetupContextType>({ isSetupOk: null });
export function useSetup() { return useContext(SetupContext); }

interface ClientLayoutProps {
  children: ReactNode;
}

const SETUP_OK_KEY = "lens_setup_ok";

/** Inline loading state — shown inside the sidebar layout, not full-screen */
function InlineLoading() {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        minHeight: "100vh",
        gap: 24,
      }}
    >
      <div style={{ animation: "kaveon-breathe 3s ease-in-out infinite" }}>
        <KaveonMark size={48} useDirectColor />
      </div>
      <p style={{ fontSize: 13, color: "var(--text-muted)", letterSpacing: "0.1em", textTransform: "uppercase" }}>
        Loading...
      </p>
    </div>
  );
}

export function ClientLayout({ children }: ClientLayoutProps) {
  const { isAuthenticated, isConnecting, noAccess, logout } = useAuth();
  const pathname = usePathname();

  const [isSetupOk, setIsSetupOk] = useState<boolean | null>(null);
  const checkedRef = useRef(false);

  useEffect(() => {
    if (!isAuthenticated && typeof sessionStorage !== "undefined") {
      sessionStorage.removeItem(SETUP_OK_KEY);
      setIsSetupOk(null);
    }
  }, [isAuthenticated]);

  useEffect(() => {
    if (!isAuthenticated || checkedRef.current) return;
    checkedRef.current = true;

    void (async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/setup/status`);
        const data: SetupData = await res.json();
        if (data.status === "ok") {
          sessionStorage.setItem(SETUP_OK_KEY, "1");
          setIsSetupOk(true);
        } else {
          sessionStorage.removeItem(SETUP_OK_KEY);
          setIsSetupOk(false);
        }
      } catch {
        setIsSetupOk(true);
      }
    })();
  }, [isAuthenticated]);

  // Public documentation — render on its own (no app sidebar, no auth gate).
  if (pathname === "/docs" || pathname?.startsWith("/docs/")) return <>{children}</>;

  // Still checking auth — render nothing (avoids white flash before login)
  if (isConnecting) return null;

  // Not authenticated — show login page (no sidebar)
  if (!isAuthenticated) return <AuthScreen />;

  // Authenticated — show sidebar layout
  return (
    <SetupContext.Provider value={{ isSetupOk }}>
      <Layout>{children}</Layout>
    </SetupContext.Provider>
  );
}
