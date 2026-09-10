/**
 * Global query semaphore — limits concurrent chart queries on dashboards.
 * Without this, a dashboard with 10 charts fires 10 parallel queries,
 * saturating the backend connection pool and causing timeouts.
 */

// The data-warehouse pool has five connections. Keep one available for filter
// options or SQL Lab. Engine requests also pass through the three-slot cap below.
const MAX_CONCURRENT = 4;
type SemaphoreState = { running: number; queue: Array<() => void> };
let state: SemaphoreState = { running: 0, queue: [] };

export function acquireQuerySlot(): Promise<() => void> {
  return new Promise((resolve) => {
    // Capture this generation so releases from a page that was reset cannot
    // decrement or over-admit work on the next page.
    const acquiredState = state;
    const start = () => {
      let released = false;
      resolve(() => {
        if (released) return;
        released = true;
        acquiredState.running = Math.max(0, acquiredState.running - 1);
        const next = acquiredState.queue.shift();
        if (next) {
          acquiredState.running++;
          next();
        }
      });
    };
    if (acquiredState.running < MAX_CONCURRENT) {
      acquiredState.running++;
      start();
    } else {
      acquiredState.queue.push(start);
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
  state.queue.length = 0;
  state = { running: 0, queue: [] };
}

/** True when no chart queries are running or queued (dashboard finished loading). */
export function isQueryIdle(): boolean {
  return state.running === 0 && state.queue.length === 0;
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
