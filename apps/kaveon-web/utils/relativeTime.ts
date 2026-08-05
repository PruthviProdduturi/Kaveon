/**
 * Human-friendly relative time: "just now", "5 minutes ago", "2 days ago",
 * then falls back to an absolute date ("3 Aug 2026") for anything older than a
 * week. Accepts an ISO string / Date / null and never shows raw milliseconds.
 */
export function relativeTime(value?: string | number | Date | null): string {
  if (!value) return "—";
  const d = value instanceof Date ? value : new Date(value);
  const ms = d.getTime();
  if (Number.isNaN(ms)) return "—";

  const diff = Date.now() - ms;
  const sec = Math.round(diff / 1000);
  const min = Math.round(sec / 60);
  const hr = Math.round(min / 60);
  const day = Math.round(hr / 24);

  if (sec < 0) return formatAbsolute(d);       // future timestamp → show the date
  if (sec < 45) return "just now";
  if (min < 60) return `${min} minute${min === 1 ? "" : "s"} ago`;
  if (hr < 24) return `${hr} hour${hr === 1 ? "" : "s"} ago`;
  if (day < 7) return `${day} day${day === 1 ? "" : "s"} ago`;
  return formatAbsolute(d);
}

function formatAbsolute(d: Date): string {
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}
