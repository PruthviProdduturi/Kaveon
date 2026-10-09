/**
 * Library preview refresh — the pass, and the record of how it went.
 *
 * A preview is a raster read off a rendered chart, so only a browser can make
 * one: the pass opens each chart or dashboard in a hidden same-origin frame
 * with `?capture=1`, that page stores its own preview and posts back, and the
 * runner advances. No server-side scheduler can do this work, because there is
 * nothing on the server that renders ECharts.
 *
 * That constraint decides where a run lives. The runner is a module singleton
 * rather than component state, so a route change does not abandon it — the
 * Maintenance page attaches to whatever is already going and detaches when it
 * unmounts. Progress is mirrored into `localStorage` on every step, so a
 * remount, a reload or a closed tab all leave an honest record instead of
 * nothing: a run still marked `running` whose heartbeat has gone cold is
 * reported as `interrupted`, never as finished.
 *
 * The record is per browser profile, which is the right scope for a fact about
 * what this browser did but not a shared one: how many previews actually exist
 * is read from the API instead (`has_thumbnail`), so coverage is always true
 * for every administrator while the run history is local to the tab that drove
 * it. A shared record would mean a new KaveonDB product family, and that
 * registry is a closed set mid-migration.
 *
 * One capture runs at a time across both passes. Nothing here throws into a
 * caller: a preview is never worth failing a page on.
 */

export type PassId = "dashboards" | "charts";

/** How a run ended, or that it has not. */
export type RunStatus = "running" | "completed" | "stopped" | "interrupted" | "failed";

/** `all` re-captures every record; `outstanding` only what is missing or older than the record it previews. */
export type RunScope = "all" | "outstanding";

export interface CaptureStep {
	/** The record being captured, matched against the frame's completion message. */
	id: string;
	/** Dashboards are captured once per theme; a chart carries one preview. */
	theme?: "light" | "dark";
	label: string;
	src: string;
}

export interface PassRun {
	status: RunStatus;
	scope: RunScope;
	total: number;
	done: number;
	label: string;
	startedAt: number;
	finishedAt: number | null;
	/** Set when the run could not be planned. */
	detail: string | null;
}

/** Longest a single capture is given before the run moves on without it. */
const STEP_TIMEOUT_MS: Record<PassId, number> = { dashboards: 25_000, charts: 35_000 };

/** How often a live run refreshes its heartbeat, so a slow step is not mistaken for a dead tab. */
const HEARTBEAT_MS = 5_000;

const STORAGE_VERSION = 1;

/** Intervals offered for the automatic refresh, in days; 0 is off. */
export const REFRESH_INTERVALS: { days: number; label: string }[] = [
	{ days: 0, label: "Off" },
	{ days: 1, label: "Daily" },
	{ days: 7, label: "Weekly" },
	{ days: 30, label: "Monthly" },
];

interface StoredRun extends PassRun {
	v: number;
	/** The page load that drove the run, so another tab's run is not mistaken for an abandoned one. */
	session: string;
	/** Last time the driving tab was demonstrably alive. */
	beatAt: number;
}

interface LiveRun {
	stopped: boolean;
	pending: { step: CaptureStep; finish: () => void } | null;
}

const SESSION = newSession();
const live = new Map<PassId, LiveRun>();
const listeners = new Map<PassId, Set<(run: PassRun | null) => void>>();

function newSession(): string {
	try {
		return crypto.randomUUID();
	} catch {
		return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
	}
}

/** A run whose heartbeat is older than this, with no live runner, was abandoned. */
function staleAfter(pass: PassId): number {
	return STEP_TIMEOUT_MS[pass] + HEARTBEAT_MS * 4;
}

function runKey(pass: PassId): string {
	return `kaveon.preview-refresh.run.${pass}`;
}

function intervalKey(pass: PassId): string {
	return `kaveon.preview-refresh.interval.${pass}`;
}

function store(): Storage | null {
	try {
		return typeof window === "undefined" ? null : window.localStorage;
	} catch {
		// Storage can be denied outright; the run then simply has no memory of itself.
		return null;
	}
}

