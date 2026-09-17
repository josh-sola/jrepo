import '../../test-setup.ts';
import { describe, expect, it } from 'bun:test';
import {
  characterOffsetInCell,
  identifierWordAt,
  rangeForOffsets,
  resolveHoverTarget,
} from './offset.ts';

// Builds a `.diff-code` cell the way Shiki renders one: every token in its
// own <span>, so a line's text is split across several text nodes.
function buildCell(tokens: string[]): { cell: HTMLElement; nodes: Text[] } {
  const cell = document.createElement('td');
  const nodes: Text[] = [];
  for (const token of tokens) {
    const span = document.createElement('span');
    const text = document.createTextNode(token);
    span.append(text);
    cell.append(span);
    nodes.push(text);
  }
  return { cell, nodes };
}

describe('characterOffsetInCell', () => {
  it('sums the length of every text node before the hit node', () => {
    // "const " + "foo" + " = 1;" — hitting inside "foo" at its own offset 1
    // should land on the whole line's offset 7.
    const { cell, nodes } = buildCell(['const ', 'foo', ' = 1;']);
    expect(characterOffsetInCell(cell, nodes[1]!, 1)).toBe(7);
  });

  it('resolves an offset in the first node without needing to sum anything', () => {
    const { cell, nodes } = buildCell(['hello', ' world']);
    expect(characterOffsetInCell(cell, nodes[0]!, 3)).toBe(3);
  });

  it('returns null for a node that is not inside the cell', () => {
    const { cell } = buildCell(['abc']);
    const stray = document.createTextNode('xyz');
    expect(characterOffsetInCell(cell, stray, 0)).toBeNull();
  });
});

describe('identifierWordAt', () => {
  const text = '  const foo = bar.baz;';

  it('expands to the full identifier from an offset in its middle', () => {
    // "foo" spans [8, 11).
    expect(identifierWordAt(text, 9)).toEqual({ start: 8, end: 11 });
  });

  it('expands correctly from the identifier’s first character', () => {
    expect(identifierWordAt(text, 8)).toEqual({ start: 8, end: 11 });
  });

  it('expands correctly from the identifier’s last character', () => {
    expect(identifierWordAt(text, 10)).toEqual({ start: 8, end: 11 });
  });

  it('returns null for whitespace', () => {
    expect(identifierWordAt(text, 0)).toBeNull();
  });

  it('returns null for punctuation', () => {
    // "." right after "bar" at index 17.
    expect(identifierWordAt(text, 17)).toBeNull();
  });

  it('returns null past the end of the line', () => {
    expect(identifierWordAt(text, text.length)).toBeNull();
  });
});

describe('resolveHoverTarget', () => {
  it('combines the offset and identifier lookups', () => {
    const { cell, nodes } = buildCell(['const ', 'foo', ' = 1;']);
    expect(resolveHoverTarget(cell, nodes[1]!, 1)).toEqual({
      character: 7,
      word: { start: 6, end: 9 },
    });
  });

  it('returns null when the hit character is not part of an identifier', () => {
    const { cell, nodes } = buildCell(['const ', 'foo', ' = 1;']);
    // Offset 0 of " = 1;" is the space right after "foo".
    expect(resolveHoverTarget(cell, nodes[2]!, 0)).toBeNull();
  });
});

describe('rangeForOffsets', () => {
  it('builds a Range spanning the requested offsets across token spans', () => {
    const { cell } = buildCell(['const ', 'foo', ' = 1;']);
    const range = rangeForOffsets(cell, 6, 9);
    expect(range).not.toBeNull();
    expect(range?.toString()).toBe('foo');
  });

  it('returns null when the cell has no text', () => {
    const cell = document.createElement('td');
    expect(rangeForOffsets(cell, 0, 1)).toBeNull();
  });
});
