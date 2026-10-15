"use client";

import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import {
	CaptureStep, PassId, PassRun, REFRESH_INTERVALS, RunScope, RunStatus,
	isDue, isRunningElsewhere, parseTimestamp, readInterval, readRun,
	startPass, stopPass, subscribe, writeInterval,
} from "../../../utils/previewRefresh";
import { unknownText } from "../../../components/Reading";
import g from "../governance/governance.module.css";
import s from "../settings.module.css";

/** One preview slot: whether it exists, and when it was captured. */
interface PreviewSlot {
	stored: boolean;
	/** Null on a record whose preview predates the stored capture time. */
	capturedAt: number | null;
}

/** One record as the pass needs to see it: its previews, and when it last changed. */
interface PreviewRecord {
	id: string;
	name: string;
	updatedAt: number | null;
	light: PreviewSlot;
	dark: PreviewSlot;
}

interface PassCopy {
	title: string;
	description: string;
	noun: string;
	plural: string;
	scheduleHelp: string;
}

const COPY: Record<PassId, PassCopy> = {
	dashboards: {
		title: "Dashboard previews",
		description:
			"Each dashboard is captured once per theme, so the Library shows the preview that matches the viewer's theme. "
			+ "A capture runs in this browser: it keeps going while you move around Studio, and is recorded as interrupted if the tab is closed or reloaded.",
		noun: "dashboard",
		plural: "dashboards",
		scheduleHelp:
			"When an administrator opens this page and the previews are older than the chosen interval, the outstanding ones are captured here. "
			+ "A preview is read off a rendered chart, so there is no server-side schedule that can run it unattended. "
			+ "Storing a preview needs permission to edit the dashboard, so a run driven by an account that cannot edit one reports it as unstored.",
	},
	charts: {
		title: "Chart previews",
		description:
			"A chart's preview is captured when the chart is saved, so this pass covers charts with no preview and charts edited since theirs was taken. "
			+ "Each capture runs the chart's query, so a full refresh takes several minutes.",
		noun: "chart",
		plural: "charts",
		scheduleHelp:
			"When an administrator opens this page and the previews are older than the chosen interval, the outstanding ones are captured here. "
			+ "Captures run one at a time and only for charts that need one, so the pass never floods the Engine.",
	},
};

async function loadRecords(pass: PassId): Promise<PreviewRecord[]> {
	const response = await msalFetch(`${API_BASE}/api/v1/${pass}`);
	if (!response.ok) throw new Error(`${pass} could not be loaded`);
	const payload = await response.json();
	const rows: Record<string, unknown>[] = Array.isArray(payload)
		? payload
		: (payload.result as Record<string, unknown>[]) || (payload.items as Record<string, unknown>[]) || [];
	return rows.map((row) => ({
		id: String(row.id),
		name: typeof row.name === "string" && row.name ? row.name : "Untitled",
		updatedAt: parseTimestamp(row.updated_at),
		light: { stored: Boolean(row.has_thumbnail), capturedAt: parseTimestamp(row.thumbnail_updated_at) },
		dark: { stored: Boolean(row.has_thumbnail_dark), capturedAt: parseTimestamp(row.thumbnail_dark_updated_at) },
	}));
}

/**
 * A preview is outstanding when there is none, or when the record it previews
 * changed after that preview was captured.
 *
 * The record's own capture time is the fact that decides it, so the answer is
 * the same for every administrator and survives a cleared browser. A preview
 * stored before the API recorded capture times carries none; that is read as
 * "cannot tell" and the preview is left alone, so nothing is re-captured on a
 * timestamp the pass cannot interpret.
 *
 * `baseline` — the finish time of a run this browser completed — is the older,
 * weaker answer, kept only for a record whose capture time is unknown. It was
 * the only answer, which is why a fresh browser could see a Library full of
 * stale previews and still be told there was nothing outstanding.
 */
function outstanding(slot: PreviewSlot, updatedAt: number | null, baseline: number | null): boolean {
	if (!slot.stored) return true;
	if (updatedAt === null) return false;
	if (slot.capturedAt !== null) return updatedAt > slot.capturedAt;
	return baseline !== null && updatedAt > baseline;
}

/**
 * The moment after which an existing preview can be called current.
 *
 * Only a run that stored everything it planned earns one. A run that captured
 * nothing — every PUT refused, say — used to be recorded as completed, and its
 * finish time then became the baseline that declared every stale preview
 * current, so the pass had nothing left to offer and the Library stayed as it
 * was. A run with unstored captures leaves the baseline alone.
 */
function baselineOf(run: PassRun | null): number | null {
	return run && run.status === "completed" && !run.failed ? run.finishedAt : null;
}

