import '../test-setup.ts';
import { afterEach, describe, expect, it } from 'bun:test';
import { captureSelectionAnchor, climbToCode } from './anchor.ts';

function cell(
  file: string,
  side: 'old' | 'new',
  line: number,
  text: string,
): HTMLTableCellElement {
  const td = document.createElement('td');
  td.className = 'diff-code';
  td.dataset.file = file;
  td.dataset.side = side;
  td.dataset.line = String(line);
  td.textContent = text;
  return td;
}

function selectAcross(
  startNode: Node,
  startOffset: number,
  endNode: Node,
  endOffset: number,
) {
  const range = document.createRange();
  range.setStart(startNode, startOffset);
  range.setEnd(endNode, endOffset);
  const sel = window.getSelection()!;
  sel.removeAllRanges();
  sel.addRange(range);
  return sel;
}

afterEach(() => window.getSelection()?.removeAllRanges());

describe('climbToCode', () => {
  it('climbs from a text node to its enclosing .diff-code cell', () => {
    const td = cell('a.py', 'new', 4, "print('hi')");
    document.body.append(td);
    expect(climbToCode(td.firstChild)).toBe(td);
  });

  it("returns null when there's no enclosing .diff-code cell", () => {
    const div = document.createElement('div');
    div.textContent = 'not code';
    document.body.append(div);
    expect(climbToCode(div.firstChild)).toBeNull();
  });
});

describe('captureSelectionAnchor', () => {
  it("builds a 1-indexed anchor from a same-cell selection, side 'new' -> new", () => {
    const td = cell('a.py', 'new', 4, "print('hi')");
    document.body.append(td);
    const sel = selectAcross(td.firstChild!, 0, td.firstChild!, 5);

    expect(captureSelectionAnchor(sel)).toEqual({
      file: 'a.py',
      side: 'new',
      start_line: 5,
      end_line: 5,
      quote: 'print',
    });
  });

  it("maps side 'old' data-side to the old anchor side", () => {
    const td = cell('a.py', 'old', 0, 'old text');
    document.body.append(td);
    const sel = selectAcross(td.firstChild!, 0, td.firstChild!, 3);

    expect(captureSelectionAnchor(sel)?.side).toBe('old');
    expect(captureSelectionAnchor(sel)?.start_line).toBe(1);
  });

  it('spans multiple lines within the same file, start=min end=max', () => {
    const a = cell('a.py', 'new', 2, 'line three');
    const b = cell('a.py', 'new', 5, 'line six');
    document.body.append(a, b);
    const sel = selectAcross(a.firstChild!, 2, b.firstChild!, 4);

    expect(captureSelectionAnchor(sel)).toMatchObject({
      start_line: 3,
      end_line: 6,
    });
  });

  it('rejects a selection spanning two different files', () => {
    const a = cell('a.py', 'new', 0, 'in a');
    const b = cell('b.py', 'new', 0, 'in b');
    document.body.append(a, b);
    const sel = selectAcross(a.firstChild!, 0, b.firstChild!, 2);

    expect(captureSelectionAnchor(sel)).toBeNull();
  });

  it('rejects a collapsed or whitespace-only selection', () => {
    const td = cell('a.py', 'new', 0, '   ');
    document.body.append(td);
    const collapsed = document.createRange();
    collapsed.setStart(td.firstChild!, 0);
    collapsed.collapse(true);
    const sel = window.getSelection()!;
    sel.removeAllRanges();
    sel.addRange(collapsed);
    expect(captureSelectionAnchor(sel)).toBeNull();

    const whitespace = selectAcross(td.firstChild!, 0, td.firstChild!, 3);
    expect(captureSelectionAnchor(whitespace)).toBeNull();
  });

  it("returns null when the selection isn't inside a diff-code cell", () => {
    const div = document.createElement('div');
    div.textContent = 'outside';
    document.body.append(div);
    const sel = selectAcross(div.firstChild!, 0, div.firstChild!, 3);
    expect(captureSelectionAnchor(sel)).toBeNull();
  });
});
