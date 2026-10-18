"use client";

import { useEffect, useRef, useState } from "react";
import Link from "next/link";

/**
 * What the product says while its demo cluster is deliberately offline.
 *
 * This is not an error. The engine work is planned, the date is known, and
 * the documentation and product overview are unaffected — so the notice says
 * all three rather than leaving a person to discover an empty Library and
 * guess. A failure nobody explains reads as a broken product; a maintenance
 * window with a date reads as a maintained one.
 *
 * Mounted above the auth gate, not inside it. Signing in needs the API, so
 * while the API is unreachable nobody reaches an authenticated page — and
 * mounting this behind that gate, as it first was, showed a message about the
 * service being down only to people the outage had already locked out. It now
 * covers the sign-in page, the session check and the authenticated shell.
 *
 * `/` and `/docs` are excluded on purpose: both render whole with the cluster
 * offline, and interrupting someone reading the documentation to tell them a
 * cluster they were not using is offline would be noise.
 *
 * Controlled by `NEXT_PUBLIC_KAVEON_SERVICE_NOTICE`, so ending the window is
 * a configuration change rather than a code change. Unset, nothing renders
 * and this costs one boolean.
 */
const STORAGE_KEY = "kaveon.service-notice.dismissed";

export function ServiceNotice() {
  const notice = process.env.NEXT_PUBLIC_KAVEON_SERVICE_NOTICE;
  const returns = process.env.NEXT_PUBLIC_KAVEON_SERVICE_NOTICE_RETURNS;
  const [open, setOpen] = useState(false);
  const dismissRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (!notice) return;
    // A dismissal is a per-browser convenience, not state anything depends
    // on: if it cannot be read the notice simply shows again, which is the
    // safe direction for a message about the service being unavailable.
    let dismissed = false;
    try {
      dismissed = window.localStorage.getItem(STORAGE_KEY) === notice;
    } catch {
      dismissed = false;
    }
    setOpen(!dismissed);
  }, [notice]);

  useEffect(() => {
    if (!open) return;
    dismissRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") dismiss();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  if (!notice || !open) return null;

  function dismiss() {
    setOpen(false);
    // Keyed on the message, so changing the notice shows it again rather
    // than being swallowed by an old dismissal.
    try {
      window.localStorage.setItem(STORAGE_KEY, notice as string);
    } catch {
      /* A browser that refuses storage still gets to close the dialog. */
    }
  }

  return (
    <div className="service-notice" role="presentation" onClick={dismiss}>
      <div
        className="service-notice__card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="service-notice-title"
        aria-describedby="service-notice-body"
        onClick={(event) => event.stopPropagation()}
      >
        <h2 id="service-notice-title" className="service-notice__title">
          The live demo is paused
        </h2>
        <p id="service-notice-body" className="service-notice__body">
          {notice}
        </p>
        {returns && (
          <p className="service-notice__returns">
            Expected back <strong>{returns}</strong>
          </p>
        )}
        <p className="service-notice__aside">
          The documentation and product overview are unaffected and stay
          available throughout.
        </p>
        <div className="service-notice__actions">
          <Link href="/docs" className="service-notice__link">
            Read the documentation
          </Link>
          <button
            ref={dismissRef}
            type="button"
            className="service-notice__dismiss"
            onClick={dismiss}
          >
            Continue anyway
          </button>
        </div>
      </div>
    </div>
  );
}
