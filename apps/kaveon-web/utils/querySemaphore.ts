/**
 * Global query semaphore — limits concurrent chart queries on dashboards.
 * Without this, a dashboard with 10 charts fires 10 parallel queries,
 * saturating the backend connection pool and causing timeouts.
 */

const MAX_CONCURRENT = 3;
let running = 0;
const queue: Array<() => void> = [];

function release() {
  running--;
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
