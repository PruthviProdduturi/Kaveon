"use client";

import { useTheme } from "../contexts/ThemeContext";

interface PaginationProps {
  total: number;
  page: number;
  pageSize: number;
  onChange: (page: number) => void;
}

export function Pagination({ total, page, pageSize, onChange }: PaginationProps) {
  const { primaryColor } = useTheme();
  const totalPages = Math.ceil(total / pageSize);
  if (totalPages <= 1) return null;

  const from = (page - 1) * pageSize + 1;
  const to = Math.min(page * pageSize, total);

  // Build page number list with ellipsis
  const pages: (number | "…")[] = [];
  if (totalPages <= 7) {
    for (let i = 1; i <= totalPages; i++) pages.push(i);
  } else {
    pages.push(1);
    if (page > 3) pages.push("…");
    for (let i = Math.max(2, page - 1); i <= Math.min(totalPages - 1, page + 1); i++) pages.push(i);
    if (page < totalPages - 2) pages.push("…");
    pages.push(totalPages);
  }

  const btn = (active: boolean, disabled?: boolean): React.CSSProperties => ({
    display: "inline-flex", alignItems: "center", justifyContent: "center",
    minWidth: 32, height: 32, padding: "0 0.4rem",
    borderRadius: 7,
    border: active ? "none" : "1px solid #e2e8f0",
    background: active ? primaryColor : "white",
    color: active ? "white" : disabled ? "#cbd5e1" : "#374151",
    fontSize: 13, fontWeight: active ? 600 : 400,
    cursor: disabled ? "not-allowed" : "pointer",
    transition: "all 0.15s",
    opacity: disabled ? 0.45 : 1,
  });

  return (
    <div style={{
      display: "flex", alignItems: "center", justifyContent: "space-between",
      padding: "0.75rem 1rem", borderTop: "1px solid #f1f5f9",
      flexWrap: "wrap", gap: "0.5rem",
    }}>
      <span style={{ fontSize: 13, color: "#64748b" }}>
        {from}–{to} of {total}
      </span>
      <div style={{ display: "flex", gap: "0.25rem", alignItems: "center" }}>
        <button type="button" style={btn(false, page === 1)}
          onClick={() => page > 1 && onChange(page - 1)} disabled={page === 1}
          aria-label="Previous page">
          <i className="fas fa-chevron-left" style={{ fontSize: 10 }} />
        </button>
        {pages.map((p, i) =>
          p === "…" ? (
            <span key={`el-${i}`} style={{ padding: "0 4px", color: "#94a3b8", fontSize: 13, userSelect: "none" }}>…</span>
          ) : (
            <button key={p} type="button" style={btn(p === page)} onClick={() => onChange(p as number)}>
              {p}
            </button>
          )
        )}
        <button type="button" style={btn(false, page === totalPages)}
          onClick={() => page < totalPages && onChange(page + 1)} disabled={page === totalPages}
          aria-label="Next page">
          <i className="fas fa-chevron-right" style={{ fontSize: 10 }} />
        </button>
      </div>
    </div>
  );
}
