import { describe, expect, test } from 'bun:test';
import { RepoStores } from './stores.ts';

describe('RepoStores', () => {
  test('returns the same RepoStore instance for the same owner and repo', () => {
    const stores = new RepoStores('/tmp/panopticon-data', {});
    const first = stores.for('Sola-Solutions', 'monorepo');
    const second = stores.for('Sola-Solutions', 'monorepo');
    expect(second).toBe(first);
  });

  test('keys distinct repos separately', () => {
    const stores = new RepoStores('/tmp/panopticon-data', {});
    const monorepo = stores.for('Sola-Solutions', 'monorepo');
    const other = stores.for('Sola-Solutions', 'other');
    expect(monorepo).not.toBe(other);
  });
});
