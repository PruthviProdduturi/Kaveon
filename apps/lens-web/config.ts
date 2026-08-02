const raw = process.env.NEXT_PUBLIC_API_BASE_URL;
function normalizeApiBase(value?: string) {
  // Default to localhost for dev, always HTTPS for production
  if (!value) return "http://localhost:8080";
  try {
    const url = new URL(value);
    const host = url.hostname;
    if (host === "localhost" || host === "127.0.0.1") {
      url.protocol = "http:";
    } else {
      url.protocol = "https:";
    }
    return url.toString().replace(/\/+$/, "");
  } catch {
    // If parsing fails, fallback to default localhost http
    return "http://localhost:8080";
  }
}

export const API_BASE = normalizeApiBase(raw);
