// localStorage persistence for a diff view's per-file "viewed" checkbox,
// keyed by a caller-supplied slug, with each file's mark validated against a
// fingerprint of that file's own content — so a change that touches one
// file clears only that file's mark, and files whose diff is unchanged keep
// theirs.
//
// panopticon's own viewed state lives server-side, keyed by blob oid (see
// ViewedResponse in src/shared/api.ts), so this module is not wired into the
// review page — kept in case a future client-only view (e.g. a stack-wide
// preview with no server round trip) earns a use for it.
import type { DiffPayload, PrDiff } from '../../shared/diff.ts';
import type { UnifiedFile } from './parseUnified.ts';

const STORAGE_PREFIX = 'panopticon_diff_viewed';
const STORAGE_VERSION = 2;

interface StoredViewedV2 {
  v: 2;
  files: Record<string, string>;
}

// djb2-ish string hash — only needs to be cheap and stable, not cryptographic.
// Takes a running hash so a fingerprint can be folded line by line instead of
// joining a whole file's content into one string first.
function foldHash(hash: number, input: string): number {
  let next = hash;
  for (let i = 0; i < input.length; i++) {
    next = (Math.imul(31, next) + input.charCodeAt(i)) | 0;
  }
  return next;
}

// A newline can't occur inside a line, so folding it between lines keeps the
// fingerprint injective — a space would let a split line hash the same as the
// unsplit one.
const LINE_SEP = '\n';

function foldLines(hash: number, lines: Iterable<string>): number {
  let next = hash;
  for (const line of lines) next = foldHash(foldHash(next, LINE_SEP), line);
  return next;
}

function finishHash(hash: number): string {
  return (hash >>> 0).toString(36);
}

export function fingerprintsForPayload(
  payload: PrDiff,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const file of payload.files) out[file.path] = fingerprintForFile(file);
  return out;
}

function fingerprintForFile(file: DiffPayload): string {
  let hash = foldHash(0, file.status);
  hash = foldHash(hash, file.oldPath ?? '');
  hash = foldHash(
    hash,
    `${file.stat.added}|${file.stat.removed}|${file.stat.modified}`,
  );
  hash = foldLines(hash, file.lhsLines);
  hash = foldLines(hash, file.rhsLines);
  return finishHash(hash);
}

export function fingerprintsForUnified(
  files: UnifiedFile[],
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const file of files) out[file.path] = fingerprintForUnifiedFile(file);
  return out;
}

function fingerprintForUnifiedFile(file: UnifiedFile): string {
  let hash = foldHash(0, file.status);
  hash = foldHash(hash, file.oldPath ?? '');
  for (const line of file.lines) {
    hash = foldHash(foldHash(hash, LINE_SEP), `${line.kind}|${line.text}`);
  }
  return finishHash(hash);
}

function storageKey(slug: string): string {
  return `${STORAGE_PREFIX}:${slug}`;
}

export function loadViewed(
  slug: string,
  fingerprints: Record<string, string>,
): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(storageKey(slug));
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Partial<StoredViewedV2>;
    if (
      parsed.v !== STORAGE_VERSION ||
      typeof parsed.files !== 'object' ||
      parsed.files === null
    ) {
      return {};
    }
    const viewed: Record<string, boolean> = {};
    for (const [path, fingerprint] of Object.entries(parsed.files)) {
      if (fingerprints[path] === fingerprint) viewed[path] = true;
    }
    return viewed;
  } catch {
    return {};
  }
}

export function saveViewed(
  slug: string,
  fingerprints: Record<string, string>,
  viewed: Record<string, boolean>,
): void {
  const files: Record<string, string> = {};
  for (const path of Object.keys(viewed)) {
    if (viewed[path] && fingerprints[path] != null)
      files[path] = fingerprints[path];
  }
  const stored: StoredViewedV2 = { v: STORAGE_VERSION, files };
  try {
    localStorage.setItem(storageKey(slug), JSON.stringify(stored));
  } catch {
    // best-effort — ignore quota/availability errors
  }
}
