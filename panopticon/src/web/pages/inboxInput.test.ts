import { describe, expect, test } from 'bun:test';
import { parsePrInput } from './inboxInput.ts';

const DEFAULT = { owner: 'acme', repo: 'widgets' };

describe('parsePrInput', () => {
  test('a bare number resolves against the default owner/repo', () => {
    expect(parsePrInput('1234', DEFAULT)).toEqual({
      owner: 'acme',
      repo: 'widgets',
      number: 1234,
    });
  });

  test('a bare number with a leading # also resolves', () => {
    expect(parsePrInput('#1234', DEFAULT)).toEqual({
      owner: 'acme',
      repo: 'widgets',
      number: 1234,
    });
  });

  test('a bare number with no default owner/repo is rejected', () => {
    expect(parsePrInput('1234', null)).toBeNull();
  });

  test('owner/repo#number parses on its own, ignoring any default', () => {
    expect(parsePrInput('Sola-Solutions/monorepo#20816', null)).toEqual({
      owner: 'Sola-Solutions',
      repo: 'monorepo',
      number: 20816,
    });
  });

  test('a GitHub PR URL parses on its own', () => {
    expect(
      parsePrInput(
        'https://github.com/Sola-Solutions/monorepo/pull/20816',
        null,
      ),
    ).toEqual({ owner: 'Sola-Solutions', repo: 'monorepo', number: 20816 });
  });

  test('a GitHub PR URL with trailing path segments still parses', () => {
    expect(
      parsePrInput('https://github.com/acme/widgets/pull/42/files', null),
    ).toEqual({ owner: 'acme', repo: 'widgets', number: 42 });
  });

  test('surrounding whitespace is trimmed', () => {
    expect(parsePrInput('  1234  ', DEFAULT)).toEqual({
      owner: 'acme',
      repo: 'widgets',
      number: 1234,
    });
  });

  test('an empty string is rejected', () => {
    expect(parsePrInput('   ', DEFAULT)).toBeNull();
  });

  test('garbage input is rejected', () => {
    expect(parsePrInput('not a pr', DEFAULT)).toBeNull();
  });

  test('a URL from another host is rejected', () => {
    expect(
      parsePrInput('https://gitlab.com/acme/widgets/pull/42', DEFAULT),
    ).toBeNull();
  });
});
