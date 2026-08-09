/**
 * Global query semaphore — limits concurrent chart queries on dashboards.
 * Without this, a dashboard with 10 charts fires 10 parallel queries,
 * saturating the backend connection pool and causing timeouts.
 */

const MAX_CONCURRENT = 3;
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
