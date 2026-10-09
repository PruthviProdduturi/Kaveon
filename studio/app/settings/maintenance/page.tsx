"use client";

import React, { useCallback, useRef, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import s from "../settings.module.css";

interface Job { running: boolean; done: number; total: number; label: string }

interface CaptureStep { label: string; src: string }

/**
 * A Library preview is captured in the browser once the chart or dashboard has
 * finished rendering, which normally happens when it is saved. These jobs cover
 * the records that were never re-saved: each one is opened in a hidden frame
 * with ?capture=1, the page stores its own preview and posts a completion
 * message, and the runner advances. A step that never reports is given up on
 * after its timeout rather than stalling the job.
 */
function useCaptureJob(doneMessage: string, stepTimeoutMs: number) {
	const [job, setJob] = useState<Job | null>(null);
	const runningRef = useRef(false);

	const run = useCallback(async (
		loadingLabel: string,
		emptyLabel: string,
		failedLabel: string,
		plan: () => Promise<CaptureStep[]>,
	) => {
		if (runningRef.current) return;
		runningRef.current = true;
		setJob({ running: true, done: 0, total: 0, label: loadingLabel });

		let steps: CaptureStep[] = [];
		try {
			steps = await plan();
		} catch {
			runningRef.current = false;
			setJob({ running: false, done: 0, total: 0, label: failedLabel });
			setTimeout(() => setJob(null), 5000);
			return;
		}
		if (!steps.length) {
			runningRef.current = false;
			setJob({ running: false, done: 0, total: 0, label: emptyLabel });
			setTimeout(() => setJob(null), 5000);
			return;
		}

		const frame = document.createElement("iframe");
		frame.style.cssText = "position:fixed;left:-9999px;top:0;width:1280px;height:800px;border:0;visibility:hidden;";
		document.body.appendChild(frame);
		let resolveDone: (() => void) | null = null;
		const onMessage = (e: MessageEvent) => {
			if (e.data?.type === "kaveon-thumb-done" || e.data?.type === "kaveon-chart-thumb-done") resolveDone?.();
		};
		window.addEventListener("message", onMessage);
		try {
			for (let i = 0; i < steps.length; i++) {
				setJob({ running: true, done: i, total: steps.length, label: steps[i].label });
				await new Promise<void>((resolve) => {
					const finish = () => { clearTimeout(timer); resolveDone = null; resolve(); };
					const timer = setTimeout(finish, stepTimeoutMs);
					resolveDone = finish;
					frame.src = steps[i].src;
				});
			}
			setJob({ running: false, done: steps.length, total: steps.length, label: `${doneMessage} ${steps.length}.` });
		} finally {
			window.removeEventListener("message", onMessage);
			frame.remove();
			runningRef.current = false;
			setTimeout(() => setJob(null), 6000);
		}
	}, [doneMessage, stepTimeoutMs]);

	return { job, run };
}

async function loadRecords(path: string): Promise<{ id: string; name: string }[]> {
	const res = await msalFetch(`${API_BASE}/api/v1/${path}`);
	if (!res.ok) throw new Error(`${path} could not be loaded`);
	const data = await res.json();
	const rows = Array.isArray(data) ? data : data.result || data.items || [];
	return rows.map((row: { id: string | number; name?: string }) => ({
		id: String(row.id), name: row.name || "Untitled",
	}));
}

function ImagesIcon() {
	return (
		<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
			<rect x="3" y="7" width="14" height="13" rx="2" />
			<path d="M7 4h12a2 2 0 0 1 2 2v11" />
			<circle cx="7.5" cy="11.5" r="1.3" />
			<path d="M3 17l4.2-4.2a1.6 1.6 0 0 1 2.2 0L17 20" />
		</svg>
	);
}

function ChartIcon() {
	return (
		<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
			<path d="M4 20V4" />
			<path d="M4 20h16" />
			<rect x="7.5" y="12" width="3" height="5" rx="0.7" />
			<rect x="13.5" y="8" width="3" height="9" rx="0.7" />
		</svg>
	);
}

const JobStatus: React.FC<{ job: Job | null }> = ({ job }) => {
	if (!job) return null;
	return (
		<div style={{ marginTop: 12 }} role="status" aria-live="polite">
			<p className={s.note}>{job.label}{job.running && job.total ? ` · ${job.done} of ${job.total}` : ""}</p>
			{job.running && job.total > 0 && (
				<div className={s.progress}><span className={s.progressFill} style={{ width: `${Math.round((job.done / job.total) * 100)}%` }} /></div>
			)}
		</div>
	);
};

export default function MaintenancePage() {
	// A dashboard is captured once per theme, so the Library can show the preview
	// that matches the viewer's theme; a chart carries one preview.
	const dashboards = useCaptureJob("Refreshed", 25000);
	const charts = useCaptureJob("Refreshed", 35000);

	const refreshDashboards = () => dashboards.run(
		"Loading dashboards…",
		"There are no dashboards to refresh.",
		"Dashboards could not be loaded.",
		async () => (await loadRecords("dashboards")).flatMap((d) =>
			(["light", "dark"] as const).map((theme) => ({
				label: `${d.name} · ${theme}`,
				src: `/dashboards/${d.id}/view?capture=1&forceTheme=${theme}`,
			})),
		),
	);

	const refreshCharts = () => charts.run(
		"Loading charts…",
		"There are no charts to refresh.",
		"Charts could not be loaded.",
		async () => (await loadRecords("charts")).map((c) => ({
			label: c.name,
			src: `/charts/${c.id}?capture=1`,
		})),
	);

	return (
		<div className={s.stack}>
			<section className={s.card}>
				<div className={s.cardHead}>
					<div className={s.cardId}>
						<div className={s.mark}><ImagesIcon /></div>
						<div>
							<h2 className={s.cardTitle}>Dashboard previews</h2>
							<p className={s.cardSub}>Re-capture every dashboard preview in light and dark. Runs in this browser tab; leave it open until it finishes.</p>
						</div>
					</div>
					<button type="button" className={`${s.ghost} ${s.primary}`} onClick={refreshDashboards} disabled={!!dashboards.job?.running}>
						{dashboards.job?.running ? "Refreshing…" : "Refresh dashboard previews"}
					</button>
				</div>
				<JobStatus job={dashboards.job} />
			</section>

			<section className={s.card}>
				<div className={s.cardHead}>
					<div className={s.cardId}>
						<div className={s.mark}><ChartIcon /></div>
						<div>
							<h2 className={s.cardTitle}>Chart previews</h2>
							<p className={s.cardSub}>Re-capture every chart preview. A chart is normally captured when it is saved, so this is for charts that have not been re-saved since. Each one runs its query, so allow a few minutes.</p>
						</div>
					</div>
					<button type="button" className={`${s.ghost} ${s.primary}`} onClick={refreshCharts} disabled={!!charts.job?.running}>
						{charts.job?.running ? "Refreshing…" : "Refresh chart previews"}
					</button>
				</div>
				<JobStatus job={charts.job} />
			</section>
		</div>
	);
}
