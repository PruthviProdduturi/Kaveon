"use client";

import { useState } from "react";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import s from "../settings.module.css";

interface Job { running: boolean; done: number; total: number; label: string }

export default function MaintenancePage() {
  const [job, setJob] = useState<Job | null>(null);

  // Re-captures every dashboard thumbnail in both themes by rendering each
  // dashboard in a hidden frame; the view posts "kaveon-thumb-done" when saved.
  const refreshThumbnails = async () => {
    if (job?.running) return;
    setJob({ running: true, done: 0, total: 0, label: "Loading dashboards…" });

    let dashboards: { id: string; name: string }[] = [];
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/dashboards`);
      const data = await res.json();
      const arr = Array.isArray(data) ? data : data.result || data.items || [];
      dashboards = arr.map((d: { id: string | number; name?: string }) => ({ id: String(d.id), name: d.name || "Untitled" }));
    } catch {
      setJob({ running: false, done: 0, total: 0, label: "Dashboards could not be loaded." });
      setTimeout(() => setJob(null), 5000);
      return;
    }

    const jobs = dashboards.flatMap(d => (["light", "dark"] as const).map(theme => ({ ...d, theme })));
    if (!jobs.length) {
      setJob({ running: false, done: 0, total: 0, label: "There are no dashboards to refresh." });
      setTimeout(() => setJob(null), 5000);
      return;
    }

    const frame = document.createElement("iframe");
    frame.style.cssText = "position:fixed;left:-9999px;top:0;width:1280px;height:800px;border:0;visibility:hidden;";
    document.body.appendChild(frame);
    let resolveDone: (() => void) | null = null;
    const onMessage = (e: MessageEvent) => { if (e.data?.type === "kaveon-thumb-done") resolveDone?.(); };
    window.addEventListener("message", onMessage);
    try {
      for (let i = 0; i < jobs.length; i++) {
        const item = jobs[i];
        setJob({ running: true, done: i, total: jobs.length, label: `${item.name} · ${item.theme}` });
        await new Promise<void>(resolve => {
          const finish = () => { clearTimeout(timer); resolveDone = null; resolve(); };
          const timer = setTimeout(finish, 25000);
          resolveDone = finish;
          frame.src = `/dashboards/${item.id}/view?capture=1&forceTheme=${item.theme}`;
        });
      }
      setJob({ running: false, done: jobs.length, total: jobs.length, label: `Refreshed ${jobs.length} thumbnails.` });
    } finally {
      window.removeEventListener("message", onMessage);
      frame.remove();
      setTimeout(() => setJob(null), 6000);
    }
  };

  return (
    <div className={s.stack}>
      <section className={s.card}>
        <div className={s.cardHead}>
          <div className={s.cardId}>
            <div className={s.mark}><i className="fas fa-images" /></div>
            <div>
              <h2 className={s.cardTitle}>Dashboard thumbnails</h2>
              <p className={s.cardSub}>Re-capture every dashboard preview in light and dark. Runs in this browser tab; leave it open until it finishes.</p>
            </div>
          </div>
          <button type="button" className={`${s.ghost} ${s.primary}`} onClick={refreshThumbnails} disabled={!!job?.running}>
            {job?.running ? "Refreshing…" : "Refresh all thumbnails"}
          </button>
        </div>
        {job && (
          <div style={{ marginTop: 12 }} role="status" aria-live="polite">
            <p className={s.note}>{job.label}{job.running && job.total ? ` · ${job.done} of ${job.total}` : ""}</p>
            {job.running && job.total > 0 && (
              <div className={s.progress}><span className={s.progressFill} style={{ width: `${Math.round((job.done / job.total) * 100)}%` }} /></div>
            )}
          </div>
        )}
      </section>
    </div>
  );
}
