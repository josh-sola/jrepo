import { describe, expect, it } from 'vitest';
import { notice, parseState, shouldCheck } from '../src/update-check.ts';

const now = new Date('2026-06-10T12:00:00.000Z');
const ago = (hours: number) =>
  new Date(now.getTime() - hours * 3_600_000).toISOString();

describe('shouldCheck', () => {
  it('checks when it never has', () => {
    expect(shouldCheck({}, now)).toBe(true);
  });

  it('waits until the last check is more than 24 hours old', () => {
    expect(shouldCheck({ checkedAt: ago(23) }, now)).toBe(false);
    expect(shouldCheck({ checkedAt: ago(24) }, now)).toBe(false);
    expect(shouldCheck({ checkedAt: ago(24.01) }, now)).toBe(true);
  });

  it('does not check when the last check is in the future', () => {
    expect(shouldCheck({ checkedAt: ago(-5) }, now)).toBe(false);
  });
});

describe('notice', () => {
  const text =
    'cm 0.2.3 is out (you have 0.2.2). Tell the user they can update with: cm upgrade';

  it('names the newer version and the command to run', () => {
    expect(notice({ latest: '0.2.3' }, '0.2.2', now)).toBe(text);
  });

  it('is silent when latest is equal or older', () => {
    expect(notice({ latest: '0.2.2' }, '0.2.2', now)).toBeUndefined();
    expect(notice({ latest: '0.2.1' }, '0.2.2', now)).toBeUndefined();
  });

  it('is silent with no latest', () => {
    expect(notice({ checkedAt: ago(1) }, '0.2.2', now)).toBeUndefined();
  });

  it('prints at most once in 24 hours', () => {
    const latest = '0.2.3';
    expect(
      notice({ latest, notifiedAt: ago(1) }, '0.2.2', now),
    ).toBeUndefined();
    expect(
      notice({ latest, notifiedAt: ago(24) }, '0.2.2', now),
    ).toBeUndefined();
    expect(notice({ latest, notifiedAt: ago(24.01) }, '0.2.2', now)).toBe(text);
  });
});

describe('parseState', () => {
  it('keeps valid fields', () => {
    const state = {
      checkedAt: ago(1),
      latest: '0.2.3',
      notifiedAt: ago(2),
    };
    expect(parseState(state)).toEqual(state);
  });

  it('treats anything that is not an object as never checked', () => {
    for (const value of [null, 'x', 4, [], undefined]) {
      expect(parseState(value)).toEqual({});
    }
  });

  it('drops fields of the wrong type or format', () => {
    expect(
      parseState({
        checkedAt: 5,
        latest: 'latest',
        notifiedAt: 'yesterday',
        extra: true,
      }),
    ).toEqual({});
    expect(parseState({ latest: '0.2.3', checkedAt: {} })).toEqual({
      latest: '0.2.3',
    });
  });
});
