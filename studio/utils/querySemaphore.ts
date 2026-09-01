/**
 * Global query semaphore — limits concurrent chart queries on dashboards.
 * Without this, a dashboard with 10 charts fires 10 parallel queries,
 * saturating the backend connection pool and causing timeouts.
 */

// Concurrent dashboard chart queries. Azure Postgres allows ~50 connections and
// the API data-warehouse pool is 10, so 6 loads dashboards ~2x faster than the
// old Fabric/Neon-era limit of 3 while staying well within the pool.
const MAX_CONCURRENT = 6;
let running = 0;
const queue: Array<() => void> = [];

function release() {
  running = Math.max(0, running - 1);
  if (queue.length > 0) {
    const next = queue.shift()!;
    running++;
    next();
  }
}

export function acquireQuerySlot(): Promise<() => void> {
  return new Promise((resolve) => {
    const start = () => resolve(release);
    if (running < MAX_CONCURRENT) {
      running++;
      start();
    } else {
      queue.push(start);
    }
  });
}

/**
 * Drop all *queued* (not-yet-started) query slots and reset the counter, so a
 * page navigation (e.g. dashboard → edit) doesn't wait behind the previous
 * page's queued dashboard-chart queries. In-flight fetches finish on their own
 * (guarded release), but the next page gets fresh slots immediately.
 */
export function resetQuerySemaphore(): void {
  queue.length = 0;
  running = 0;
}

/** True when no chart queries are running or queued (dashboard finished loading). */
export function isQueryIdle(): boolean {
  return running === 0 && queue.length === 0;
}
