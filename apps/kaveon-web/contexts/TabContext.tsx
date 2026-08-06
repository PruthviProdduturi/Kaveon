"use client";

import React, { createContext, useContext, useState, useCallback, ReactNode } from "react";

export interface Tab {
  id: string;
  label: string;
  href: string;
  type: "dashboard" | "chart" | "dataset" | "query" | "page";
}

interface TabContextType {
  tabs: Tab[];
  activeTabId: string | null;
  openTab: (tab: Omit<Tab, "id">) => void;
  closeTab: (id: string) => void;
  setActiveTab: (id: string) => void;
}

const TabContext = createContext<TabContextType | undefined>(undefined);

export function useTabContext(): TabContextType {
  const context = useContext(TabContext);
  if (!context) throw new Error("useTabContext must be used within TabProvider");
  return context;
}

export function TabProvider({ children }: { children: ReactNode }) {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);

  const openTab = useCallback((tab: Omit<Tab, "id">) => {
    const id = tab.href;
    setTabs((prev) => {
      const existing = prev.find((t) => t.id === id);
      if (existing) return prev;
      return [...prev, { ...tab, id }];
    });
    setActiveTabId(id);
  }, []);

  const closeTab = useCallback((id: string) => {
    setTabs((prev) => {
      const idx = prev.findIndex((t) => t.id === id);
      const next = prev.filter((t) => t.id !== id);
      if (id === activeTabId && next.length > 0) {
        const newIdx = Math.min(idx, next.length - 1);
        setActiveTabId(next[newIdx].id);
      } else if (next.length === 0) {
        setActiveTabId(null);
      }
      return next;
    });
  }, [activeTabId]);

  const setActiveTab = useCallback((id: string) => {
    setActiveTabId(id);
  }, []);

  return (
    <TabContext.Provider value={{ tabs, activeTabId, openTab, closeTab, setActiveTab }}>
      {children}
    </TabContext.Provider>
  );
}
