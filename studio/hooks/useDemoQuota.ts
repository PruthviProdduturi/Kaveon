"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { msalFetch } from "../utils/msalFetch";
import { quotaLabel, type DemoPosture, type DemoQuota } from "../utils/demoQuota";

const NONE: DemoPosture = { readOnly: false, engine: false, quota: null };

/**
 * The viewer's demo posture: read from `GET /api/v1/engine/quota` on mount,
 * again on `refresh()` (after a live run), and again when the window's
 * oldest charge is due to leave it, so the counter climbs back on its own.
 * A self-hosted install — no quota — costs one request and shows nothing.
 */
export function useDemoQuota(): DemoPosture & { label: string | null; refresh: () => void } {
	const [posture, setPosture] = useState<DemoPosture>(NONE);
	const timer = useRef<number | null>(null);

	const refresh = useCallback(async () => {
		try {
			const res = await msalFetch("/api/v1/engine/quota");
			if (!res.ok) return;
			const body = await res.json();
			const demo = body?.demo ?? {};
			const quota = body?.quota && typeof body.quota === "object" ? (body.quota as DemoQuota) : null;
			setPosture({ readOnly: !!demo.read_only, engine: !!demo.engine, quota });
		} catch {
			// The counter is a convenience; the Engine enforces the quota.
		}
	}, []);

	useEffect(() => {
		void refresh();
	}, [refresh]);

	useEffect(() => {
		if (timer.current !== null) window.clearTimeout(timer.current);
		const resetsAt = posture.quota?.resets_at_ms;
		if (!resetsAt) return;
		const wait = Math.max(1_000, resetsAt - Date.now() + 500);
		timer.current = window.setTimeout(() => void refresh(), Math.min(wait, 2_147_483_647));
		return () => {
			if (timer.current !== null) window.clearTimeout(timer.current);
		};
	}, [posture.quota?.resets_at_ms, refresh]);

	return { ...posture, label: quotaLabel(posture.quota), refresh: () => void refresh() };
}
