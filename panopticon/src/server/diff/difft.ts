// In difft 0.69.0, `aligned_lines` and every chunk `line_number` share one
// 0-based index space, so a chunk indexes straight into the lines array.

export interface DifftChange {
  start: number;
  end: number;
  content?: string;
  highlight?: string;
}

export interface DifftLineSide {
  line_number: number;
  changes: DifftChange[];
}

export interface DifftChunkItem {
  lhs?: DifftLineSide;
  rhs?: DifftLineSide;
}

export type DifftChunk = DifftChunkItem[];

export type DifftStatus = 'changed' | 'unchanged' | 'created' | 'deleted';

export interface DifftFile {
  path: string;
  language: string;
  status: DifftStatus;
  aligned_lines?: [number | null, number | null][];
  chunks?: DifftChunk[];
}

function isDifftChange(value: unknown): value is DifftChange {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.start === 'number' &&
    typeof v.end === 'number' &&
    (v.content === undefined || typeof v.content === 'string') &&
    (v.highlight === undefined || typeof v.highlight === 'string')
  );
}

function isDifftLineSide(value: unknown): value is DifftLineSide {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  if (typeof v.line_number !== 'number') return false;
  if (v.changes === undefined) return true;
  return Array.isArray(v.changes) && v.changes.every(isDifftChange);
}

function isDifftChunkItem(value: unknown): value is DifftChunkItem {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  if (v.lhs !== undefined && !isDifftLineSide(v.lhs)) return false;
  if (v.rhs !== undefined && !isDifftLineSide(v.rhs)) return false;
  return true;
}

function isAlignedPair(
  value: unknown,
): value is [number | null, number | null] {
  if (!Array.isArray(value) || value.length !== 2) return false;
  const [a, b] = value as unknown[];
  return (
    (a === null || typeof a === 'number') &&
    (b === null || typeof b === 'number')
  );
}

function isDifftStatus(value: unknown): value is DifftStatus {
  return (
    value === 'changed' ||
    value === 'unchanged' ||
    value === 'created' ||
    value === 'deleted'
  );
}

export function isDifftFile(value: unknown): value is DifftFile {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  if (typeof v.path !== 'string') return false;
  if (typeof v.language !== 'string') return false;
  if (!isDifftStatus(v.status)) return false;
  if (v.aligned_lines !== undefined) {
    if (
      !Array.isArray(v.aligned_lines) ||
      !v.aligned_lines.every(isAlignedPair)
    ) {
      return false;
    }
  }
  if (v.chunks !== undefined) {
    if (!Array.isArray(v.chunks)) return false;
    for (const chunk of v.chunks) {
      if (!Array.isArray(chunk) || !chunk.every(isDifftChunkItem)) return false;
    }
  }
  return true;
}

// difft emits one JSON object per line; brace-matching is the fallback for
// output that does not split cleanly.
export function parseDifftOutput(raw: string): DifftFile[] {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return [];
  const jsonl = tryParseJsonl(trimmed);
  if (jsonl !== null) return jsonl;
  return parseConcatenatedObjects(trimmed).filter(isDifftFile);
}

function tryParseJsonl(text: string): DifftFile[] | null {
  const lines = text.split('\n').filter((line) => line.trim().length > 0);
  const files: DifftFile[] = [];
  for (const line of lines) {
    let value: unknown;
    try {
      value = JSON.parse(line);
    } catch {
      return null;
    }
    if (!isDifftFile(value)) return null;
    files.push(value);
  }
  return files;
}

function parseConcatenatedObjects(text: string): unknown[] {
  const objects: unknown[] = [];
  const n = text.length;
  let i = 0;
  while (i < n) {
    while (i < n && /\s/.test(text[i] ?? '')) i += 1;
    if (i >= n) break;
    const start = i;
    let depth = 0;
    let inString = false;
    let escaped = false;
    while (i < n) {
      const c = text[i];
      if (inString) {
        if (escaped) {
          escaped = false;
        } else if (c === '\\') {
          escaped = true;
        } else if (c === '"') {
          inString = false;
        }
        i += 1;
      } else if (c === '"') {
        inString = true;
        i += 1;
      } else if (c === '{') {
        depth += 1;
        i += 1;
      } else if (c === '}') {
        depth -= 1;
        i += 1;
        if (depth === 0) break;
      } else {
        i += 1;
      }
    }
    try {
      objects.push(JSON.parse(text.slice(start, i)));
    } catch {
      break;
    }
  }
  return objects;
}

export const DIFFT_TIMEOUT_MS = 20_000;

export async function runDifft(
  oldFile: string,
  newFile: string,
  timeoutMs: number = DIFFT_TIMEOUT_MS,
): Promise<DifftFile> {
  const proc = Bun.spawn(['difft', '--display', 'json', oldFile, newFile], {
    env: { ...process.env, DFT_UNSTABLE: 'yes' },
    stdout: 'pipe',
    stderr: 'pipe',
  });
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    proc.kill();
  }, timeoutMs);
  let stdout: string;
  let stderr: string;
  let exitCode: number;
  try {
    [stdout, stderr, exitCode] = await Promise.all([
      new Response(proc.stdout).text(),
      new Response(proc.stderr).text(),
      proc.exited,
    ]);
  } finally {
    clearTimeout(timer);
  }
  if (timedOut) {
    throw new Error(`difft timed out after ${timeoutMs / 1000}s on ${newFile}`);
  }
  if (exitCode !== 0) {
    throw new Error(`difft failed on ${newFile} (exit ${exitCode}): ${stderr}`);
  }
  const files = parseDifftOutput(stdout);
  const file = files[0];
  if (file === undefined) {
    throw new Error(`difft produced no parseable output for ${newFile}`);
  }
  return file;
}

export async function assertDifftVersion(expected: string): Promise<void> {
  const proc = Bun.spawn(['difft', '--version'], {
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const [stdout, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    proc.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(`difft --version exited ${exitCode}`);
  }
  const match = /Difftastic (\S+)/.exec(stdout);
  const actual = match?.[1] ?? 'unknown';
  if (actual !== expected) {
    throw new Error(
      `difft version mismatch: expected ${expected}, found ${actual}. The diff engine's JSON parsing is pinned to a specific difft build.`,
    );
  }
}
