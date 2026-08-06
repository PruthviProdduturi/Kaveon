"use client";

import { useState, useCallback, useEffect } from "react";

export interface RecentItem {
  id: string;
  label: string;
  href: string;
  type: "dashboard" | "chart" | "dataset" | "query" | "chat";
  timestamp: number;
}

const STORAGE_KEY = "kaveon-recents";
const SYNC_EVENT = "kaveon-recents-updated";
const MAX_RECENTS = 12;

function loadRecents(): RecentItem[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

function saveAndNotify(items: RecentItem[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(items));
    // Dispatch custom event for same-tab sync (StorageEvent only fires cross-tab)
    window.dispatchEvent(new CustomEvent(SYNC_EVENT));
  } catch {}
}

export function useRecents() {
  const [recents, setRecents] = useState<RecentItem[]>(loadRecents);

  // Listen for updates from other hook instances (same tab) and other tabs
  useEffect(() => {
    const reload = () => setRecents(loadRecents());
    window.addEventListener(SYNC_EVENT, reload);
    window.addEventListener("storage", (e) => {
      if (e.key === STORAGE_KEY) reload();
    });
    return () => {
      window.removeEventListener(SYNC_EVENT, reload);
    };
  }, []);

  const addRecent = useCallback((item: Omit<RecentItem, "timestamp">) => {
    const current = loadRecents();
    const filtered = current.filter((r) => r.id !== item.id);
    const next = [{ ...item, timestamp: Date.now() }, ...filtered].slice(0, MAX_RECENTS);
    saveAndNotify(next);
  }, []);

  const removeRecent = useCallback((id: string) => {
    const current = loadRecents();
    const next = current.filter((r) => r.id !== id);
    saveAndNotify(next);
  }, []);

  const clearRecents = useCallback(() => {
    saveAndNotify([]);
  }, []);

  return { recents, addRecent, removeRecent, clearRecents };
}
