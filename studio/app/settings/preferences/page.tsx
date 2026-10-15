"use client";

import { useEffect, useState } from "react";
import { useTheme } from "../../../contexts/ThemeContext";
import s from "../settings.module.css";

// Personal, browser-local settings. Nothing here changes the server.
const BANNER_KEY = "kaveon-context-banner";

export default function PreferencesPage() {
  const { theme, toggleTheme } = useTheme();
  // Read after mount, not during render. Read during render, the server
  // rendered the switch as on and a browser that had hidden the banner
  // rendered it as off, which is a hydration mismatch on the one attribute
  // that says what the control's state is.
  const [banner, setBanner] = useState(true);
  useEffect(() => {
    try { setBanner(localStorage.getItem(BANNER_KEY) !== "hidden"); } catch { /* default stands */ }
  }, []);

  const setBannerPref = (on: boolean) => {
    setBanner(on);
    try { if (on) localStorage.removeItem(BANNER_KEY); else localStorage.setItem(BANNER_KEY, "hidden"); } catch { /* preference is best effort */ }
  };

  return (
    <div className={s.stack}>
      <section className={s.card}>
        <div className={s.cardHead}>
          <div className={s.cardId}>
            <div className={s.mark}><i className="fas fa-user-gear" /></div>
            <div><h2 className={s.cardTitle}>Preferences</h2><p className={s.cardSub}>Saved in this browser, for you only.</p></div>
          </div>
        </div>
        <div style={{ marginTop: 14 }}>
          <div className={s.row}>
            <div><div className={s.rowLabel}>Dark theme</div><div className={s.rowHelp}>Applies to every Studio page.</div></div>
            <button type="button" role="switch" aria-checked={theme === "dark"} aria-label="Dark theme" className={s.switch} onClick={toggleTheme}><i /></button>
          </div>
          <div className={s.row}>
            <div><div className={s.rowLabel}>Context banner on Chat</div><div className={s.rowHelp}>Shows compiled DLM coverage at the top of the chat page.</div></div>
            <button type="button" role="switch" aria-checked={banner} aria-label="Context banner on Chat" className={s.switch} onClick={() => setBannerPref(!banner)}><i /></button>
          </div>
        </div>
      </section>
    </div>
  );
}
