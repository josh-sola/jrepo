import { describe, expect, it } from 'bun:test';
import type { DiffChangeSpan, DiffPayload } from '../../shared/diff.ts';
import { refineFileSpans } from './refineSpans.ts';

const encoder = new TextEncoder();
const byteLength = (s: string) => encoder.encode(s).length;

// Difftastic's own worst case for prose: one span tiling the entire line,
// on both sides — the shape that makes a mod row "saturated".
function fullLineSpan(line: string): DiffChangeSpan[] {
  return [{ start: 0, end: byteLength(line) }];
}

function makeFile(partial: Partial<DiffPayload>): DiffPayload {
  return {
    path: 'driver.py',
    oldPath: null,
    status: 'changed',
    language: 'python',
    aligned: [],
    lhsLines: [],
    rhsLines: [],
    lhsSpans: {},
    rhsSpans: {},
    stat: { added: 0, removed: 0, modified: 0 },
    structural: true,
    fallbackReason: null,
    binary: false,
    collapseReason: null,
    ...partial,
  };
}

describe('refineFileSpans', () => {
  it('refines a reflowed-prose run to just the inserted words', () => {
    const lhsLines = [
      'alpha bravo charlie delta echo foxtrot golf hotel india juliet',
      'kilo lima mike november oscar papa quebec romeo sierra tango',
    ];
    // Reflowed at a different word-wrap point than lhs, plus "prepare
    // input now" inserted — rewrap alone must yield no spans, only the
    // inserted phrase.
    const rhsLines = [
      'alpha bravo charlie delta echo foxtrot golf hotel india juliet prepare input',
      'now kilo lima mike november oscar papa quebec romeo sierra tango',
    ];
    const file = makeFile({
      aligned: [
        [0, 0],
        [1, 1],
      ],
      lhsLines,
      rhsLines,
      lhsSpans: {
        '0': fullLineSpan(lhsLines[0]!),
        '1': fullLineSpan(lhsLines[1]!),
      },
      rhsSpans: {
        '0': fullLineSpan(rhsLines[0]!),
        '1': fullLineSpan(rhsLines[1]!),
      },
    });

    const { lhs, rhs } = refineFileSpans(file);

    // No words were deleted, only inserted — the reflow itself carries no
    // signal, so the untouched sentences on the lhs get no spans at all.
    expect(lhs['0']).toEqual([]);
    expect(lhs['1']).toEqual([]);

    const prepareStart = rhsLines[0]!.indexOf('prepare');
    const inputEnd = rhsLines[0]!.indexOf('input') + 'input'.length;
    // "prepare" and "input" are adjacent, separated only by whitespace —
    // merged into one span rather than two.
    expect(rhs['0']).toEqual([{ start: prepareStart, end: inputEnd }]);

    const nowEnd = rhsLines[1]!.indexOf('now') + 'now'.length;
    expect(rhs['1']).toEqual([{ start: 0, end: nowEnd }]);
  });

  it("emits no spans for a run that's a genuine complete rewrite", () => {
    const lhsLine =
      'the quick brown fox jumps over the lazy dog today in the meadow';
    const rhsLine =
      'xray yankee zulu omega sigma tau upsilon phi chi psi rho nu mu';
    const file = makeFile({
      aligned: [[0, 0]],
      lhsLines: [lhsLine],
      rhsLines: [rhsLine],
      lhsSpans: { '0': fullLineSpan(lhsLine) },
      rhsSpans: { '0': fullLineSpan(rhsLine) },
    });

    const { lhs, rhs } = refineFileSpans(file);

    expect(lhs['0']).toEqual([]);
    expect(rhs['0']).toEqual([]);
  });

  it('passes through sparse, non-saturated spans verbatim', () => {
    const lhsSpans: DiffChangeSpan[] = [{ start: 15, end: 16, content: '+' }];
    const rhsSpans: DiffChangeSpan[] = [{ start: 17, end: 18, content: '1' }];
    const file = makeFile({
      aligned: [[0, 0]],
      lhsLines: ['  return a + b;'],
      rhsLines: ['  return a + b + 1;'],
      lhsSpans: { '0': lhsSpans },
      rhsSpans: { '0': rhsSpans },
    });

    const { lhs, rhs } = refineFileSpans(file);

    expect(lhs['0']).toEqual(lhsSpans);
    expect(rhs['0']).toEqual(rhsSpans);
  });

  it("maps an inserted word's span to correct UTF-8 byte offsets across a preceding multibyte char", () => {
    const lhsLine = 'café bar baz';
    const rhsLine = 'café bar baz qux';
    const file = makeFile({
      aligned: [[0, 0]],
      lhsLines: [lhsLine],
      rhsLines: [rhsLine],
      lhsSpans: { '0': fullLineSpan(lhsLine) },
      rhsSpans: { '0': fullLineSpan(rhsLine) },
    });

    const { lhs, rhs } = refineFileSpans(file);

    expect(lhs['0']).toEqual([]);
    // "café" is 4 chars but 5 UTF-8 bytes ('é' is 2 bytes) — "qux" starts
    // right after "café bar baz " (5 + 1 + 3 + 1 + 3 + 1 = 14 bytes) and
    // the line is 17 bytes total.
    expect(rhs['0']).toEqual([{ start: 14, end: 17 }]);
  });

  it("doesn't misattribute an inserted line's words as lhs deletions", () => {
    const lhsLines = [
      'one two three four five six',
      'seven eight nine ten eleven twelve',
    ];
    const rhsLines = [
      'one two three four five six',
      'alpha-inserted beta-inserted gamma-inserted',
      'seven eight nine ten eleven twelve',
    ];
    const file = makeFile({
      aligned: [
        [0, 0],
        [null, 1],
        [1, 2],
      ],
      lhsLines,
      rhsLines,
      lhsSpans: {
        '0': fullLineSpan(lhsLines[0]!),
        '1': fullLineSpan(lhsLines[1]!),
      },
      rhsSpans: {
        '0': fullLineSpan(rhsLines[0]!),
        '1': fullLineSpan(rhsLines[1]!),
        '2': fullLineSpan(rhsLines[2]!),
      },
    });

    const { lhs, rhs } = refineFileSpans(file);

    expect(lhs['0']).toEqual([]);
    expect(lhs['1']).toEqual([]);
    expect(rhs['0']).toEqual([]);
    expect(rhs['2']).toEqual([]);
    expect(rhs['1']).toEqual([{ start: 0, end: rhsLines[1]!.length }]);
  });

  it('does not treat a 50%-coverage row as saturated', () => {
    const lhsLine = 'aaaa bbbb';
    const rhsLine = 'cccc dddd';
    const lhsSpans: DiffChangeSpan[] = [{ start: 0, end: 4 }]; // "aaaa" only: 4 of 8 non-ws bytes
    const rhsSpans: DiffChangeSpan[] = [{ start: 0, end: byteLength(rhsLine) }]; // fully tiled
    const file = makeFile({
      aligned: [[0, 0]],
      lhsLines: [lhsLine],
      rhsLines: [rhsLine],
      lhsSpans: { '0': lhsSpans },
      rhsSpans: { '0': rhsSpans },
    });

    const { lhs, rhs } = refineFileSpans(file);

    // Below-threshold coverage on the lhs alone should keep this row out
    // of any run — both sides pass through untouched.
    expect(lhs['0']).toEqual(lhsSpans);
    expect(rhs['0']).toEqual(rhsSpans);
  });
});
