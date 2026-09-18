import { describe, expect, test } from 'bun:test';
import type { StackEntry } from '../../../shared/stack.ts';
import { layoutStackGraph } from './graph.ts';

function entry(
  overrides: Partial<StackEntry> & { number: number },
): StackEntry {
  return {
    title: `pr ${overrides.number}`,
    state: 'open',
    draft: false,
    headRef: `head-${overrides.number}`,
    baseRef: 'base',
    isCurrent: false,
    parent: null,
    additions: 0,
    deletions: 0,
    ...overrides,
  };
}

describe('layoutStackGraph', () => {
  test('a linear chain sits in a single lane, newest first', () => {
    const entries = [
      entry({ number: 1, parent: null }),
      entry({ number: 2, parent: 1 }),
      entry({ number: 3, parent: 2 }),
    ];

    const rows = layoutStackGraph(entries);

    expect(rows.map((row) => row.entry.number)).toEqual([3, 2, 1]);
    expect(rows.every((row) => row.lane === 0)).toBe(true);

    const [row3, row2, row1] = rows;
    expect(row3?.hasChildInLane).toBe(false);
    expect(row3?.hasParentBelow).toBe(true);
    expect(row2?.hasChildInLane).toBe(true);
    expect(row2?.hasParentBelow).toBe(true);
    expect(row1?.hasChildInLane).toBe(true);
    expect(row1?.hasParentBelow).toBe(false);
  });

  test('a fork gives the second child its own lane', () => {
    // #1 (root) <- #2 (current) <- { #3 (draft) <- #5, #4 (draft) }
    const entries = [
      entry({ number: 1, parent: null, state: 'merged' }),
      entry({ number: 2, parent: 1, isCurrent: true }),
      entry({ number: 3, parent: 2, draft: true }),
      entry({ number: 5, parent: 3 }),
      entry({ number: 4, parent: 2, draft: true }),
    ];

    const rows = layoutStackGraph(entries);
    const byNumber = new Map(rows.map((row) => [row.entry.number, row]));

    expect(rows.map((row) => row.entry.number)).toEqual([4, 5, 3, 2, 1]);
    expect(byNumber.get(4)?.lane).toBe(1);
    expect(byNumber.get(5)?.lane).toBe(0);
    expect(byNumber.get(3)?.lane).toBe(0);
    expect(byNumber.get(2)?.lane).toBe(0);
    expect(byNumber.get(1)?.lane).toBe(0);

    expect(byNumber.get(2)?.mergeFrom).toEqual([1]);
    expect(byNumber.get(5)?.through).toEqual([1]);
    expect(byNumber.get(3)?.through).toEqual([1]);
    expect(byNumber.get(4)?.hasParentBelow).toBe(true);
    expect(byNumber.get(4)?.hasChildInLane).toBe(false);
    expect(byNumber.get(2)?.hasChildInLane).toBe(true);
  });

  test('returns nothing for an empty stack', () => {
    expect(layoutStackGraph([])).toEqual([]);
  });
});