function planSteps(
	pass: PassId,
	records: PreviewRecord[],
	scope: RunScope,
	baseline: number | null,
): CaptureStep[] {
	if (pass === "charts") {
		return records
			.filter((record) => scope === "all" || outstanding(record.light, record.updatedAt, baseline))
			.map((record) => ({ id: record.id, label: record.name, src: `/charts/${record.id}?capture=1` }));
	}
	const themes: { theme: "light" | "dark"; label: string }[] = [
		{ theme: "light", label: "Light theme" },
		{ theme: "dark", label: "Dark theme" },
	];
	return records.flatMap((record) =>
		themes
			.filter(({ theme }) => scope === "all"
				|| outstanding(theme === "dark" ? record.dark : record.light, record.updatedAt, baseline))
			.map(({ theme, label }) => ({
				id: record.id, theme, label: `${record.name} · ${label}`,
				src: `/dashboards/${record.id}/view?capture=1&forceTheme=${theme}`,
			})),
	);
}

function formatMoment(at: number): string {
	try {
		return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(at);
	} catch {
		return new Date(at).toISOString();
	}
}

function outcomeOf(run: PassRun | null): string {
	if (!run) return "No run recorded";
	switch (run.status) {
		case "running": return `In progress · ${run.done} of ${run.total || "?"}`;
		case "completed":
			if (!run.total) return "Completed · nothing outstanding";
			return run.failed
				? `Completed · ${run.total - run.failed} of ${run.total} captured`
				: `Completed · ${run.total} captured`;
		case "stopped": return `Stopped · ${run.done} of ${run.total}`;
		case "interrupted": return `Interrupted · ${run.done} of ${run.total}`;
		case "failed": return run.total ? `Failed · none of ${run.total} could be stored` : "Could not start";
	}
}

/**
 * The Maintenance view of one pass.
 *
 * The run itself lives in `utils/previewRefresh`, outside React, so this hook
 * only attaches to it: it reads the stored record on mount, follows the live
 * run while the page is open, and re-reads coverage from the API when a run
 * ends. `blocked` is the other pass capturing — one capture at a time.
 */
