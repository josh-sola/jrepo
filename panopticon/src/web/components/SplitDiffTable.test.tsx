import '../test-setup.ts';
import { describe, expect, test } from 'bun:test';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { EMPTY_FOLD_STATE } from '../diff/folds.ts';
import type { DiffRow } from '../diff/rows.ts';
import { isCommentTarget, SplitDiffTable } from './SplitDiffTable.tsx';

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const ROWS: DiffRow[] = [
  { l: 0, r: 0, kind: 'unchanged' },
  { l: 1, r: 1, kind: 'mod' },
  { l: 2, r: 2, kind: 'unchanged' },
  { l: null, r: 3, kind: 'add' },
];
const LINES = ['a', 'b', 'c', 'd'];

describe('isCommentTarget', () => {
  test('matches lines inside the range on the target side only', () => {
    const target = { side: 'new' as const, startLine: 3, line: 1 };
    expect(isCommentTarget(target, 'new', 1)).toBe(true);
    expect(isCommentTarget(target, 'new', 2)).toBe(true);
    expect(isCommentTarget(target, 'new', 3)).toBe(true);
    expect(isCommentTarget(target, 'new', 0)).toBe(false);
    expect(isCommentTarget(target, 'old', 2)).toBe(false);
    expect(isCommentTarget(target, 'new', null)).toBe(false);
    expect(isCommentTarget(null, 'new', 2)).toBe(false);
  });
});

describe('SplitDiffTable comment target', () => {
  test('marks the gutter and code cells of the targeted lines', () => {
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    act(() => {
      root.render(
        <SplitDiffTable
          path="a.ts"
          rows={ROWS}
          foldState={EMPTY_FOLD_STATE}
          onRevealFold={() => {}}
          lhsLines={LINES}
          rhsLines={LINES}
          lhsTokens={null}
          rhsTokens={null}
          commentTarget={{ side: 'new', startLine: 1, line: 2 }}
        />,
      );
    });

    const marked = [...container.querySelectorAll('td[data-comment-target]')];
    // Two rows in range, each with a new-side gutter cell and code cell.
    expect(marked).toHaveLength(4);
    const codeCells = marked.filter((td) => td.classList.contains('diff-code'));
    expect(codeCells.map((td) => td.getAttribute('data-line'))).toEqual([
      '1',
      '2',
    ]);
    expect(
      codeCells.every((td) => td.getAttribute('data-side') === 'new'),
    ).toBe(true);

    act(() => root.unmount());
    container.remove();
  });
});
