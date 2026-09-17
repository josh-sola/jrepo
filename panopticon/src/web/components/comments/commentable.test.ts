import { describe, expect, it } from 'bun:test';
import {
  computeCommentableLines,
  firstCommentableLine,
  isCommentable,
  toApiSide,
  toUiSide,
} from './commentable.ts';
import type { DiffPayload } from '../../../shared/diff.ts';

function makeFile(overrides: Partial<DiffPayload>): DiffPayload {
  return {
    path: 'synthetic.txt',
    oldPath: null,
    status: 'changed',
    language: 'Text',
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
    ...overrides,
  };
}

describe('toUiSide / toApiSide', () => {
  it('round-trips RIGHT/new and LEFT/old', () => {
    expect(toUiSide('RIGHT')).toBe('new');
    expect(toUiSide('LEFT')).toBe('old');
    expect(toApiSide('new')).toBe('RIGHT');
    expect(toApiSide('old')).toBe('LEFT');
  });
});

describe('computeCommentableLines', () => {
  it('marks a mod pair itself commentable on both sides', () => {
    const file = makeFile({
      aligned: [[0, 0]],
      rhsSpans: { '0': [{ start: 0, end: 3 }] },
    });
    const lines = computeCommentableLines(file);
    expect(isCommentable(lines, 'old', 0)).toBe(true);
    expect(isCommentable(lines, 'new', 0)).toBe(true);
  });

  it('marks lines within three rows of a change commentable, and lines past that not', () => {
    // 9 unchanged pairs (indices 0-8), with the only change at index 4.
    const aligned: [number, number][] = Array.from({ length: 9 }, (_, i) => [
      i,
      i,
    ]);
    const file = makeFile({
      aligned,
      rhsSpans: { '4': [{ start: 0, end: 1 }] },
    });
    const lines = computeCommentableLines(file);

    // Index 1 (distance 3) is commentable; index 0 (distance 4) is not.
    expect(isCommentable(lines, 'new', 1)).toBe(true);
    expect(isCommentable(lines, 'new', 0)).toBe(false);
    // Index 7 (distance 3) is commentable; index 8 (distance 4) is not.
    expect(isCommentable(lines, 'new', 7)).toBe(true);
    expect(isCommentable(lines, 'new', 8)).toBe(false);
  });

  it('treats an add-only or del-only pair as a change even with no span entry', () => {
    const file = makeFile({
      aligned: [
        [null, 0],
        [1, null],
      ],
      lhsLines: ['deleted line'],
      rhsLines: ['added line'],
    });
    const lines = computeCommentableLines(file);
    expect(isCommentable(lines, 'new', 0)).toBe(true);
    expect(isCommentable(lines, 'old', 1)).toBe(true);
  });

  it('leaves a file with no changes at all with nothing commentable', () => {
    const file = makeFile({
      aligned: [
        [0, 0],
        [1, 1],
      ],
    });
    const lines = computeCommentableLines(file);
    expect(lines.old.size).toBe(0);
    expect(lines.new.size).toBe(0);
  });
});

describe('firstCommentableLine', () => {
  it('prefers the new side when both sides have commentable lines', () => {
    const file = makeFile({
      aligned: [[0, 0]],
      rhsSpans: { '0': [{ start: 0, end: 1 }] },
    });
    const lines = computeCommentableLines(file);
    expect(firstCommentableLine(lines)).toEqual({ side: 'new', line: 0 });
  });

  it('falls back to the old side for a deletion-only file', () => {
    const file = makeFile({
      aligned: [[0, null]],
      lhsLines: ['deleted line'],
    });
    const lines = computeCommentableLines(file);
    expect(firstCommentableLine(lines)).toEqual({ side: 'old', line: 0 });
  });

  it('returns null when nothing is commentable', () => {
    const file = makeFile({
      aligned: [[0, 0]],
    });
    const lines = computeCommentableLines(file);
    expect(firstCommentableLine(lines)).toBeNull();
  });
});
