import { describe, expect, test } from 'bun:test';
import { Semaphore } from './semaphore.ts';

describe('Semaphore', () => {
  test('never runs more than max tasks at once', async () => {
    const semaphore = new Semaphore(2);
    let active = 0;
    let maxActive = 0;

    const task = () =>
      semaphore.run(async () => {
        active += 1;
        maxActive = Math.max(maxActive, active);
        await new Promise((resolve) => setTimeout(resolve, 5));
        active -= 1;
      });

    await Promise.all([task(), task(), task(), task(), task()]);
    expect(maxActive).toBeLessThanOrEqual(2);
  });

  test("preserves each task's own result", async () => {
    const semaphore = new Semaphore(1);
    const results = await Promise.all(
      [1, 2, 3].map((n) => semaphore.run(() => Promise.resolve(n * 10))),
    );
    expect(results).toEqual([10, 20, 30]);
  });

  test('releases the slot even when a task throws', async () => {
    const semaphore = new Semaphore(1);
    await expect(
      semaphore.run(() => Promise.reject(new Error('boom'))),
    ).rejects.toThrow('boom');
    // If the slot weren't released, this would hang.
    const result = await semaphore.run(() => Promise.resolve('ok'));
    expect(result).toBe('ok');
  });
});
