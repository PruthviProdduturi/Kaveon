export const dynamic = "force-dynamic";

const API_BASE = (process.env.API_URL || "http://localhost:8080").replace(/\/+$/, "");
const PROXY_SECRET = process.env.KAVEON_PROXY_SECRET || "";

export async function GET() {
	const headers: Record<string, string> = {
		"x-user-email": "diag@kaveon.app",
		"x-user-name": "Diagnostic",
		"x-user-role": "Admin",
		"x-user-roles": "Admin",
	};
	if (PROXY_SECRET) headers["x-proxy-secret"] = PROXY_SECRET;

	try {
		const res = await fetch(`${API_BASE}/api/v1/dashboards`, { headers });
		if (!res.ok) return Response.json({ error: res.status, text: await res.text() });
		const data = await res.json();
		const items = Array.isArray(data) ? data : [];
		return Response.json({
			count: items.length,
			names: items.map((d: { name?: string }) => d.name),
		});
	} catch (e: unknown) {
		return Response.json({ error: String(e) }, { status: 500 });
	}
}
