import '../test-setup.ts';
import { describe, expect, it } from 'bun:test';
import { byteSpansToCharSpans, rangesForSpans } from './changeSpans.ts';

describe('byteSpansToCharSpans', () => {
  it('passes ASCII-only offsets through unchanged (byte offset == char offset)', () => {
    const line = '  return a + b + 1;';
    // Real `difft --display json` output for this exact line (see
    // types.ts's DiffChangeSpan for how this was captured): the "+ 1"
    // addition is two spans, "+" then "1".
    const spans = [
      { start: 15, end: 16, content: '+' },
      { start: 17, end: 18, content: '1' },
    ];
    expect(byteSpansToCharSpans(line, spans)).toEqual([
      { start: 15, end: 16 },
      { start: 17, end: 18 },
    ]);
  });

  it('translates UTF-8 byte offsets to UTF-16 char offsets across a 2-byte character', () => {
    // Empirically captured from `DFT_UNSTABLE=yes difft --display json` on
    // `const s = "héllo wörld";` vs. `const s = "héllo 世界";` — difftastic's
    // start/end are byte offsets into the line's UTF-8 encoding, and 'é'/'ö'
    // are 2 bytes each in UTF-8 but 1 UTF-16 code unit in the JS string, so
    // every span after the first multi-byte char drifts if treated as a
    // plain char offset.
    const line = 'const s = "héllo wörld";';
    const spans = [
      { start: 10, end: 11, content: '"' },
      { start: 11, end: 17, content: 'héllo' },
      { start: 17, end: 18, content: ' ' },
      { start: 18, end: 24, content: 'wörld' },
      { start: 24, end: 25, content: '"' },
    ];
    const charSpans = byteSpansToCharSpans(line, spans);
    expect(charSpans).toEqual([
      { start: 10, end: 11 },
      { start: 11, end: 16 },
      { start: 16, end: 17 },
      { start: 17, end: 22 },
      { start: 22, end: 23 },
    ]);
    for (let i = 0; i < spans.length; i++) {
      expect(line.slice(charSpans[i]!.start, charSpans[i]!.end)).toBe(
        spans[i]!.content,
      );
    }
  });

  it('translates byte offsets across a 4-byte character (surrogate pair in JS)', () => {
    // Captured the same way from `const s = "a🎉b";` vs. `const s = "a🎉c";`
    // — 🎉 is 4 bytes in UTF-8 but a 2-code-unit surrogate pair in the JS
    // string.
    const line = 'const s = "a🎉b";';
    const spans = [
      { start: 11, end: 12, content: 'a' },
      { start: 12, end: 16, content: '🎉' },
      { start: 16, end: 17, content: 'b' },
    ];
    const charSpans = byteSpansToCharSpans(line, spans);
    expect(charSpans).toEqual([
      { start: 11, end: 12 },
      { start: 12, end: 14 },
      { start: 14, end: 15 },
    ]);
    expect(line.slice(charSpans[1]!.start, charSpans[1]!.end)).toBe('🎉');
  });

  it('clamps a negative or past-end-of-line byte offset instead of throwing', () => {
    const line = 'abc';
    expect(byteSpansToCharSpans(line, [{ start: -5, end: 100 }])).toEqual([
      { start: 0, end: 3 },
    ]);
  });

  it('returns an empty array for no spans', () => {
    expect(byteSpansToCharSpans('abc', [])).toEqual([]);
  });
});

describe('rangesForSpans', () => {
  function cell(html: string): HTMLElement {
    const el = document.createElement('td');
    el.innerHTML = html;
    document.body.append(el);
    return el;
  }

  it('builds a Range for a span entirely inside one text node', () => {
    const root = cell('hello world');
    const [range] = rangesForSpans(root, [{ start: 0, end: 5 }]);
    expect(range!.toString()).toBe('hello');
  });

  it('builds a Range that crosses a nested syntax span boundary', () => {
    const root = cell(
      '<span class="shiki-keyword">function</span> add(a, b) {',
    );
    // "tion add" straddles the boundary between the highlighted keyword span
    // and the plain trailing text node.
    const range = rangesForSpans(root, [{ start: 4, end: 12 }])[0]!;
    expect(range.toString()).toBe('tion add');
  });

  it('returns one Range per span, in order, skipping any that collapse', () => {
    const root = cell('function add(a, b) {');
    const ranges = rangesForSpans(root, [
      { start: 0, end: 8 },
      { start: 9, end: 12 },
    ]);
    expect(ranges.map((r) => r.toString())).toEqual(['function', 'add']);
  });

  it("clamps an end offset past the cell's textContent length", () => {
    const root = cell('abc');
    const [range] = rangesForSpans(root, [{ start: 1, end: 100 }]);
    expect(range!.toString()).toBe('bc');
  });

  it("drops a span that's entirely past the cell's textContent length", () => {
    const root = cell('abc');
    expect(rangesForSpans(root, [{ start: 10, end: 20 }])).toEqual([]);
  });

  it('returns no ranges for an empty cell or no spans', () => {
    expect(rangesForSpans(cell(''), [{ start: 0, end: 1 }])).toEqual([]);
    expect(rangesForSpans(cell('abc'), [])).toEqual([]);
  });
});
