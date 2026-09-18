import { afterEach, describe, expect, test } from 'bun:test';
import type { PrSummary } from '../../shared/github.ts';
import {
  isGraphitePlaceholder,
  realBaseFromHistory,
  resetGraphiteBaseMemoForTests,
  resolveStackBase,
} from './graphiteBase.ts';
import type { BaseRefChange } from './source.ts';

afterEach(() => {
  resetGraphiteBaseMemoForTests();
});

function change(
  previousRefName: string,
  currentRefName: string,
  createdAt: string,
): BaseRefChange {
  return { previousRefName, currentRefName, createdAt, actorLogin: null };
}

function makePr(overrides: Partial<PrSummary> & { number: number }): PrSummary {
  return {
    owner: 'Sola-Solutions',
    repo: 'monorepo',
    title: `PR #${overrides.number}`,
    body: '',
    state: 'open',
    draft: false,
    author: { login: 'josh-sola', avatarUrl: null, isBot: false },
    base: { ref: `graphite-base/${overrides.number}`, sha: 'b' },
    head: { ref: `branch-${overrides.number}`, sha: 'h' },
    mergeCommitSha: null,
    additions: 1,
    deletions: 1,
    changedFiles: 1,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    url: `https://github.com/Sola-Solutions/monorepo/pull/${overrides.number}`,
    ...overrides,
  };
}

describe('isGraphitePlaceholder', () => {
  test('matches graphite-base/<n>', () => {
    expect(isGraphitePlaceholder('graphite-base/20673')).toBe(true);
  });

  test('rejects a real branch name and a malformed placeholder', () => {
    expect(isGraphitePlaceholder('josh/dslir-better-text-resolution')).toBe(
      false,
    );
    expect(isGraphitePlaceholder('graphite-base/not-a-number')).toBe(false);
  });
});

describe('realBaseFromHistory', () => {
  test('resolves PR #20673 to its real parent branch', () => {
    // Matches the live BaseRefChangedEvent history from the bug report:
    // Graphite flips the base to the placeholder and back twice, and the
    // real parent is the previous ref on the newest change.
    const changes: BaseRefChange[] = [
      change(
        'josh/dslir-better-text-resolution',
        'graphite-base/20673',
        '2026-09-16T00:00:00Z',
      ),
      change(
        'graphite-base/20673',
        'josh/dslir-better-text-resolution',
        '2026-09-17T00:00:00Z',
      ),
      change(
        'josh/dslir-better-text-resolution',
        'graphite-base/20673',
        '2026-09-18T00:00:00Z',
      ),
    ];

    expect(realBaseFromHistory(changes)).toBe(
      'josh/dslir-better-text-resolution',
    );
  });

  test('returns null when every previous ref in the history is a placeholder', () => {
    const changes: BaseRefChange[] = [
      change('graphite-base/1', 'graphite-base/2', '2026-01-01T00:00:00Z'),
    ];

    expect(realBaseFromHistory(changes)).toBeNull();
  });

  test('returns null for an empty history', () => {
    expect(realBaseFromHistory([])).toBeNull();
  });
});

describe('resolveStackBase', () => {
  test('returns base.ref unchanged when it is not a placeholder', async () => {
    const pr = makePr({ number: 2, base: { ref: 'master', sha: 'b' } });
    const source = { listBaseRefChanges: async () => [] };

    expect(await resolveStackBase(source, pr)).toBe('master');
  });

  test('resolves a placeholder base through history', async () => {
    const pr = makePr({
      number: 3,
      base: { ref: 'graphite-base/3', sha: 'b' },
    });
    const source = {
      listBaseRefChanges: async () => [
        change('branch-below', 'graphite-base/3', '2026-01-01T00:00:00Z'),
      ],
    };

    expect(await resolveStackBase(source, pr)).toBe('branch-below');
  });

  test('memoizes the history lookup so a second call skips listBaseRefChanges', async () => {
    const pr = makePr({
      number: 4,
      base: { ref: 'graphite-base/4', sha: 'b' },
    });
    let calls = 0;
    const source = {
      listBaseRefChanges: async () => {
        calls += 1;
        return [
          change('branch-below', 'graphite-base/4', '2026-01-01T00:00:00Z'),
        ];
      },
    };

    expect(await resolveStackBase(source, pr)).toBe('branch-below');
    expect(await resolveStackBase(source, pr)).toBe('branch-below');
    expect(calls).toBe(1);
  });
});
