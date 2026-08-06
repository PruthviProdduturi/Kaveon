"use client";

import React from "react";
import { useRouter, usePathname } from "next/navigation";
import { useTabContext, Tab } from "../contexts/TabContext";

const TYPE_ICONS: Record<Tab["type"], string> = {
  dashboard: "📊",
  chart: "📈",
  dataset: "🗂",
  query: "⌨️",
  page: "📄",
};

export function TabBar() {
  const { tabs, activeTabId, setActiveTab, closeTab } = useTabContext();
  const router = useRouter();
  const pathname = usePathname();

  if (tabs.length === 0) return null;

  const handleClick = (tab: Tab) => {
    setActiveTab(tab.id);
    router.push(tab.href);
  };

  const handleClose = (e: React.MouseEvent, id: string) => {
    e.stopPropagation();
    const tab = tabs.find((t) => t.id === id);
    closeTab(id);
    // If closing active tab, the context picks the next one
    if (id === activeTabId) {
      const remaining = tabs.filter((t) => t.id !== id);
      if (remaining.length > 0) {
        const idx = Math.min(tabs.findIndex((t) => t.id === id), remaining.length - 1);
        router.push(remaining[Math.max(0, idx)].href);
      } else {
        router.push("/workspace");
      }
    }
  };

  return (
    <div
      style={{
        display: "flex",
        alignItems: "stretch",
        gap: 0,
        borderBottom: "1px solid var(--border)",
        background: "var(--bg-surface)",
        height: 38,
        flexShrink: 0,
        overflow: "hidden",
        paddingLeft: 4,
      }}
    >
      {tabs.map((tab) => {
        const isActive = tab.href === pathname || tab.id === activeTabId;
        return (
          <div
            key={tab.id}
            onClick={() => handleClick(tab)}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 6,
              padding: "0 12px",
              fontSize: 12,
              fontWeight: isActive ? 500 : 400,
              color: isActive ? "var(--text-primary)" : "var(--text-muted)",
              background: isActive ? "var(--bg-primary)" : "transparent",
              borderRight: "1px solid var(--border)",
              borderBottom: isActive ? "2px solid var(--accent)" : "2px solid transparent",
              cursor: "pointer",
              transition: "all 0.1s",
              maxWidth: 200,
              minWidth: 0,
              whiteSpace: "nowrap",
              position: "relative",
            }}
          >
            <span style={{ fontSize: 11, opacity: 0.6 }}>{TYPE_ICONS[tab.type]}</span>
            <span
              style={{
                overflow: "hidden",
                textOverflow: "ellipsis",
                flex: 1,
                minWidth: 0,
              }}
            >
              {tab.label}
            </span>
            <button
              type="button"
              onClick={(e) => handleClose(e, tab.id)}
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                width: 18,
                height: 18,
                borderRadius: 4,
                border: "none",
                background: "transparent",
                color: "var(--text-muted)",
                cursor: "pointer",
                fontSize: 14,
                lineHeight: 1,
                flexShrink: 0,
                transition: "background 0.1s",
              }}
              onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-hover)")}
              onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
            >
              ×
            </button>
          </div>
        );
      })}
    </div>
  );
}

export default TabBar;
