"use client";

import { ReactNode, useState, useEffect, useRef } from "react";
import { usePathname } from "next/navigation";
import { useAuth } from "../auth/useAuth";
import { AuthScreen } from "./AuthScreen";
import { Layout } from "./Layout";
import { LoadingOverlay } from "./LoadingOverlay";
import { LoomXLogo } from "./LoomXLogo";
import { msalFetch } from "../utils/msalFetch";
import { API_BASE } from "../config";

interface SetupData { status: string; endpoint?: string; database?: string; }

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

export function ClientLayout({ children }: ClientLayoutProps) {
  const { isAuthenticated, isConnecting } = useAuth();
  const pathname = usePathname();

  const [setupData,  setSetupData]  = useState<SetupData | null>(null);
  const [dismissed,  setDismissed]  = useState(false);
  const checkedRef = useRef(false);

  useEffect(() => {
    if (!isAuthenticated && typeof sessionStorage !== "undefined") {
      sessionStorage.removeItem(SETUP_OK_KEY);
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
        } else {
          sessionStorage.removeItem(SETUP_OK_KEY);
          setSetupData(data);
        }
      } catch {
        // API not yet reachable — fail open.
      }
    })();
  }, [isAuthenticated]);

  if (isConnecting) return <LoadingOverlay />;
  if (!isAuthenticated) return <AuthScreen />;

  const showOverlay =
    setupData &&
    setupData.status !== "ok" &&
    !dismissed &&
    pathname !== "/settings/system";

  return (
    <>
      <Layout>{children}</Layout>
      {showOverlay && (
        <SetupRedirectOverlay onDismiss={() => setDismissed(true)} />
      )}
    </>
  );
}
