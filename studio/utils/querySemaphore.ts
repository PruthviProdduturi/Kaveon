/**
 * Global query semaphore — limits concurrent chart queries on dashboards.
 * Without this, a dashboard with 10 charts fires 10 parallel queries,
 * saturating the backend connection pool and causing timeouts.
 */

// Concurrent dashboard chart queries against the selected source.
// The default AKS resource group admits one dashboard statement at a time;
// each statement already fans out across all three workers.
const MAX_CONCURRENT = 1;
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

// The test Engine admits four 512 MiB query budgets. Keep one slot available
// for filter options or SQL Lab while a rich dashboard loads. This limiter is
// independent of the relational backend's existing six-query limit.
let engineRunning = 0;
const engineQueue: Array<() => void> = [];

export function acquireEngineQuerySlot(): Promise<() => void> {
  return new Promise((resolve) => {
    const start = () => {
      engineRunning++;
      let released = false;
      resolve(() => {
        if (released) return;
        released = true;
        engineRunning--;
        engineQueue.shift()?.();
      });
    };
    if (engineRunning < 3) start();
    else engineQueue.push(start);
  });
}
