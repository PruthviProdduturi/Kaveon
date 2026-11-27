"use client";

/**
 * What every compiled context is serving, and whether it matches its source.
 *
 * Freshness was observable one dataset at a time and correctable fleet-wide,
 * with nothing in between: `GET /datasets/{id}/freshness` answered for one,
 * and `POST /dlm/sweep` rebuilt every stale one it found. An operator asking
 * "what is stale right now?" had to run the sweep to find out, which changed
 * the answer. `GET /dlm/freshness` is the read; this is the page for it.
 *
 * The distinction the table exists to draw is between a context serving older
 * data than its source holds and a context that cannot answer at all. Both
 * are "not fresh" and they need different things: the first is answered live
 * and rebuilt behind the ask, the second is a dataset whose compile never
 * finished.
 */

import React, { useCallback, useEffect, useRef, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { reasonFromStatus, unknownText, ReadReason } from "../../../components/Reading";
import g from "../governance/governance.module.css";
import s from "../settings.module.css";

type Recommendation = "use_context" | "rebuild" | "no_context";

interface Row {
	dataset_id: string;
	fact_table: string | null;
	status: string | null;
	built_at: string | null;
	score: number;
	fresh: boolean;
	recommendation: Recommendation;
	data_modified: boolean;
	signal: string | null;
	rebuilding: boolean;
}

interface Status {
	checked: number;
	stale: number;
	rebuilding: number;
	uncompiled: number;
	datasets: Row[];
	/** Present when the server declined the read rather than finding nothing. */
	skipped?: string;
}

/** The state each row is in, named the way an operator would describe it. */
const STATE: Record<Recommendation, { label: string; tone: string }> = {
	use_context: { label: "Current", tone: g.kindCatalog },
	rebuild: { label: "Behind source", tone: g.kindSettings },
	no_context: { label: "Not answerable", tone: g.kindFailed },
};

/** A score is confidence, not age, so it is shown as one. */
function confidence(score: number): string {
	return `${Math.round(score * 100)}%`;
}

function builtText(at: string | null): string {
	if (!at) return "—";
	const parsed = Date.parse(at.endsWith("Z") ? at : `${at}Z`);
	if (Number.isNaN(parsed)) return at;
	const hours = (Date.now() - parsed) / 3_600_000;
	if (hours < 1) return "under an hour ago";
	if (hours < 48) return `${Math.round(hours)}h ago`;
	return `${Math.round(hours / 24)}d ago`;
}

/** The signal the score was computed from, which decides how much to trust it. */
function signalText(signal: string | null): string {
	if (signal === "engine_source_version") return "source version";
	if (signal === "postgresql_change_counter") return "change counter";
	return "—";
}

export default function ContextFreshness() {
	const [status, setStatus] = useState<Status | null>(null);
	const [problem, setProblem] = useState<string | null>(null);
	const [sweeping, setSweeping] = useState(false);
	const [swept, setSwept] = useState<string | null>(null);
	const mounted = useRef(true);

	useEffect(() => () => { mounted.current = false; }, []);

	const load = useCallback(async () => {
		try {
			const response = await msalFetch(`${API_BASE}/api/v1/dlm/freshness`);
			if (!response.ok) {
				throw Object.assign(new Error("read failed"),
					{ reason: reasonFromStatus(response.status) });
			}
			const payload: Status = await response.json();
			if (!mounted.current) return;
			setStatus(payload);
			setProblem(null);
		} catch (error) {
			if (!mounted.current) return;
			setStatus(null);
			setProblem(unknownText({
				what: "Context freshness",
				says: "which contexts are serving current data",
				reason: (error as { reason?: ReadReason | null }).reason ?? null,
			}));
		}
	}, []);

	useEffect(() => { void load(); }, [load]);

	const sweep = useCallback(async () => {
		setSweeping(true);
		setSwept(null);
		try {
			const response = await msalFetch(`${API_BASE}/api/v1/dlm/sweep`, { method: "POST" });
			const payload = response.ok ? await response.json() : null;
			if (!mounted.current) return;
			// A sweep that found nothing stale and a sweep that could not start
			// both change nothing; only one of them is a success.
			setSwept(payload
				? `Checked ${payload.checked}; ${payload.triggered} rebuild${payload.triggered === 1 ? "" : "s"} started.`
				: "The sweep could not be started.");
			await load();
		} finally {
			if (mounted.current) setSweeping(false);
		}
	}, [load]);

	const rows = status?.datasets ?? [];
	const attention = (status?.stale ?? 0) + (status?.uncompiled ?? 0);

	return (
		<section className={s.card}>
			<div className={s.cardHead}>
				<div className={s.cardId}>
					<div className={s.mark} aria-hidden="true">
						<i className="fas fa-arrows-rotate" />
					</div>
					<div>
						<h2 className={s.cardTitle}>Context freshness</h2>
						<p className={s.cardSub}>
							Every compiled context, and whether the data it answers from still matches
							its source. Reading this page changes nothing: a context is rebuilt when
							its source moves, not when someone looks.
						</p>
					</div>
				</div>
				<div className={s.actions}>
					<button
						type="button"
						className={s.ghost}
						onClick={() => void load()}
						disabled={sweeping}
					>
						Refresh
					</button>
					<button
						type="button"
						className={s.primary}
						onClick={() => void sweep()}
						disabled={sweeping || !attention}
					>
						{sweeping ? "Rebuilding…" : "Rebuild what is behind"}
					</button>
				</div>
			</div>

			<div className={s.facts}>
				<div className={s.fact}>
					<div className={s.factLabel}>Compiled</div>
					<div className={s.factValue}>{status ? status.checked : "—"}</div>
				</div>
				<div className={s.fact}>
					<div className={s.factLabel}>Behind source</div>
					<div className={s.factValue}>{status ? status.stale : "—"}</div>
				</div>
				<div className={s.fact}>
					<div className={s.factLabel}>Rebuilding now</div>
					<div className={s.factValue}>{status ? status.rebuilding : "—"}</div>
				</div>
				<div className={s.fact}>
					<div className={s.factLabel}>Not answerable</div>
					<div className={s.factValue}>{status ? status.uncompiled : "—"}</div>
				</div>
			</div>

			{problem && <p className={g.problem} role="status">{problem}</p>}
			{status?.skipped && <p className={g.problem} role="status">{status.skipped}</p>}
			{swept && <p className={g.saved} role="status">{swept}</p>}

			{!problem && !status?.skipped && (
				rows.length ? (
					<div className={g.tableWrap}>
						<table className={g.table}>
							<thead>
								<tr>
									<th>Dataset</th>
									<th>Table</th>
									<th>State</th>
									<th className={g.num}>Confidence</th>
									<th>Built</th>
									<th>Signal</th>
								</tr>
							</thead>
							<tbody>
								{rows.map((row) => {
									const state = STATE[row.recommendation] ?? STATE.no_context;
									return (
										<tr key={row.dataset_id}>
											<td className={g.mono}>{row.dataset_id}</td>
											<td className={g.mono}>{row.fact_table ?? "—"}</td>
											<td>
												<span className={`${g.kind} ${state.tone}`}>{state.label}</span>
												{row.rebuilding && (
													<span className={g.detail}> rebuilding</span>
												)}
											</td>
											<td className={`${g.num} ${row.fresh ? "" : g.counterHot}`}>
												{confidence(row.score)}
											</td>
											<td>{builtText(row.built_at)}</td>
											<td className={g.detail}>{signalText(row.signal)}</td>
										</tr>
									);
								})}
							</tbody>
						</table>
					</div>
				) : (
					<div className={g.empty}>
						{status
							? "No dataset has compiled a context yet. One compiles when a dataset is created."
							: "Reading context freshness…"}
					</div>
				)
			)}

			<div className={s.schedule}>
				<p className={s.note}>
					<b>Nothing rebuilds on a clock.</b> A question against a context that is behind
					its source is answered with a live query and the context is rebuilt behind the
					answer, so a stale context is corrected by being used. The scheduled sweep is
					the <b>kaveon-dlm-freshness</b> job, which is off unless a cluster enables it and
					names an account for it to run as. Use the button above to rebuild without
					waiting for a question, or have a loading pipeline call
					<b> POST /dlm/notify-data-change</b> when it finishes.
				</p>
			</div>
		</section>
	);
}
