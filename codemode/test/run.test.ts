import { describe, expect, it } from 'vitest';
import { runProgram, toOutput } from '../src/run.ts';

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

describe('runProgram', () => {
  it('runs a program with type annotations', async () => {
    expect(await runProgram({}, 'const x: number = 1; return x')).toEqual({
      ok: true,
      value: 1,
    });
  });

  it('reports a runtime error at its line in an annotated program', async () => {
    const code = [
      'type Box = { value: number };',
      'const box: Box | null = null as Box | null;',
      'return (box as Box).value;',
    ].join('\n');
    expect(await runProgram({}, code)).toMatchObject({
      ok: false,
      error: { kind: 'ExecutionFailure', location: { line: 3 } },
    });
  });

  it('reports an enum as a parse error', async () => {
    expect(await runProgram({}, 'enum E { A }\nreturn E.A')).toMatchObject({
      ok: false,
      error: { kind: 'ParseError', location: { line: 1, column: 1 } },
    });
  });
});
