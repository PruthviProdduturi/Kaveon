"use client";

import { useEffect, useRef, useState } from "react";
import { broadcastResponseToMainFrame } from "@azure/msal-browser/redirect-bridge";

export default function MicrosoftPopupCallback() {
  const started = useRef(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void broadcastResponseToMainFrame().catch(() => setFailed(true));
  }, []);
  return <p role="status">{failed ? "Unable to finish sign-in. Close this window and retry from the portal." : "Completing Microsoft sign-in…"}</p>;
}
