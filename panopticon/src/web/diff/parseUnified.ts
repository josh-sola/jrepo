// Dependency-free unified-diff parser: the fallback render when a plan's
// `payload` is null (a `--stdin` diff push, or any box without difft). Turns
// raw `git diff` text into per-file, per-line records with both-side line
// numbers, so the single-column fallback view can render line gutters and
// anchor comments without difftastic's rich alignment data.

export type UnifiedLineKind = 'add' | 'del' | 'context' | 'nonewline';

export interface UnifiedLine {
  kind: UnifiedLineKind;
  oldLine: number | null;
  newLine: number | null;
  text: string;
  // Anchor side/line per review.js semantics: '-' -> old, '+' -> new,
  // context -> new. `anchorLine` is 0-indexed (real line number - 1) so the
  // shared climbToCode/+1 logic in diff/anchor.ts works unmodified. null for
  // the "\ No newline at end of file" marker, which isn't selectable.
  anchorSide: 'old' | 'new' | null;
  anchorLine: number | null;
}

export interface UnifiedFile {
  path: string;
  oldPath: string | null;
  status: 'changed' | 'created' | 'deleted';
  lines: UnifiedLine[];
}

const DIFF_HEADER_RE = /^diff --git a\/(.+) b\/(.+)$/;
const HUNK_HEADER_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

function stripAbPrefix(raw: string): string {
  return raw === '/dev/null' ? raw : raw.replace(/^[ab]\//, '');
}

// A `--- `/`+++ ` pair seen while a hunk is still open is a real file
// boundary only if the declared counts are used up or the pair is
// immediately followed by a hunk header — hand-authored diff content is
// never followed by `@@`, so this can't misfire on it.
function isConfirmedFileHeader(
  oldRemaining: number,
  newRemaining: number,
  lineAfterPair: string | undefined,
): boolean {
  return (
    (oldRemaining <= 0 && newRemaining <= 0) ||
    (lineAfterPair?.startsWith('@@ ') ?? false)
  );
}

// A `--- `/`+++ ` pair with no preceding `diff --git` header — the only
// file-boundary signal headerless input has.
function fileFromHeaderlessPaths(
  oldRaw: string | null,
  newRaw: string,
): UnifiedFile {
  const oldIsNull = oldRaw === '/dev/null';
  const newIsNull = newRaw === '/dev/null';
  const path = newIsNull ? (oldRaw ?? newRaw) : newRaw;
  const oldPath =
    !oldIsNull && !newIsNull && oldRaw != null && oldRaw !== newRaw
      ? oldRaw
      : null;
  const status: UnifiedFile['status'] = oldIsNull
    ? 'created'
    : newIsNull
      ? 'deleted'
      : 'changed';
  return { path, oldPath, status, lines: [] };
}

export function parseUnifiedDiff(diffText: string): UnifiedFile[] {
  const files: UnifiedFile[] = [];
  const lines = diffText.split('\n');
  let current: UnifiedFile | null = null;
  let oldLine = 0;
  let newLine = 0;
  // Declared counts from the last `@@` header. They don't decide whether a
  // hunk is open — they are only one signal for isConfirmedFileHeader.
  let oldRemaining = 0;
  let newRemaining = 0;
  let hunkOpen = false;
  let pendingOldPath: string | null = null;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;

    const headerMatch = DIFF_HEADER_RE.exec(line);
    if (headerMatch) {
      current = {
        path: headerMatch[2]!,
        oldPath: null,
        status: 'changed',
        lines: [],
      };
      files.push(current);
      hunkOpen = false;
      oldRemaining = 0;
      newRemaining = 0;
      pendingOldPath = null;
      continue;
    }

    // A lone "--- " is never a header — only a "--- "/"+++ " pair does, and
    // only once it's confirmed as a real boundary (see isConfirmedFileHeader).
    if (
      line.startsWith('--- ') &&
      lines[i + 1]?.startsWith('+++ ') &&
      (!hunkOpen ||
        isConfirmedFileHeader(oldRemaining, newRemaining, lines[i + 2]))
    ) {
      pendingOldPath = stripAbPrefix(line.slice(4));
      hunkOpen = false;
      oldRemaining = 0;
      newRemaining = 0;
      continue;
    }
    if (!hunkOpen && pendingOldPath != null && line.startsWith('+++ ')) {
      const newPath = stripAbPrefix(line.slice(4));
      // A `diff --git` header already opened `current` for this exact file
      // and no hunk has run yet — this pair is just that file's metadata.
      if (!current || current.lines.length > 0) {
        current = fileFromHeaderlessPaths(pendingOldPath, newPath);
        files.push(current);
      }
      pendingOldPath = null;
      continue;
    }
    if (!current) continue;

    if (line.startsWith('rename from ')) {
      current.oldPath = line.slice('rename from '.length);
      continue;
    }
    if (line.startsWith('rename to ')) {
      current.path = line.slice('rename to '.length);
      continue;
    }
    if (line.startsWith('new file mode')) {
      current.status = 'created';
      continue;
    }
    if (line.startsWith('deleted file mode')) {
      current.status = 'deleted';
      continue;
    }
    if (
      line.startsWith('index ') ||
      line.startsWith('similarity index') ||
      line.startsWith('dissimilarity index') ||
      line.startsWith('Binary files ')
    ) {
      continue;
    }

    const hunkMatch = HUNK_HEADER_RE.exec(line);
    if (hunkMatch) {
      oldLine = parseInt(hunkMatch[1]!, 10);
      newLine = parseInt(hunkMatch[3]!, 10);
      oldRemaining = hunkMatch[2] != null ? parseInt(hunkMatch[2], 10) : 1;
      newRemaining = hunkMatch[4] != null ? parseInt(hunkMatch[4], 10) : 1;
      hunkOpen = true;
      continue;
    }
    // Doesn't count toward either side's remaining lines, so it can trail a hunk already at 0.
    if (line.startsWith('\\')) {
      current.lines.push({
        kind: 'nonewline',
        oldLine: null,
        newLine: null,
        text: line,
        anchorSide: null,
        anchorLine: null,
      });
      continue;
    }
    if (!hunkOpen) continue;

    if (line.startsWith('+')) {
      current.lines.push({
        kind: 'add',
        oldLine: null,
        newLine,
        text: line.slice(1),
        anchorSide: 'new',
        anchorLine: newLine - 1,
      });
      newLine++;
      newRemaining--;
      continue;
    }
    if (line.startsWith('-')) {
      current.lines.push({
        kind: 'del',
        oldLine,
        newLine: null,
        text: line.slice(1),
        anchorSide: 'old',
        anchorLine: oldLine - 1,
      });
      oldLine++;
      oldRemaining--;
      continue;
    }
    // A bare empty line is a context line whose leading space got stripped
    // (e.g. by whitespace-trimming before the diff reached this parser) —
    // except the one at the very end of `lines`, which is always just the
    // artifact of `diffText` ending in "\n", never real content.
    if (line.startsWith(' ') || (line === '' && i < lines.length - 1)) {
      current.lines.push({
        kind: 'context',
        oldLine,
        newLine,
        text: line.startsWith(' ') ? line.slice(1) : '',
        anchorSide: 'new',
        anchorLine: newLine - 1,
      });
      oldLine++;
      newLine++;
      oldRemaining--;
      newRemaining--;
      continue;
    }
    // Anything else (a bare "" already handled above, so this is truly
    // unrecognized) is dropped rather than closing the hunk — only a `@@`,
    // a `diff --git`, or a confirmed `--- `/`+++ ` pair does that.
  }

  return files;
}
