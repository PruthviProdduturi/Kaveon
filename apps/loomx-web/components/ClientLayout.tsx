"use client";

import { ReactNode, useState, useEffect, useRef } from "react";
import { useAuth } from "../auth/useAuth";
import { AuthScreen } from "./AuthScreen";
import { Layout } from "./Layout";
import { LoadingOverlay } from "./LoadingOverlay";
import { SetupModal, type SetupData } from "./SetupModal";
import { msalFetch } from "../utils/msalFetch";
import { API_BASE } from "../config";

interface ClientLayoutProps {
  children: ReactNode;
}

/**
 * Client-side layout wrapper that enforces authentication and verifies that
 * the metadata database is reachable and initialised before rendering the app.
 *
 * Flow:
 *   1. Wait for MSAL auth check (isConnecting).
 *   2. If not authenticated → show AuthScreen.
 *   3. Once authenticated, call GET /api/v1/setup/status once.
 *   4. If status is 'ok' → render the full Layout.
 *   5. Otherwise → show SetupModal (blocks the app until resolved).
 */
export function ClientLayout({ children }: ClientLayoutProps) {
  const { isAuthenticated, isConnecting } = useAuth();

  const [setupData, setSetupData] = useState<SetupData | null>(null);
  const checkedRef = useRef(false);

  useEffect(() => {
    if (!isAuthenticated || checkedRef.current) return;
    checkedRef.current = true;

    const checkSetup = async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/setup/status`);
        const data: SetupData = await res.json();
        setSetupData(data);
      } catch {
        // If the setup check itself fails (e.g. API not running yet), don't
        // block the user — treat as ok and let normal error handling take over.
        setSetupData({ status: "ok" });
      }
    };

    void checkSetup();
  }, [isAuthenticated]);

  // ── Loading states ────────────────────────────────────────────────────────

  if (isConnecting) {
    return <LoadingOverlay />;
  }

  if (!isAuthenticated) {
    return <AuthScreen />;
  }

  // Still waiting for the setup status response.
  if (!setupData) {
    return <LoadingOverlay />;
  }

  // ── Setup required ────────────────────────────────────────────────────────

  if (setupData.status !== "ok") {
    return (
      <SetupModal
        data={setupData}
        onComplete={() => setSetupData({ status: "ok" })}
      />
    );
  }

  // ── Normal app ────────────────────────────────────────────────────────────

  return <Layout>{children}</Layout>;
}