function usePass(pass: PassId, blocked: boolean) {
	const [run, setRun] = useState<PassRun | null>(null);
	const [elsewhere, setElsewhere] = useState(false);
	const [intervalDays, setIntervalDays] = useState(0);
	const [records, setRecords] = useState<PreviewRecord[] | null>(null);
	const [problem, setProblem] = useState<string | null>(null);
	const [scheduled, setScheduled] = useState(false);
	const runRef = useRef<PassRun | null>(null);
	const autoRef = useRef(false);
	const mountedRef = useRef(true);

	// Set on the way in as well as cleared on the way out: React remounts an
	// effect in development, and a ref that is only ever cleared would leave
	// every later state update discarded.
	useEffect(() => {
		mountedRef.current = true;
		return () => { mountedRef.current = false; };
	}, []);

	const sync = useCallback((next: PassRun | null) => {
		runRef.current = next;
		if (!mountedRef.current) return;
		setRun(next);
		setElsewhere(next?.status === "running" ? isRunningElsewhere(pass) : false);
	}, [pass]);

	useEffect(() => {
		sync(readRun(pass));
		setIntervalDays(readInterval(pass));
		return subscribe(pass, sync);
	}, [pass, sync]);

	// A run driven by another tab publishes no events here, and a tab that dies
	// publishes nothing at all. So while the record says a run is live and it is
	// not this tab's, re-read it: that both follows the other tab's progress and
	// lets the heartbeat go cold, which is what turns an abandoned run into an
	// interrupted one instead of leaving the page waiting on a tab that is gone.
	useEffect(() => {
		if (!elsewhere) return;
		const timer = window.setInterval(() => sync(readRun(pass)), 4000);
		return () => window.clearInterval(timer);
	}, [elsewhere, pass, sync]);

	const reload = useCallback(async () => {
		try {
			const loaded = await loadRecords(pass);
			if (!mountedRef.current) return;
			setRecords(loaded);
			setProblem(null);
		} catch {
			if (!mountedRef.current) return;
			// Not an empty Library. Stored as `[]`, `ready` went true and the
			// card reported "Previews stored — 0 of 0", "Outstanding captures
			// — 0" and "There are no dashboards to preview" over a Library the
			// page had simply failed to read — the figures and the verdict both
			// made out of a request that did not answer.
			setRecords(null);
			setProblem(unknownText({
				what: "The Library",
				says: `which ${COPY[pass].plural} have previews`,
			}));
		}
	}, [pass]);

	useEffect(() => { void reload(); }, [reload]);

	// The baseline is read once, before the run starts, and handed to the plan.
	// Read inside `plan` it would come from the run already in flight — whose
	// status is `running`, never `completed` — so the plan silently used no
	// baseline at all and could come back empty for a button the page had just
	// reported as having work to do.
	const begin = useCallback((scope: RunScope): Promise<boolean> => {
		const baseline = baselineOf(runRef.current);
		return startPass(pass, scope, async () =>
			planSteps(pass, await loadRecords(pass), scope, baseline));
	}, [pass]);

	// Coverage is re-read when a run ends rather than when the call that started
	// it resolves: a run survives leaving the page, so the instance that pressed
	// the button is often no longer the one on screen when it finishes.
	const statusRef = useRef<RunStatus | null | undefined>(undefined);
	const status = run?.status ?? null;
	useEffect(() => {
		const previous = statusRef.current;
		statusRef.current = status;
		if (previous === "running" && status !== "running") void reload();
	}, [status, reload]);

	const baseline = baselineOf(run);
	const pending = useMemo(
		() => (records ? planSteps(pass, records, "outstanding", baseline) : []),
		[pass, records, baseline],
	);
	const stored = useMemo(
		() => (records ?? []).filter((record) => record.light.stored || record.dark.stored).length,
		[records],
	);

	// The automatic refresh. It fires only from this page, only once the chosen
	// interval has elapsed, only for what is outstanding, and only when nothing
	// else is capturing — and it releases the attempt if the lock was already
	// taken, so the other pass finishes first and this one starts when it frees.
	const live = run?.status === "running" && !elsewhere;
	useEffect(() => {
		if (!records || intervalDays <= 0 || live || blocked || elsewhere) return;
		if (autoRef.current || !pending.length || !isDue(run, intervalDays)) return;
		autoRef.current = true;
		setScheduled(true);
		void (async () => {
			if (!(await begin("outstanding")) && mountedRef.current) {
				autoRef.current = false;
				setScheduled(false);
			}
		})();
	}, [records, intervalDays, live, blocked, elsewhere, pending.length, run, begin]);

	const chooseInterval = useCallback((days: number) => {
		writeInterval(pass, days);
		setIntervalDays(days);
	}, [pass]);

	return {
		pass, run, live, elsewhere, blocked, intervalDays, problem, scheduled, reload,
		total: records?.length ?? 0, stored, pending: pending.length,
		ready: records !== null, begin, chooseInterval,
		stop: useCallback(() => stopPass(pass), [pass]),
	};
}

type Pass = ReturnType<typeof usePass>;

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

/** The sentence under the facts: what is happening, or what happened last. */
function statusLine(pass: Pass): string | null {
	const { run, live, elsewhere, scheduled, pending, ready } = pass;
	const copy = COPY[pass.pass];
	if (elsewhere) {
		const progress = run?.total ? ` · ${run.done} of ${run.total}` : "";
		return `A refresh of these previews is already in progress${progress}.`
			+ " It is being driven by another tab; if that tab has gone, this clears within a minute.";
	}
	if (live && run) {
		const progress = run.total ? ` · ${run.done} of ${run.total}` : "";
		const prefix = scheduled ? "Scheduled refresh — " : "";
		return `${prefix}${run.label}${progress}`;
	}
	if (run?.status === "interrupted") {
		return `The last run was interrupted after ${run.done} of ${run.total} — the tab was closed or reloaded before it finished.`
			+ (pending ? " Resume to capture what is left." : "");
	}
	if (run?.status === "failed") {
		if (!run.total) return run.detail ?? "The last run could not be started.";
		return `None of the ${run.total} captures in the last run could be stored.`
			+ (run.detail ? ` ${run.detail}` : "");
	}
	if (run?.status === "stopped") {
		return `The last run was stopped after ${run.done} of ${run.total}.`
			+ (pending ? " Resume to capture what is left." : "");
	}
	// A completed run that could not store some of its captures says so, rather
	// than reporting a refresh that changed nothing as a success.
	if (run?.status === "completed" && run.failed) {
		return `The last run captured ${run.total - run.failed} of ${run.total}; ${run.failed} could not be stored.`
			+ (run.detail ? ` ${run.detail}` : "");
	}
	// A read that did not answer is finished and unknown, so the card must not
	// sit in "Reading the Library…" either, and must not fall through to the
	// counts below — which are counts of nothing read.
	if (pass.problem) return null;
	if (!ready) return "Reading the Library…";
	if (!pass.total) return `There are no ${copy.plural} to preview.`;
	if (!pending) return "Every preview is current.";
	return `${pending} ${pending === 1 ? "capture is" : "captures are"} outstanding.`;
}

