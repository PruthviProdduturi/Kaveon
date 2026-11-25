export async function readError(res: Response, fallback: string): Promise<string> {
  try {
    const body = await res.json();
    const detail = body?.detail ?? body?.error ?? body?.message;
    if (typeof detail === "string") return detail;
    if (detail && typeof detail.message === "string") return detail.message;
  } catch { /* not JSON */ }
  return `${fallback} (HTTP ${res.status})`;
}
