import { describe, expect, it } from 'vitest';
import { stripTypes } from '../src/strip.ts';

describe('stripTypes', () => {
  it('blanks type syntax and keeps every column in place', () => {
    expect(stripTypes('const x: number = 1;\nreturn x as number')).toEqual({
      code: 'const x         = 1;\nreturn x          ',
    });
  });

  it('returns plain JavaScript unchanged', () => {
    const code =
      'const a = await tools.fs.read({ path: "x" });\nreturn a; // done\n';
    expect(stripTypes(code)).toEqual({ code });
  });

  it('leaves code it cannot parse for the interpreter to report', () => {
    expect(stripTypes('const x = ;')).toEqual({ code: 'const x = ;' });
  });

  it('reports an enum as a parse error at its own line', () => {
    expect(stripTypes('const a = 1;\n  enum E { A }\nreturn E.A')).toEqual({
      error: {
        kind: 'ParseError',
        message:
          'TypeScript enum is not supported in strip-only mode (line 2, col 3)',
        location: { line: 2, column: 3 },
      },
    });
  });

  it('reports a namespace as a parse error', () => {
    const result = stripTypes('namespace N { export const a = 1 }\nreturn 1');
    expect(result).toMatchObject({
      error: { kind: 'ParseError', location: { line: 1, column: 1 } },
    });
  });
});
