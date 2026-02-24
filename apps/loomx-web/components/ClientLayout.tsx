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
 *   3. Once authenticated, render Layout + children IMMEDIATELY so page.tsx
 *      data fetches fire in parallel with the /status check.
 *   4. /status fires in the background (sessionStorage short-circuits it on
 *      every subsequent load within the same browser session).
 *   5. If /status returns non-ok → overlay SetupModal (fixed, full-screen,
 *      blocks interaction) without unmounting the already-loading page.
 *   6. Once SetupModal completes → clear the overlay, page is already warm.
 */
export function ClientLayout({ children }: ClientLayoutProps) {
  const { isAuthenticated, isConnecting } = useAuth();

  // null  = check not yet returned (may or may not be needed)
  // value = setup is required — show the modal
  const [setupData, setSetupData] = useState<SetupData | null>(null);
  const checkedRef = useRef(false);

  useEffect(() => {
    if (!isAuthenticated || checkedRef.current) return;
    checkedRef.current = true;

    // Fast path: if we already confirmed setup is ok in this browser session,
    // skip the network round-trip entirely.
    if (typeof sessionStorage !== "undefined" && sessionStorage.getItem(SETUP_OK_KEY) === "1") {
      return;
    }

    // Fire /status in the background — children are already rendering and
    // their data fetches are running in parallel.
    void (async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/setup/status`);
        const data: SetupData = await res.json();
        if (data.status === "ok") {
          if (typeof sessionStorage !== "undefined") {
            sessionStorage.setItem(SETUP_OK_KEY, "1");
          }
        } else {
          // Setup required — overlay the modal on top of the already-rendered app.
          setSetupData(data);
        }
      } catch {
        // If the check fails (API not running yet), fail open — let normal
        // error handling surface the problem to the user.
      }
    })();
  }, [isAuthenticated]);

  // ── Auth loading states (keep these — they're fast) ──────────────────────

  if (isConnecting) {
    return <LoadingOverlay />;
  }

  if (!isAuthenticated) {
    return <AuthScreen />;
  }

  // ── Render immediately — no blocking on the setup check ──────────────────
  //
  // Layout + children mount and start their own data fetches right away.
  // If the setup check returns "needs setup", SetupModal overlays the screen
  // as a fixed full-screen layer so the user can't interact with the app
  // until the database is configured.

  return (
    <>
      <Layout>{children}</Layout>

      {setupData && setupData.status !== "ok" && (
        <div
          style={{
            position: "fixed",
            inset: 0,
            zIndex: 9999,
            background: "rgba(0, 0, 0, 0.65)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <SetupModal
            data={setupData}
            onComplete={() => {
              if (typeof sessionStorage !== "undefined") {
                sessionStorage.setItem(SETUP_OK_KEY, "1");
              }
              setSetupData(null);
            }}
          />
        </div>
      )}
    </>
  );
}
