import assert from 'node:assert/strict';
import test from 'node:test';
import { acquireEngineQuerySlot } from '../utils/querySemaphore.ts';

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
