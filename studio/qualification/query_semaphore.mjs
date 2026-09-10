import assert from 'node:assert/strict';
import test from 'node:test';
import { acquireEngineQuerySlot, acquireQuerySlot, resetQuerySemaphore } from '../utils/querySemaphore.ts';

test('dashboard work executes in bounded parallel batches', async () => {
  resetQuerySemaphore();
  let active = 0;
  let peak = 0;
  const tasks = Array.from({ length: 18 }, async () => {
    const release = await acquireQuerySlot();
    active++;
    peak = Math.max(peak, active);
    await new Promise(resolve => setTimeout(resolve, 2));
    active--;
    release();
    release();
  });
  await Promise.all(tasks);
  assert.equal(peak, 4);
  assert.equal(active, 0);
});

test('reset isolates releases from an earlier page generation', async () => {
  resetQuerySemaphore();
  const oldReleases = await Promise.all(Array.from({ length: 4 }, () => acquireQuerySlot()));
  resetQuerySemaphore();
  const newReleases = await Promise.all(Array.from({ length: 4 }, () => acquireQuerySlot()));
  oldReleases.forEach(release => release());
  let seventhStarted = false;
  const seventh = acquireQuerySlot().then(release => { seventhStarted = true; return release; });
  await new Promise(resolve => setTimeout(resolve, 2));
  assert.equal(seventhStarted, false);
  newReleases[0]();
  (await seventh)();
  newReleases.slice(1).forEach(release => release());
});

test('Engine dashboard work queues at three slots and releases after failure', async () => {
  let active = 0;
  let peak = 0;
  let completed = 0;
  const tasks = Array.from({ length: 30 }, async (_, index) => {
    const release = await acquireEngineQuerySlot();
    active++;
    peak = Math.max(peak, active);
    try {
      await new Promise(resolve => setTimeout(resolve, 2));
      if (index === 4) throw new Error('simulated query failure');
    } catch (error) {
      assert.equal(error.message, 'simulated query failure');
    } finally {
      active--;
      release();
      release(); // Repeated cleanup must not over-admit queued work.
      completed++;
    }
  });
  await Promise.all(tasks);
  assert.equal(peak, 3);
  assert.equal(active, 0);
  assert.equal(completed, 30);
});
