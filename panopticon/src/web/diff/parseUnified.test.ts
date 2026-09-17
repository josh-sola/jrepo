import { describe, expect, it } from 'bun:test';
import { parseUnifiedDiff } from './parseUnified.ts';
import { unifiedDiffText } from './__fixtures__/diffPayload.ts';

describe('parseUnifiedDiff', () => {
  it('parses hunks with correct line numbers on both sides', () => {
    const file = parseUnifiedDiff(unifiedDiffText)[0]!;
    expect(file.path).toBe('src/app.py');
    expect(file.status).toBe('changed');
    expect(file.lines[0]).toMatchObject({
      kind: 'del',
      oldLine: 1,
      newLine: null,
      text: 'def greet(name):',
      anchorSide: 'old',
      anchorLine: 0,
    });
    expect(file.lines[1]).toMatchObject({
      kind: 'del',
      oldLine: 2,
      newLine: null,
    });
    expect(file.lines[2]).toMatchObject({
      kind: 'add',
      oldLine: null,
      newLine: 1,
      text: 'def greet(name: str) -> str:',
      anchorSide: 'new',
      anchorLine: 0,
    });
    expect(file.lines[3]).toMatchObject({
      kind: 'add',
      oldLine: null,
      newLine: 2,
    });
    expect(file.lines[4]).toMatchObject({
      kind: 'context',
      oldLine: 3,
      newLine: 3,
      text: '',
      anchorSide: 'new',
      anchorLine: 2,
    });
    expect(file.lines[5]).toMatchObject({
      kind: 'context',
      oldLine: 4,
      newLine: 4,
      text: "print(greet('world'))",
      anchorSide: 'new',
      anchorLine: 3,
    });
  });

  it('parses multiple files from one diff, each with its own line-number sequence', () => {
    const files = parseUnifiedDiff(unifiedDiffText);
    expect(files.map((f) => f.path)).toEqual([
      'src/app.py',
      'web/src/newFeature.ts',
      'legacy/old_math.py',
      'src/utils/strings.py',
    ]);
  });

  it('marks a new-file push as created and a whole-file removal as deleted', () => {
    const files = parseUnifiedDiff(unifiedDiffText);
    const created = files.find((f) => f.path === 'web/src/newFeature.ts')!;
    expect(created.status).toBe('created');
    expect(created.lines[0]).toMatchObject({ kind: 'add', newLine: 1 });

    const deleted = files.find((f) => f.path === 'legacy/old_math.py')!;
    expect(deleted.status).toBe('deleted');
    expect(deleted.lines.every((l) => l.kind === 'del')).toBe(true);
  });

  it("captures a rename's old and new path", () => {
    const files = parseUnifiedDiff(unifiedDiffText);
    const renamed = files.find((f) => f.path === 'src/utils/strings.py')!;
    expect(renamed.oldPath).toBe('src/utils/str_helpers.py');
    expect(renamed.lines.filter((l) => l.kind === 'add')).toHaveLength(3);
  });

  it('surfaces a "\\ No newline at end of file" marker as a non-anchorable line', () => {
    const files = parseUnifiedDiff(unifiedDiffText);
    const created = files.find((f) => f.path === 'web/src/newFeature.ts')!;
    const marker = created.lines[created.lines.length - 1]!;
    expect(marker.kind).toBe('nonewline');
    expect(marker.anchorSide).toBeNull();
    expect(marker.anchorLine).toBeNull();
  });

  it('returns no files for an empty diff', () => {
    expect(parseUnifiedDiff('')).toEqual([]);
  });

  it("keeps a hunk intact when a deleted line's own content starts with '--- '", () => {
    const diff = `--- a/a.sql
+++ b/a.sql
@@ -1,2 +0,0 @@
-- a SQL comment
-removed too
`;
    const file = parseUnifiedDiff(diff)[0]!;
    expect(file.lines).toHaveLength(2);
    expect(file.lines[0]).toMatchObject({
      kind: 'del',
      text: '- a SQL comment',
    });
    expect(file.lines[1]).toMatchObject({ kind: 'del', text: 'removed too' });
  });

  it("keeps a single file when an added line's own content starts with '++ '", () => {
    const diff = `--- a/a.txt
+++ b/a.txt
@@ -1,1 +1,2 @@
 keep
+++ x
`;
    const files = parseUnifiedDiff(diff);
    expect(files).toHaveLength(1);
    expect(files[0]!.lines).toMatchObject([
      { kind: 'context', text: 'keep' },
      { kind: 'add', text: '++ x' },
    ]);
  });

  it("splits a headerless multi-file diff even when the next file's header immediately follows the last hunk line", () => {
    const diff = `--- a/a.py
+++ b/a.py
@@ -1,1 +1,1 @@
-old a
+new a
--- a/b.py
+++ b/b.py
@@ -1,1 +1,1 @@
-old b
+new b
`;
    const files = parseUnifiedDiff(diff);
    expect(files.map((f) => f.path)).toEqual(['a.py', 'b.py']);
    expect(files[0]!.lines.map((l) => l.text)).toEqual(['old a', 'new a']);
    expect(files[1]!.lines.map((l) => l.text)).toEqual(['old b', 'new b']);
  });

  it('treats an omitted hunk count as 1', () => {
    const diff = `--- a/a.txt
+++ b/a.txt
@@ -3 +3 @@
-old
+new
`;
    const file = parseUnifiedDiff(diff)[0]!;
    expect(file.lines).toMatchObject([
      { kind: 'del', oldLine: 3, text: 'old' },
      { kind: 'add', newLine: 3, text: 'new' },
    ]);
  });

  it("treats a bare empty line mid-hunk as a stripped context line, and still splits the next file's header", () => {
    const diff = `--- a/x.py
+++ b/x.py
@@ -1,3 +1,3 @@
 first

-old
+new
--- a/y.py
+++ b/y.py
@@ -1,2 +1,2 @@
 keep
-gone
+here
`;
    const files = parseUnifiedDiff(diff);
    expect(files.map((f) => f.path)).toEqual(['x.py', 'y.py']);
    expect(files[0]!.lines.map((l) => l.kind)).toEqual([
      'context',
      'context',
      'del',
      'add',
    ]);
  });

  it('keeps a hunk intact around a blank mid-hunk context line, with correct line numbers on both sides', () => {
    const diff = `--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,4 @@
 before

 after
-old
+new
`;
    const file = parseUnifiedDiff(diff)[0]!;
    expect(file.lines).toMatchObject([
      { kind: 'context', oldLine: 1, newLine: 1, text: 'before' },
      { kind: 'context', oldLine: 2, newLine: 2, text: '' },
      { kind: 'context', oldLine: 3, newLine: 3, text: 'after' },
      { kind: 'del', oldLine: 4, text: 'old' },
      { kind: 'add', newLine: 4, text: 'new' },
    ]);
  });

  it('does not gain a phantom trailing context line when the diff text ends in a newline', () => {
    const diff = `--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
-old
+new
`;
    const file = parseUnifiedDiff(diff)[0]!;
    expect(file.lines.map((l) => l.kind)).toEqual(['del', 'add']);
  });

  it('still splits two files when a hunk under-declares its line counts', () => {
    const diff = `--- a/a.py
+++ b/a.py
@@ -1,1 +1,1 @@
-old a
-old a2
+new a
+new a2
--- a/b.py
+++ b/b.py
@@ -1,1 +1,1 @@
-old b
+new b
`;
    const files = parseUnifiedDiff(diff);
    expect(files.map((f) => f.path)).toEqual(['a.py', 'b.py']);
    expect(files[0]!.lines.map((l) => l.text)).toEqual([
      'old a',
      'old a2',
      'new a',
      'new a2',
    ]);
    expect(files[1]!.lines.map((l) => l.text)).toEqual(['old b', 'new b']);
  });

  it('splits two files even when a hunk declares more lines than its body actually has', () => {
    const diff = `--- a/web/src/first.ts
+++ b/web/src/first.ts
@@ -1,3 +1,3 @@
-old first
+new first
--- a/web/src/second.ts
+++ b/web/src/second.ts
@@ -1,1 +1,1 @@
-old second
+new second
`;
    const files = parseUnifiedDiff(diff);
    expect(files.map((f) => f.path)).toEqual([
      'web/src/first.ts',
      'web/src/second.ts',
    ]);
    expect(files[0]!.lines.map((l) => l.text)).toEqual([
      'old first',
      'new first',
    ]);
    expect(files[1]!.lines.map((l) => l.text)).toEqual([
      'old second',
      'new second',
    ]);
  });

  it('keeps every line of a hunk whose body is longer than its declared count', () => {
    const diff = `--- a/a.txt
+++ b/a.txt
@@ -1,1 +1,1 @@
-old
+new
+extra
`;
    const file = parseUnifiedDiff(diff)[0]!;
    expect(file.lines.map((l) => l.text)).toEqual(['old', 'new', 'extra']);
  });
});
