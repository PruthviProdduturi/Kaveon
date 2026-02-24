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

const SETUP_OK_KEY = "loomx_setup_ok";

/**
 * Client-side layout wrapper that enforces authentication and verifies that
 * the metadata database is reachable and initialised before rendering the app.
 *
 * Flow:
 *   1. Wait for MSAL auth check (isConnecting).
 *   2. If not authenticated → show AuthScreen.
 *   3. Once authenticated, check sessionStorage for a cached "ok" result.
 *      If found → skip the network call and render immediately.
 *   4. Otherwise call GET /api/v1/setup/status, cache "ok" in sessionStorage.
 *   5. If status is not 'ok' → show SetupModal (blocks the app until resolved).
 *
 * sessionStorage is scoped to the browser tab session, so setup is only
 * re-verified after a full tab close/reopen or after SetupModal completes.
 */
export function ClientLayout({ children }: ClientLayoutProps) {
  const { isAuthenticated, isConnecting } = useAuth();

  const [setupData, setSetupData] = useState<SetupData | null>(null);
  const checkedRef = useRef(false);

  useEffect(() => {
    if (!isAuthenticated || checkedRef.current) return;
    checkedRef.current = true;

    // Fast path: if we already confirmed setup is ok in this browser session,
    // skip the network round-trip (avoids a 7-10s delay on every page refresh).
    if (typeof sessionStorage !== "undefined" && sessionStorage.getItem(SETUP_OK_KEY) === "1") {
      setSetupData({ status: "ok" });
      return;
    }

    const checkSetup = async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/setup/status`);
        const data: SetupData = await res.json();
        if (data.status === "ok" && typeof sessionStorage !== "undefined") {
          sessionStorage.setItem(SETUP_OK_KEY, "1");
        }
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
        onComplete={() => {
          if (typeof sessionStorage !== "undefined") {
            sessionStorage.setItem(SETUP_OK_KEY, "1");
          }
          setSetupData({ status: "ok" });
        }}
      />
    );
  }

  // ── Normal app ────────────────────────────────────────────────────────────

  return <Layout>{children}</Layout>;
}
