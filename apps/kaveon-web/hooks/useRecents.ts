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

function saveRecents(items: RecentItem[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(items));
  } catch {}
}

export function useRecents() {
  const [recents, setRecents] = useState<RecentItem[]>(loadRecents);

  // Sync across tabs
  useEffect(() => {
    const handler = (e: StorageEvent) => {
      if (e.key === STORAGE_KEY) setRecents(loadRecents());
    };
    window.addEventListener("storage", handler);
    return () => window.removeEventListener("storage", handler);
  }, []);

  const addRecent = useCallback((item: Omit<RecentItem, "timestamp">) => {
    setRecents((prev) => {
      const filtered = prev.filter((r) => r.id !== item.id);
      const next = [{ ...item, timestamp: Date.now() }, ...filtered].slice(0, MAX_RECENTS);
      saveRecents(next);
      return next;
    });
  }, []);

  const removeRecent = useCallback((id: string) => {
    setRecents((prev) => {
      const next = prev.filter((r) => r.id !== id);
      saveRecents(next);
      return next;
    });
  }, []);

  const clearRecents = useCallback(() => {
    setRecents([]);
    saveRecents([]);
  }, []);

  return { recents, addRecent, removeRecent, clearRecents };
}
