import type { DiffPayload } from '../../shared/diff.ts';
import type { PrFile } from '../../shared/github.ts';
import { toDiffStatus } from './payload.ts';

// git treats a blob as binary when a NUL byte shows up in its first 8000
// bytes; matching that keeps collapse and rendering decisions consistent
// with what `git diff` itself would call binary.
const BINARY_SCAN_BYTES = 8000;

export function isBinary(bytes: Uint8Array): boolean {
  const limit = Math.min(bytes.length, BINARY_SCAN_BYTES);
  for (let i = 0; i < limit; i += 1) {
    if (bytes[i] === 0) return true;
  }
  return false;
}

export function buildBinaryPayload(file: PrFile): DiffPayload {
  return {
    path: file.path,
    oldPath: file.previousPath,
    status: toDiffStatus(file.status),
    language: 'Binary',
    aligned: [],
    lhsLines: [],
    rhsLines: [],
    lhsSpans: {},
    rhsSpans: {},
    stat: { added: 0, removed: 0, modified: 0 },
    structural: false,
    fallbackReason: 'binary',
    binary: true,
    collapseReason: null,
  };
}
