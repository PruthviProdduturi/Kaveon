"use client";

/**
 * The words readers used that their dataset could not place.
 *
 * Everything structural about a dataset is derived when it is created --
 * dimensions, measures, time grain, value index, all from the Engine's
 * statistics. The one thing no statistic can supply is the vocabulary of the
 * organisation reading it: that a column is "the number", that "churn" here
 * means cancelled accounts, the product names and the acronyms.
 *
 * That gap used to be invisible, because an unmapped word was answered with
 * whichever measure was the default. It is now refused by name, so every gap
 * arrives attached to a real question someone asked -- which is a better
 * curation brief than any upfront modelling exercise, and the reason this page
 * is a queue rather than a form.
 *
 * Each row is answered in place: map the word to a measure or a dimension, or
 * dismiss it. A mapping writes the same curated alias the context page writes,
 * so it takes effect on the next question.
 */

import React, { useCallback, useEffect, useRef, useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { reasonFromStatus, unknownText, ReadReason } from "../../../components/Reading";
import g from "../governance/governance.module.css";
import s from "../settings.module.css";

interface Gap {
	dataset_id: string;
	dataset_name: string | null;
	term: string;
	count: number;
	askers: number;
	last_seen: number;
	examples: string[];
	measures: string[];
	dimensions: string[];
	closest: string[];
}

interface Queue {
	terms: number;
	datasets: number;
	gaps: Gap[];
	note?: string;
	skipped?: string;
}

/** A target as the select carries it: the element kind and its name. */
type Target = { kind: "metric" | "dimension"; name: string };

function parseTarget(value: string): Target | null {
	const [kind, ...rest] = value.split(":");
	const name = rest.join(":");
	if (!name || (kind !== "metric" && kind !== "dimension")) return null;
	return { kind, name };
}

function seenText(at: number): string {
	const minutes = (Date.now() - at * 1000) / 60_000;
	if (minutes < 60) return "in the last hour";
	if (minutes < 60 * 48) return `${Math.round(minutes / 60)}h ago`;
	return `${Math.round(minutes / 1440)}d ago`;
}

export default function VocabularyGaps() {
	const [queue, setQueue] = useState<Queue | null>(null);
	const [problem, setProblem] = useState<string | null>(null);
	const [chosen, setChosen] = useState<Record<string, string>>({});
	const [busy, setBusy] = useState<string | null>(null);
	const [done, setDone] = useState<string | null>(null);
	const mounted = useRef(true);

	useEffect(() => () => { mounted.current = false; }, []);

	const load = useCallback(async () => {
		try {
			const response = await msalFetch(`${API_BASE}/api/v1/dlm/vocabulary-gaps`);
			if (!response.ok) {
				throw Object.assign(new Error("read failed"),
					{ reason: reasonFromStatus(response.status) });
			}
			const payload: Queue = await response.json();
			if (!mounted.current) return;
			setQueue(payload);
			setProblem(null);
		} catch (error) {
			if (!mounted.current) return;
			setQueue(null);
			setProblem(unknownText({
				what: "The vocabulary queue",
				says: "which words readers could not have answered",
				reason: (error as { reason?: ReadReason | null }).reason ?? null,
			}));
		}
	}, []);

	useEffect(() => { void load(); }, [load]);

	const resolve = useCallback(async (gap: Gap, target: Target | null) => {
		const key = `${gap.dataset_id}:${gap.term}`;
		setBusy(key);
		setDone(null);
		try {
			const response = await msalFetch(`${API_BASE}/api/v1/dlm/vocabulary-gaps/resolve`, {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({
					dataset_id: gap.dataset_id,
					term: gap.term,
					...(target ? { kind: target.kind, name: target.name } : { dismiss: true }),
				}),
			});
			if (!mounted.current) return;
			setDone(response.ok
				? (target
					? `“${gap.term}” now means ${target.name}.`
					: `“${gap.term}” dismissed.`)
				: `“${gap.term}” could not be saved.`);
			await load();
		} finally {
			if (mounted.current) setBusy(null);
		}
	}, [load]);

	const rows = queue?.gaps ?? [];
	// A plain sum over at most MAX_TERMS rows; memoising it would need `rows`
	// to be stable, and `rows` is a fresh array on every render by design.
	const asks = rows.reduce((total, row) => total + row.count, 0);

	return (
		<section className={s.card}>
			<div className={s.cardHead}>
				<div className={s.cardId}>
					<div className={s.mark} aria-hidden="true">
						<i className="fas fa-language" />
					</div>
					<div>
						<h2 className={s.cardTitle}>Vocabulary queue</h2>
						<p className={s.cardSub}>
							Words readers asked that their dataset could not place, most asked first.
							A dataset derives its own structure; the words your organisation uses for
							it are the one thing it cannot. Map a word once and every later question
							using it is answered.
						</p>
					</div>
				</div>
				<div className={s.actions}>
					<button type="button" className={s.ghost} onClick={() => void load()} disabled={!!busy}>
						Refresh
					</button>
				</div>
			</div>

			<div className={s.facts}>
				<div className={s.fact}>
					<div className={s.factLabel}>Words waiting</div>
					<div className={s.factValue}>{queue ? queue.terms : "—"}</div>
				</div>
				<div className={s.fact}>
					<div className={s.factLabel}>Questions they cost</div>
					<div className={s.factValue}>{queue ? asks : "—"}</div>
				</div>
				<div className={s.fact}>
					<div className={s.factLabel}>Datasets affected</div>
					<div className={s.factValue}>{queue ? queue.datasets : "—"}</div>
				</div>
			</div>

			{problem && <p className={g.problem} role="status">{problem}</p>}
			{queue?.skipped && <p className={g.problem} role="status">{queue.skipped}</p>}
			{done && <p className={g.saved} role="status">{done}</p>}

			{!problem && !queue?.skipped && (
				rows.length ? (
					<div className={g.tableWrap}>
						<table className={g.table}>
							<thead>
								<tr>
									<th>Word</th>
									<th>Dataset</th>
									<th className={g.num}>Asked</th>
									<th>Last</th>
									<th>For example</th>
									<th>Means</th>
									<th />
								</tr>
							</thead>
							<tbody>
								{rows.map((gap) => {
									const key = `${gap.dataset_id}:${gap.term}`;
									const value = chosen[key]
										?? (gap.closest.length
											? `${gap.measures.includes(gap.closest[0]) ? "metric" : "dimension"}:${gap.closest[0]}`
											: "");
									const target = parseTarget(value);
									return (
										<tr key={key}>
											<td className={g.mono}>{gap.term}</td>
											<td>{gap.dataset_name ?? gap.dataset_id}</td>
											<td className={g.num}>
												{gap.count}
												{gap.askers > 1 && (
													<span className={g.detail}> · {gap.askers} people</span>
												)}
											</td>
											<td className={g.detail}>{seenText(gap.last_seen)}</td>
											<td className={g.detail} title={gap.examples.join(" / ")}>
												{gap.examples[0] ?? "—"}
											</td>
											<td>
												<select
													className={g.select}
													value={value}
													aria-label={`What ${gap.term} means`}
													onChange={(event) =>
														setChosen((prior) => ({ ...prior, [key]: event.target.value }))}
												>
													<option value="">Choose…</option>
													{gap.measures.length > 0 && (
														<optgroup label="Measures">
															{gap.measures.map((name) => (
																<option key={`m:${name}`} value={`metric:${name}`}>{name}</option>
															))}
														</optgroup>
													)}
													{gap.dimensions.length > 0 && (
														<optgroup label="Dimensions">
															{gap.dimensions.map((name) => (
																<option key={`d:${name}`} value={`dimension:${name}`}>{name}</option>
															))}
														</optgroup>
													)}
												</select>
											</td>
											<td>
												<div className={g.rowActions}>
													<button
														type="button"
														className={s.ghost}
														disabled={!target || busy === key}
														onClick={() => void resolve(gap, target)}
													>
														{busy === key ? "Saving…" : "Map"}
													</button>
													<button
														type="button"
														className={s.ghost}
														disabled={busy === key}
														onClick={() => void resolve(gap, null)}
														title="This word is noise, not missing vocabulary"
													>
														Dismiss
													</button>
												</div>
											</td>
										</tr>
									);
								})}
							</tbody>
						</table>
					</div>
				) : (
					<div className={g.empty}>
						{queue
							? "Every word readers have used has been placed."
							: "Reading the vocabulary queue…"}
					</div>
				)
			)}

			<div className={s.schedule}>
				<p className={s.note}>
					<b>A mapping is permanent; the queue is not.</b> Mapping a word writes an alias on
					that measure or dimension — the same curation the dataset&apos;s context page
					writes — and dismissing one records that you judged it noise. Both survive a
					restart and a regenerate. The observations themselves are held in the API process,
					so a restart empties this list; the words come back as people ask again. Dismissing
					a word stops it being listed here, not being refused — a question using a word the
					dataset cannot place is still declined rather than answered with the wrong measure.
				</p>
			</div>
		</section>
	);
}
