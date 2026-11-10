import { describe, expect, it } from 'vitest';
import { toOutput } from '../src/run.ts';

describe('toOutput', () => {
  it('keeps value and logs for a success', () => {
    expect(
      toOutput({ ok: true, value: 1, logs: ['a'], toolCalls: [] }),
    ).toEqual({ ok: true, value: 1, logs: ['a'] });
  });

  it('drops empty logs and warnings', () => {
    expect(
      toOutput({
        ok: true,
        value: null,
        logs: [],
        warnings: [],
        toolCalls: [],
      }),
    ).toEqual({ ok: true, value: null });
  });

  it('flags truncation and keeps the error for a failure', () => {
    const error = { kind: 'ToolFailure', message: 'no' } as const;
    expect(
      toOutput({ ok: false, error, truncated: true, toolCalls: [] }),
    ).toEqual({ ok: false, error, truncated: true });
  });
});