function readStored(pass: PassId): StoredRun | null {
	const storage = store();
	if (!storage) return null;
	try {
		const raw = storage.getItem(runKey(pass));
		if (!raw) return null;
		const parsed = JSON.parse(raw) as StoredRun;
		if (!parsed || parsed.v !== STORAGE_VERSION || typeof parsed.startedAt !== "number") return null;
		return parsed;
	} catch {
		return null;
	}
}

function writeStored(pass: PassId, run: StoredRun): void {
	const storage = store();
	try {
		storage?.setItem(runKey(pass), JSON.stringify(run));
	} catch {
		// A full or denied store costs the record, not the run.
	}
	notify(pass, publicRun(run));
}

function publicRun(run: StoredRun): PassRun {
	return {
		status: run.status, scope: run.scope, total: run.total, done: run.done,
		label: run.label, startedAt: run.startedAt, finishedAt: run.finishedAt,
		detail: run.detail,
	};
}

function notify(pass: PassId, run: PassRun | null): void {
	listeners.get(pass)?.forEach((listener) => listener(run));
}

/**
 * The last run of a pass as it should be reported now.
 *
 * A stored `running` record with no runner behind it is reconciled to
 * `interrupted` and persisted, so the state the page shows and the state on
 * disk never disagree.
 */
export function readRun(pass: PassId): PassRun | null {
	const stored = readStored(pass);
	if (!stored) return null;
	if (stored.status !== "running" || live.has(pass)) return publicRun(stored);
	const abandoned = stored.session === SESSION || Date.now() - stored.beatAt > staleAfter(pass);
	if (!abandoned) return publicRun(stored);
	const reconciled: StoredRun = { ...stored, status: "interrupted", finishedAt: stored.beatAt };
	writeStored(pass, reconciled);
	return publicRun(reconciled);
}

/** True while another tab is driving this pass, so this one must not start a second runner. */
export function isRunningElsewhere(pass: PassId): boolean {
	if (live.has(pass)) return false;
	const stored = readStored(pass);
	if (!stored || stored.status !== "running") return false;
	return stored.session !== SESSION && Date.now() - stored.beatAt <= staleAfter(pass);
}

/** True while any pass is capturing in this tab; one capture runs at a time. */
export function isCapturing(): boolean {
	return live.size > 0;
}

export function subscribe(pass: PassId, listener: (run: PassRun | null) => void): () => void {
	const set = listeners.get(pass) ?? new Set();
	set.add(listener);
	listeners.set(pass, set);
	return () => {
		set.delete(listener);
	};
}

/** When the chosen interval has elapsed since the last completed run. */
export function isDue(run: PassRun | null, intervalDays: number, now: number = Date.now()): boolean {
	if (intervalDays <= 0) return false;
	if (!run || run.status !== "completed" || !run.finishedAt) return true;
	return now - run.finishedAt >= intervalDays * 86_400_000;
}

export function readInterval(pass: PassId): number {
	const storage = store();
	const raw = storage?.getItem(intervalKey(pass));
	const days = raw === null || raw === undefined ? 0 : Number(raw);
	return REFRESH_INTERVALS.some((choice) => choice.days === days) ? days : 0;
}

export function writeInterval(pass: PassId, days: number): void {
	try {
		store()?.setItem(intervalKey(pass), String(days));
	} catch {
		// Nothing to do: the interval reverts to off on the next read.
	}
}

/**
 * Timestamps reach the browser both as `2026-10-09T04:20:32.300734Z` and as a
 * naive `2026-08-25T21:08:43.641125`. The API stores UTC either way, so a
 * missing designator is read as UTC rather than as local time — otherwise a
 * record would look hours newer than its preview and never settle.
 */
export function parseTimestamp(value: unknown): number | null {
	if (typeof value !== "string" || !value) return null;
	const normalised = /(?:Z|[+-]\d{2}:?\d{2})$/.test(value) ? value : `${value}Z`;
	const parsed = Date.parse(normalised);
	return Number.isFinite(parsed) ? parsed : null;
}

