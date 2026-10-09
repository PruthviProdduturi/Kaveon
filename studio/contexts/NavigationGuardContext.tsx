"use client";

/**
 * Navigation Guard
 *
 * One place that stands between unsaved editor work and every way out of a
 * page. An editor registers itself with `useUnsavedWorkGuard` while it holds
 * unsaved changes, and the provider then intercepts:
 *
 *   - clicks on any in-app link (the sidebar, the wordmark, recents, any <Link>),
 *   - programmatic pushes routed through `useGuardedNavigate`,
 *   - the browser's Back and Forward buttons,
 *   - closing or reloading the tab (the browser's own generic prompt).
 *
 * Every intercepted exit offers the three real choices — save and continue,
 * discard and continue, or stay — so answering the dialog never costs the user
 * their work.
 *
 * Nothing is intercepted while the registered editor is clean: the link
 * listener, the history sentinel and the `beforeunload` handler are all armed
 * only while there is genuinely something to lose.
 */

import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { ConfirmModal } from "../components/ConfirmModal";

/** The copy shown when an exit is intercepted. */
export interface UnsavedWorkPrompt {
  title: string;
  message: string;
  /** Primary action — saves, then continues. */
  saveLabel: string;
  /** Destructive action — continues without saving. */
  discardLabel: string;
  /** Dismissal — stays on the page. */
  stayLabel: string;
}

export interface UnsavedWorkGuard {
  /** True while the editor holds changes that are not persisted. */
  isDirty: boolean;
  /** Copy for a plain "you are leaving this page" exit. */
  leavePrompt: UnsavedWorkPrompt;
  /**
   * Persists the work. Must resolve `true` only once the work is stored, and
   * `false` when the save failed. `null` when the editor cannot save without
   * more input from the user (an unnamed new document, for instance), which
   * reduces the dialog to discard-or-stay.
   */
  save: (() => Promise<boolean>) | null;
}

/** Runs `proceed` once the user has decided what happens to the unsaved work. */
export type RequestExit = (proceed: () => void, prompt?: Partial<UnsavedWorkPrompt>) => void;

interface NavigationGuardApi {
  registerGuard: (guard: UnsavedWorkGuard | null) => void;
  requestExit: RequestExit;
  navigate: (href: string) => void;
}

const NavigationGuardContext = createContext<NavigationGuardApi | null>(null);

interface PendingExit {
  prompt: UnsavedWorkPrompt;
  proceed: () => void;
  canSave: boolean;
}

