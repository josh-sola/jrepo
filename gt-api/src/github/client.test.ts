import { afterEach, describe, expect, test } from 'bun:test';
import { readGhToken } from './client.ts';

const original = process.env.GITHUB_TOKEN;

afterEach(() => {
  if (original === undefined) delete process.env.GITHUB_TOKEN;
  else process.env.GITHUB_TOKEN = original;
});

describe('readGhToken', () => {
  test('uses GITHUB_TOKEN when it is set', async () => {
    process.env.GITHUB_TOKEN = 'env-token';

    expect(await readGhToken()).toBe('env-token');
  });
});
