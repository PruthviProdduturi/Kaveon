"use client";

import { ReactNode, useState, useEffect, useRef, createContext, useContext } from "react";
import { usePathname } from "next/navigation";
import { useAuth } from "../auth/useAuth";
import { ServiceNotice } from "./ServiceNotice";
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
    if (!isAuthenticated) {
      setIsSetupOk(null);
      // The latch has to come off with the answer it guards. It did not, so a
      // session that went unauthenticated and came back — a refetch that
      // momentarily fails, a sign-out and sign-in without a page load — left
      // `isSetupOk` null for the rest of the session. Null is "the check has
      // not come back", and the pages that read it wait on it: Favorites and
      // Workspace activity never leave their spinner, and Home never reads
      // its sessions or its catalogue.
      checkedRef.current = false;
    }
  }, [isAuthenticated]);

  useEffect(() => {
    if (!isAuthenticated || checkedRef.current) return;
    checkedRef.current = true;

    void (async () => {
      try {
        const res = await msalFetch(`${API_BASE}/api/v1/setup/status`);
        // Only a response that answered may produce a verdict. A refusal or a
        // gateway error has no `status` field, so reading one out of it
        // resolved to `false` — "this installation reports itself as not set
        // up" — and Favorites then told the reader the system of record was
        // not configured on a server that had merely declined the question.
        if (!res.ok) { setIsSetupOk(true); return; }
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

  // The two surfaces that need nothing from the cluster: the product page and
  // the documentation. Both render whole with the API down, so they are left
  // alone — interrupting someone reading the documentation to tell them a
  // cluster they were not using is offline would be noise.
  if (pathname === "/" || pathname === "/docs" || pathname?.startsWith("/docs/")) return <>{children}</>;

  // Everything below depends on the API, sign-in included, so the notice sits
  // above the auth gate rather than behind it. Behind it — where it was — the
  // only people who could see a message about the service being unavailable
  // were people who had already signed in, which needs the service to be
  // available. It was unreachable by its own audience.
  const notice = <ServiceNotice />;

  if (pathname === "/auth/microsoft" || pathname === "/login") {
    return <>{notice}{children}</>;
  }

  // Still checking auth. This returned null: a blank page for as long as the
  // check takes, and nothing behind the notice once it is dismissed. The mark
  // and the word are the difference between a slow page and a broken one.
  if (isConnecting) return <>{notice}<InlineLoading /></>;

  // Not authenticated — show login page (no sidebar)
  if (!isAuthenticated) return <>{notice}<AuthScreen /></>;

  // Authenticated — show sidebar layout
  return (
    <SetupContext.Provider value={{ isSetupOk }}>
      {notice}
      <Layout>{children}</Layout>
    </SetupContext.Provider>
  );
}
