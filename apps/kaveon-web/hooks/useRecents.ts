"use client";

import { useState, useCallback, useEffect, useRef } from "react";
import { msalFetch } from "../utils/msalFetch";

export interface RecentItem {
  id: string;
  label: string;
  href: string;
  type: "dashboard" | "chart" | "dataset" | "query" | "chat";
  timestamp: number;
}

const STORAGE_KEY = "kaveon-recents";
const SYNC_EVENT = "kaveon-recents-updated";
const MAX_RECENTS = 20;

// Local storage as fast cache — API as persistent source of truth
function loadLocal(): RecentItem[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

function saveLocal(items: RecentItem[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(items));
    window.dispatchEvent(new CustomEvent(SYNC_EVENT));
  } catch {}
}

export function useRecents() {
  const [recents, setRecents] = useState<RecentItem[]>(loadLocal);
  const loadedFromApi = useRef(false);

  // Load from API on mount (source of truth), merge with local cache
  useEffect(() => {
    if (loadedFromApi.current) return;
    loadedFromApi.current = true;

    msalFetch("/api/v1/user/recents")
      .then(r => r.ok ? r.json() : null)
      .then(data => {
        if (!data || !Array.isArray(data)) return;
        const apiItems: RecentItem[] = data.map((d: any) => ({
          id: d.item_id,
          label: d.label,
          href: d.href,
          type: d.type,
          timestamp: new Date(d.created_at).getTime(),
        }));
        if (apiItems.length > 0) {
          // Merge: API items take priority, local items fill gaps
          const merged = [...apiItems];
          const ids = new Set(apiItems.map(i => i.id));
          const local = loadLocal();
          for (const l of local) {
            if (!ids.has(l.id)) {
              merged.push(l);
              // Sync local-only items to API
              msalFetch("/api/v1/user/recents", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ item_id: l.id, label: l.label, href: l.href, type: l.type }),
              }).catch(() => {});
            }
          }
          const final = merged.slice(0, MAX_RECENTS);
          saveLocal(final);
          setRecents(final);
        }
      })
      .catch(() => {});
  }, []);

  // Listen for same-tab sync
  useEffect(() => {
    const reload = () => setRecents(loadLocal());
    window.addEventListener(SYNC_EVENT, reload);
    window.addEventListener("storage", (e) => {
      if (e.key === STORAGE_KEY) reload();
    });
    return () => window.removeEventListener(SYNC_EVENT, reload);
  }, []);

  const addRecent = useCallback((item: Omit<RecentItem, "timestamp">) => {
    const current = loadLocal();
    const filtered = current.filter((r) => r.id !== item.id);
    const next = [{ ...item, timestamp: Date.now() }, ...filtered].slice(0, MAX_RECENTS);
    saveLocal(next);

    // Persist to API (fire and forget)
    msalFetch("/api/v1/user/recents", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ item_id: item.id, label: item.label, href: item.href, type: item.type }),
    }).catch(() => {});
  }, []);

  const removeRecent = useCallback((id: string) => {
    const current = loadLocal();
    const next = current.filter((r) => r.id !== id);
    saveLocal(next);

    // Remove from API (fire and forget)
    msalFetch(`/api/v1/user/recents/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }).catch(() => {});
  }, []);

  // Clear all recents, or just one type — updates local + the per-user DB.
  const clearRecents = useCallback((type?: RecentItem["type"]) => {
    const current = loadLocal();
    saveLocal(type ? current.filter((r) => r.type !== type) : []);
    const qs = type ? `?type=${encodeURIComponent(type)}` : "";
    msalFetch(`/api/v1/user/recents${qs}`, { method: "DELETE" }).catch(() => {});
  }, []);

  return { recents, addRecent, removeRecent, clearRecents };
}
