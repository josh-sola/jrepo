import { describe, expect, test } from 'bun:test';
import type { CollapseRule } from '../config.ts';
import { classifyCollapse } from './collapse.ts';

const RULES: CollapseRule[] = [
  { glob: '**/*.test.ts', reason: 'test' },
  { glob: '**/pnpm-lock.yaml', reason: 'lockfile' },
  { glob: '**/__snapshots__/**', reason: 'snapshot' },
];

describe('classifyCollapse', () => {
  test('generated wins over every rule', () => {
    const reason = classifyCollapse({
      path: 'src/foo.test.ts',
      added: 0,
      removed: 0,
      generated: true,
      rules: RULES,
    });
    expect(reason).toBe('generated');
  });

  test('matches a test glob', () => {
    const reason = classifyCollapse({
      path: 'src/foo.test.ts',
      added: 5,
      removed: 1,
      generated: false,
      rules: RULES,
    });
    expect(reason).toBe('test');
  });

  test('matches a lockfile glob', () => {
    const reason = classifyCollapse({
      path: 'pnpm-lock.yaml',
      added: 5,
      removed: 1,
      generated: false,
      rules: RULES,
    });
    expect(reason).toBe('lockfile');
  });

  test('matches a snapshot glob', () => {
    const reason = classifyCollapse({
      path: 'src/__snapshots__/foo.snap',
      added: 5,
      removed: 1,
      generated: false,
      rules: RULES,
    });
    expect(reason).toBe('snapshot');
  });

  test('falls back to large when the change is huge and nothing else matches', () => {
    const reason = classifyCollapse({
      path: 'src/big.ts',
      added: 1500,
      removed: 600,
      generated: false,
      rules: RULES,
    });
    expect(reason).toBe('large');
  });

  test('returns null when nothing matches and the change is small', () => {
    const reason = classifyCollapse({
      path: 'src/small.ts',
      added: 3,
      removed: 1,
      generated: false,
      rules: RULES,
    });
    expect(reason).toBeNull();
  });

  test('large is not triggered at exactly the threshold', () => {
    const reason = classifyCollapse({
      path: 'src/big.ts',
      added: 1000,
      removed: 1000,
      generated: false,
      rules: RULES,
    });
    expect(reason).toBeNull();
  });
});