/** Ask the pass to stop; the capture in flight is abandoned at its next boundary. */
export function stopPass(pass: PassId): void {
	const runner = live.get(pass);
	if (!runner) return;
	runner.stopped = true;
	runner.pending?.finish();
}

/**
 * Drive one pass to completion.
 *
 * Resolves when the run ends — completed, stopped, or unable to plan — with
 * `true` if it ran. One capture happens at a time, so a call made while
 * something else is capturing returns `false` immediately and the caller is
 * free to try again once the lock is free.
 */
export async function startPass(
	pass: PassId,
	scope: RunScope,
	plan: () => Promise<CaptureStep[]>,
): Promise<boolean> {
	if (live.size > 0 || isRunningElsewhere(pass)) return false;

	const runner: LiveRun = { stopped: false, pending: null };
	live.set(pass, runner);

	const startedAt = Date.now();
	let run: StoredRun = {
		v: STORAGE_VERSION, session: SESSION, status: "running", scope,
		total: 0, done: 0, label: "Reading the Library…",
		startedAt, beatAt: startedAt, finishedAt: null, detail: null,
	};
	const commit = (patch: Partial<StoredRun>) => {
		run = { ...run, ...patch, beatAt: Date.now() };
		writeStored(pass, run);
	};
	commit({});

	const heartbeat = window.setInterval(() => {
		if (run.status === "running") commit({});
	}, HEARTBEAT_MS);

	let frame: HTMLIFrameElement | null = null;
	const onMessage = (event: MessageEvent) => {
		// Same-origin only, and matched to the capture actually in flight: a late
		// message from the previous record must not advance the current one.
		if (event.origin !== window.location.origin) return;
		const data = event.data as { type?: string; id?: string; theme?: string } | null;
		if (data?.type !== "kaveon-thumb-done" && data?.type !== "kaveon-chart-thumb-done") return;
		const pending = runner.pending;
		if (!pending || String(data.id) !== pending.step.id) return;
		if (pending.step.theme && data.theme && data.theme !== pending.step.theme) return;
		pending.finish();
	};

	try {
		let steps: CaptureStep[];
		try {
			steps = await plan();
		} catch {
			commit({ status: "failed", finishedAt: Date.now(), label: "The Library could not be read.", detail: "The Library could not be read." });
			return true;
		}
		if (runner.stopped) {
			commit({ status: "stopped", finishedAt: Date.now(), label: "Stopped before any capture." });
			return true;
		}
		if (!steps.length) {
			commit({ status: "completed", total: 0, done: 0, finishedAt: Date.now(), label: "Every preview was already current." });
			return true;
		}

		frame = document.createElement("iframe");
		frame.title = "Library preview capture";
		frame.setAttribute("aria-hidden", "true");
		frame.style.cssText = "position:fixed;left:-9999px;top:0;width:1280px;height:800px;border:0;visibility:hidden;";
		document.body.appendChild(frame);
		window.addEventListener("message", onMessage);

		for (let index = 0; index < steps.length; index++) {
			if (runner.stopped) break;
			const step = steps[index];
			commit({ total: steps.length, done: index, label: step.label });
			await new Promise<void>((resolve) => {
				let settled = false;
				const finish = () => {
					if (settled) return;
					settled = true;
					window.clearTimeout(timer);
					runner.pending = null;
					resolve();
				};
				const timer = window.setTimeout(finish, STEP_TIMEOUT_MS[pass]);
				runner.pending = { step, finish };
				if (frame) frame.src = step.src;
			});
		}

		const done = runner.stopped ? run.done : steps.length;
		commit(runner.stopped
			? { status: "stopped", done, finishedAt: Date.now(), label: `Stopped after ${done} of ${steps.length}.` }
			: { status: "completed", done, total: steps.length, finishedAt: Date.now(), label: `Captured ${steps.length}.` });
	} finally {
		window.clearInterval(heartbeat);
		window.removeEventListener("message", onMessage);
		frame?.remove();
		live.delete(pass);
		notify(pass, readRun(pass));
	}
	return true;
}