export function NavigationGuardProvider({ children }: { children: React.ReactNode }) {
  const router = useRouter();
  const guardRef = useRef<UnsavedWorkGuard | null>(null);
  const [isDirty, setIsDirty] = useState(false);
  const [pending, setPending] = useState<PendingExit | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  /** Set while a confirmed exit is in flight, so the interceptors stand down. */
  const leavingRef = useRef(false);

  const registerGuard = useCallback((guard: UnsavedWorkGuard | null) => {
    guardRef.current = guard;
    setIsDirty(Boolean(guard?.isDirty));
  }, []);

  const requestExit = useCallback<RequestExit>((proceed, overrides) => {
    const guard = guardRef.current;
    // An exit the user already confirmed must not be re-asked, however the
    // caller routes the rest of the way out.
    if (leavingRef.current || !guard || !guard.isDirty) {
      proceed();
      return;
    }
    setPending({
      prompt: { ...guard.leavePrompt, ...overrides },
      proceed,
      canSave: Boolean(guard.save),
    });
  }, []);

  const navigate = useCallback((href: string) => {
    requestExit(() => router.push(href));
  }, [requestExit, router]);

  const runProceed = useCallback((proceed: () => void) => {
    leavingRef.current = true;
    setPending(null);
    proceed();
  }, []);

  const handleSaveAndLeave = useCallback(async () => {
    const save = guardRef.current?.save;
    const proceed = pending?.proceed;
    if (!save || !proceed) return;
    setIsSaving(true);
    let saved = false;
    try {
      saved = await save();
    } finally {
      setIsSaving(false);
    }
    if (saved) {
      runProceed(proceed);
    } else {
      // The editor surfaces its own save error; close the dialog and leave the
      // user on the page with their work intact.
      setPending(null);
    }
  }, [pending, runProceed]);

  const handleDiscardAndLeave = useCallback(() => {
    if (pending) runProceed(pending.proceed);
  }, [pending, runProceed]);

  const handleStay = useCallback(() => {
    if (!isSaving) setPending(null);
  }, [isSaving]);

  // A fresh stretch of unsaved work re-arms the interceptors after an exit that
  // was confirmed earlier (editing a document, leaving it, then editing again).
  useEffect(() => {
    if (isDirty) leavingRef.current = false;
  }, [isDirty]);

  // ── In-app links ────────────────────────────────────────────────────────────
  // Next's <Link> handles a plain left click on its own anchor. A listener in
  // the capture phase on `document` runs before React's delegated handler on
  // the root container, so stopping propagation there is what actually keeps
  // the navigation from starting.
  useEffect(() => {
    if (!isDirty) return;

    const onClick = (event: MouseEvent) => {
      if (leavingRef.current) return;
      if (event.defaultPrevented || event.button !== 0) return;
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;

      const target = event.target as Element | null;
      const anchor = target?.closest?.("a[href]") as HTMLAnchorElement | null;
      if (!anchor || anchor.hasAttribute("download")) return;
      if (anchor.target && anchor.target !== "_self") return;

      let url: URL;
      try {
        url = new URL(anchor.href, window.location.href);
      } catch {
        return;
      }
      // A different origin leaves the app entirely — that is `beforeunload`'s
      // job. Same-path links are in-page anchors and change nothing.
      if (url.origin !== window.location.origin) return;
      if (url.pathname === window.location.pathname) return;

      event.preventDefault();
      event.stopPropagation();
      const href = `${url.pathname}${url.search}${url.hash}`;
      requestExit(() => router.push(href));
    };

    document.addEventListener("click", onClick, true);
    return () => document.removeEventListener("click", onClick, true);
  }, [isDirty, requestExit, router]);

  // ── Closing or reloading the tab ────────────────────────────────────────────
  // Only the browser's own generic prompt is available here; no custom copy is
  // possible. Armed only while dirty, so a plain reload is never interrupted.
  useEffect(() => {
    if (!isDirty) return;
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  }, [isDirty]);

  // ── Back and Forward ────────────────────────────────────────────────────────
  // A history traversal cannot be cancelled: by the time `popstate` fires the
  // entry is already gone. The remedy is a sentinel — while there is unsaved
  // work the current URL sits in the history stack twice, so the first Back
  // press lands on the duplicate and stays on the page. The duplicate is
  // re-pushed on every press, so there is never more than one extra entry, and
  // leaving for real therefore means going back two entries.
  useEffect(() => {
    if (!isDirty) return;

    const guardedPath = window.location.pathname;
    const hadPriorEntry = window.history.length > 1;
    const pushSentinel = () => window.history.pushState(null, "", window.location.href);

    pushSentinel();

    const onPopState = () => {
      if (leavingRef.current) return;
      if (!guardRef.current?.isDirty) return;
      pushSentinel();
      requestExit(() => window.history.go(hadPriorEntry ? -2 : -1));
    };

    window.addEventListener("popstate", onPopState);
    return () => {
      window.removeEventListener("popstate", onPopState);
      // Work became clean without leaving (the user undid their edits, or
      // saved and stayed): drop the sentinel so Back works first time again.
      // Skipped once an exit is in flight, where the stack is already moving.
      if (!leavingRef.current && window.location.pathname === guardedPath) {
        window.history.back();
      }
    };
  }, [isDirty, requestExit]);

  const api = useMemo<NavigationGuardApi>(
    () => ({ registerGuard, requestExit, navigate }),
    [registerGuard, requestExit, navigate],
  );

  return (
    <NavigationGuardContext.Provider value={api}>
      {children}
      {pending && (
        pending.canSave ? (
          <ConfirmModal
            isOpen
            danger={false}
            icon="fas fa-exclamation-triangle"
            width={460}
            title={pending.prompt.title}
            message={pending.prompt.message}
            confirmLabel={pending.prompt.saveLabel}
            onConfirm={handleSaveAndLeave}
            secondaryLabel={pending.prompt.discardLabel}
            onSecondary={handleDiscardAndLeave}
            cancelLabel={pending.prompt.stayLabel}
            onCancel={handleStay}
            busy={isSaving}
          />
        ) : (
          <ConfirmModal
            isOpen
            danger
            icon="fas fa-exclamation-triangle"
            width={460}
            title={pending.prompt.title}
            message={pending.prompt.message}
            confirmLabel={pending.prompt.discardLabel}
            onConfirm={handleDiscardAndLeave}
            cancelLabel={pending.prompt.stayLabel}
            onCancel={handleStay}
          />
        )
      )}
    </NavigationGuardContext.Provider>
  );
}

function useNavigationGuardApi(): NavigationGuardApi | null {
  return useContext(NavigationGuardContext);
}

/**
 * Registers an editor's unsaved work with the guard for as long as the editor
 * is mounted, and returns the function it should route its own exits through
 * (a View button, for instance).
 *
 * Memoise the guard — its identity is the effect's dependency.
 */
export function useUnsavedWorkGuard(guard: UnsavedWorkGuard): RequestExit {
  const api = useNavigationGuardApi();
  const registerGuard = api?.registerGuard;

  useEffect(() => {
    if (!registerGuard) return;
    registerGuard(guard);
    return () => registerGuard(null);
  }, [registerGuard, guard]);

  const fallback = useCallback<RequestExit>((proceed) => proceed(), []);
  return api?.requestExit ?? fallback;
}

/**
 * `router.push` that asks first when an editor holds unsaved work. Use it for
 * navigation driven by anything other than a link — menu items, search
 * results, list rows.
 */
export function useGuardedNavigate(): (href: string) => void {
  const api = useNavigationGuardApi();
  const router = useRouter();
  const fallback = useCallback((href: string) => router.push(href), [router]);
  return api?.navigate ?? fallback;
}