const PassCard: React.FC<{ pass: Pass; icon: React.ReactNode }> = ({ pass, icon }) => {
	const copy = COPY[pass.pass];
	const { run, live, elsewhere, blocked, pending, intervalDays } = pass;
	const resumable = run?.status === "interrupted" || run?.status === "stopped";
	const unavailable = elsewhere || blocked || !pass.ready;
	// The bar follows any live run, including one another tab is driving.
	const active = run?.status === "running";
	const percent = active && run?.total ? Math.round((run.done / run.total) * 100) : 0;
	const status = statusLine(pass);

	return (
		<section className={s.card}>
			<div className={s.cardHead}>
				<div className={s.cardId}>
					<div className={s.mark}>{icon}</div>
					<div>
						<h2 className={s.cardTitle}>{copy.title}</h2>
						<p className={s.cardSub}>{copy.description}</p>
					</div>
				</div>
				<div className={s.actions}>
					{live ? (
						<button type="button" className={s.ghost} onClick={pass.stop}>Stop</button>
					) : (
						<>
							<button
								type="button"
								className={s.ghost}
								onClick={() => void pass.begin("all")}
								disabled={unavailable || !pass.total}
							>
								Refresh all
							</button>
							<button
								type="button"
								className={`${s.ghost} ${s.primary}`}
								onClick={() => void pass.begin("outstanding")}
								disabled={unavailable || !pending}
							>
								{resumable && pending ? "Resume" : "Refresh outstanding"}
							</button>
						</>
					)}
				</div>
			</div>

			<div className={s.facts}>
				<div className={s.fact}>
					<div className={s.factLabel}>Previews stored</div>
					<div className={s.factValue}>{pass.ready ? `${pass.stored} of ${pass.total}` : "—"}</div>
				</div>
				<div className={s.fact}>
					<div className={s.factLabel}>Outstanding captures</div>
					<div className={s.factValue}>{pass.ready ? pending : "—"}</div>
				</div>
				<div className={s.fact}>
					<div className={s.factLabel}>Last refreshed</div>
					<div className={s.factValue}>
						{run?.finishedAt ? formatMoment(run.finishedAt) : "Not yet in this browser"}
					</div>
				</div>
				<div className={s.fact}>
					<div className={s.factLabel}>Last run</div>
					<div className={s.factValue}>{outcomeOf(run)}</div>
				</div>
			</div>

			{active && run?.total ? (
				<div
					className={s.progress}
					role="progressbar"
					aria-valuemin={0}
					aria-valuemax={100}
					aria-valuenow={percent}
					aria-label={`${copy.title} refresh progress`}
				>
					<span className={s.progressFill} style={{ width: `${percent}%` }} />
				</div>
			) : null}

			{status ? (
				<div className={s.cardFoot}>
					<p className={s.note} role="status" aria-live="polite">{status}</p>
				</div>
			) : null}

			{pass.problem ? (
				<div className={s.cardFoot}>
					<p className={s.note} role="status">{pass.problem}</p>
					<button type="button" className={s.ghost} onClick={() => void pass.reload()}>Try again</button>
				</div>
			) : null}

			<div className={s.schedule}>
				<div>
					<div className={s.rowLabel}>Automatic refresh</div>
					<div className={s.rowHelp}>{copy.scheduleHelp}</div>
				</div>
				<select
					className={g.select}
					value={intervalDays}
					onChange={(event) => pass.chooseInterval(Number(event.target.value))}
					aria-label={`Automatic refresh interval for ${copy.title.toLowerCase()}`}
				>
					{REFRESH_INTERVALS.map((choice) => (
						<option key={choice.days} value={choice.days}>{choice.label}</option>
					))}
				</select>
			</div>
		</section>
	);
};

export default function MaintenancePage() {
	// Both passes drive hidden frames in this tab, so only one may capture at a
	// time; each card is told whether the other is busy.
	const [dashboardsBusy, setDashboardsBusy] = useState(false);
	const [chartsBusy, setChartsBusy] = useState(false);
	const dashboards = usePass("dashboards", chartsBusy);
	const charts = usePass("charts", dashboardsBusy);

	useEffect(() => { setDashboardsBusy(dashboards.live); }, [dashboards.live]);
	useEffect(() => { setChartsBusy(charts.live); }, [charts.live]);

	return (
		<div className={s.stack}>
			<PassCard pass={dashboards} icon={<ImagesIcon />} />
			<PassCard pass={charts} icon={<ChartIcon />} />
		</div>
	);
}
