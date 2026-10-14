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

/**
 * There used to be a `lens_setup_ok` sessionStorage mirror of `isSetupOk`
 * alongside this context, and four unrelated surfaces read it to decide
 * whether to tell the operator their metadata database was unconfigured —
 * SQL Lab's error modal, both tabs of Saved queries, and `ListPageShell`.
 * The mirror is absent in three different situations that read identically:
 * setup genuinely incomplete, the check not yet returned, and the check
 * itself unable to reach the API. The third is what the complaint was about,
 * and it sent people to a wizard this installation refuses. The mirror is
 * gone; this context is the one answer, and a read that failed is reported by
 * `components/Reading` from the status it failed with.
 */

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
    if (!isAuthenticated) setIsSetupOk(null);
  }, [isAuthenticated]);

  useEffect(() => {
    if (!isAuthenticated || checkedRef.current) return;
    checkedRef.current = true;

    void (async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/setup/status`);
        const data: SetupData = await res.json();
        setIsSetupOk(data.status === "ok");
      } catch {
        // The check itself not answering says nothing about setup, so it is
        // not reported as a verdict either way: the app opens, and whichever
        // read actually fails reports its own failure.
        setIsSetupOk(true);
      }
    })();
  }, [isAuthenticated]);

  // Public pages — render on their own (no app sidebar, no auth gate).
  if (pathname === "/" || pathname === "/auth/microsoft" || pathname === "/login" || pathname === "/docs" || pathname?.startsWith("/docs/")) return <>{children}</>;

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
