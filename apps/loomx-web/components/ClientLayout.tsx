"use client";

import { ReactNode, useState, useEffect, useRef, createContext, useContext } from "react";
import { usePathname } from "next/navigation";
import { useAuth } from "../auth/useAuth";
import { AuthScreen } from "./AuthScreen";
import { Layout } from "./Layout";
import { LoadingOverlay } from "./LoadingOverlay";
import { LoomXLogo } from "./LoomXLogo";
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

function SetupRedirectOverlay({ onDismiss }: { onDismiss: () => void }) {
  return (
    <div style={{
      position: "fixed", inset: 0, background: "rgba(10,16,30,0.85)",
      display: "flex", alignItems: "center", justifyContent: "center",
      zIndex: 9999, padding: 20,
    }}>
      <div style={{
        background: "#1e293b", border: "1px solid #2d3f5c", borderRadius: 18,
        padding: "36px 40px 40px", width: "100%", maxWidth: 460,
        boxShadow: "0 32px 72px rgba(0,0,0,0.6)", textAlign: "center",
        position: "relative",
      }}>
        {/* Dismiss button */}
        <button
          onClick={onDismiss}
          style={{
            position: "absolute", top: 16, right: 16,
            background: "none", border: "none", cursor: "pointer",
            color: "#475569", fontSize: 18, lineHeight: 1, padding: "4px 6px",
          }}
          title="Dismiss — I'll set this up manually"
        >
          <i className="fas fa-times" />
        </button>

        <div style={{ display: "flex", justifyContent: "center", marginBottom: 4 }}>
          <LoomXLogo size={48} animate="pulse" />
        </div>
        <h2 style={{ fontSize: 21, fontWeight: 700, color: "#f1f5f9", margin: "16px 0 8px" }}>
          Setup Required
        </h2>
        <p style={{ fontSize: 13.5, color: "#94a3b8", lineHeight: 1.65, marginBottom: 28 }}>
          LoomX needs a metadata database to store your data. Configure it in System Settings to get started.
        </p>
        <a
          href="/settings/system"
          style={{
            display: "flex", alignItems: "center", justifyContent: "center", gap: 8,
            background: "linear-gradient(135deg,#3b82f6,#6366f1)",
            border: "none", borderRadius: 10, padding: "12px 20px",
            fontSize: 14, fontWeight: 600, color: "#fff", cursor: "pointer",
            textDecoration: "none", marginBottom: 14,
          }}
        >
          <i className="fas fa-sliders" />
          Go to System Settings
        </a>
        <button
          onClick={onDismiss}
          style={{
            background: "none", border: "none", cursor: "pointer",
            fontSize: 12.5, color: "#475569", textDecoration: "underline",
          }}
        >
          I&apos;ll configure this later
        </button>
      </div>
    </div>
  );
}

const SETUP_OK_KEY = "loomx_setup_ok";

function NoAccessScreen({ onLogout }: { onLogout: () => void }) {
  return (
    <div style={{ position: "fixed", inset: 0, background: "#f8fafc", display: "flex", alignItems: "center", justifyContent: "center", padding: 20 }}>
      <div style={{ textAlign: "center", maxWidth: 400 }}>
        <div style={{ width: 64, height: 64, borderRadius: "50%", background: "#fef2f2", border: "1px solid #fecaca", display: "flex", alignItems: "center", justifyContent: "center", margin: "0 auto 20px" }}>
          <i className="fas fa-ban" style={{ fontSize: 26, color: "#dc2626" }} />
        </div>
        <h2 style={{ fontSize: 20, fontWeight: 700, color: "#0f172a", margin: "0 0 10px" }}>No Access</h2>
        <p style={{ fontSize: 14, color: "#64748b", lineHeight: 1.6, margin: "0 0 28px" }}>
          You don&apos;t have a role assigned in LoomX. Contact your administrator and ask them to assign you a role via Azure AD → Enterprise Applications → LoomX → Users and groups.
        </p>
        <button
          onClick={onLogout}
          style={{ display: "inline-flex", alignItems: "center", gap: 8, padding: "10px 20px", borderRadius: 8, border: "none", background: "#0f172a", color: "#fff", fontSize: 13.5, fontWeight: 600, cursor: "pointer" }}
        >
          <i className="fas fa-right-from-bracket" /> Sign out
        </button>
      </div>
    </div>
  );
}

export function ClientLayout({ children }: ClientLayoutProps) {
  const { isAuthenticated, isConnecting, noAccess, logout } = useAuth();
  const pathname = usePathname();

  const [setupData,  setSetupData]  = useState<SetupData | null>(null);
  const [dismissed,  setDismissed]  = useState(false);
  const [isSetupOk,  setIsSetupOk]  = useState<boolean | null>(null);
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
          setSetupData(data);
          setIsSetupOk(false);
        }
      } catch {
        // API not yet reachable — fail open so pages can still load.
        setIsSetupOk(true);
      }
    })();
  }, [isAuthenticated]);

  if (isConnecting) return <LoadingOverlay />;
  if (noAccess) return <NoAccessScreen onLogout={logout} />;
  if (!isAuthenticated) return <AuthScreen />;

  const showOverlay =
    setupData &&
    setupData.status !== "ok" &&
    !dismissed &&
    pathname !== "/settings/system";

  return (
    <SetupContext.Provider value={{ isSetupOk }}>
      <Layout>{children}</Layout>
      {showOverlay && (
        <SetupRedirectOverlay onDismiss={() => setDismissed(true)} />
      )}
    </SetupContext.Provider>
  );
}
