import type { Database } from 'bun:sqlite';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import type { DiffPayload, PrDiff } from '../../shared/diff.ts';
import type { PrFile } from '../../shared/github.ts';
import type { CollapseRule } from '../config.ts';
import {
  type DiffCacheKey,
  type WhitespaceMode,
  getCachedDiff,
  setCachedDiff,
} from './cache.ts';
import { buildBinaryPayload, isBinary } from './binary.ts';
import { classifyCollapse } from './collapse.ts';
import { runDifft } from './difft.ts';
import { buildStructuralPayload } from './payload.ts';
import { Semaphore } from './semaphore.ts';
import { buildTextPayload } from './textdiff.ts';

export interface DiffDeps {
  readBlob(oid: string): Promise<Uint8Array | null>;
  generatedPaths(headSha: string, paths: string[]): Promise<Set<string>>;
  db: Database;
  tmpDir: string;
  rules: CollapseRule[];
}

export interface DiffInput {
  baseSha: string;
  headSha: string;
  files: PrFile[];
  ignoreWhitespace: boolean;
}

const MARKDOWN_EXTENSIONS = ['.md', '.mdx'];
const MAX_CONCURRENT_DIFFTS = 4;

export async function buildPrDiff(
  deps: DiffDeps,
  input: DiffInput,
): Promise<PrDiff> {
  const whitespace: WhitespaceMode = input.ignoreWhitespace ? 'ignore' : 'keep';
  const paths = input.files.map((file) => file.path);
  const generated = await deps.generatedPaths(input.headSha, paths);
  const semaphore = new Semaphore(MAX_CONCURRENT_DIFFTS);

  const files = await Promise.all(
    input.files.map((file) =>
      semaphore.run(() => buildFileDiff(deps, file, whitespace, generated)),
    ),
  );

  return { headSha: input.headSha, baseSha: input.baseSha, files };
}

async function buildFileDiff(
  deps: DiffDeps,
  file: PrFile,
  whitespace: WhitespaceMode,
  generated: Set<string>,
): Promise<DiffPayload> {
  const cacheKey: DiffCacheKey = {
    oldOid: file.oldOid,
    newOid: file.newOid,
    path: file.path,
    whitespace,
  };
  const cached = getCachedDiff(deps.db, cacheKey);
  if (cached !== null) return cached;

  const payload = await computeDiffPayload(deps, file, whitespace);
  const collapseReason = classifyCollapse({
    path: file.path,
    added: payload.stat.added,
    removed: payload.stat.removed,
    generated: generated.has(file.path),
    rules: deps.rules,
  });
  const withReason: DiffPayload = { ...payload, collapseReason };
  setCachedDiff(deps.db, cacheKey, withReason);
  return withReason;
}

async function computeDiffPayload(
  deps: DiffDeps,
  file: PrFile,
  whitespace: WhitespaceMode,
): Promise<DiffPayload> {
  const [oldBytes, newBytes] = await Promise.all([
    file.oldOid !== null ? deps.readBlob(file.oldOid) : Promise.resolve(null),
    file.newOid !== null ? deps.readBlob(file.newOid) : Promise.resolve(null),
  ]);

  if (
    (oldBytes !== null && isBinary(oldBytes)) ||
    (newBytes !== null && isBinary(newBytes))
  ) {
    return buildBinaryPayload(file);
  }

  const oldText = decodeBlob(oldBytes);
  const newText = decodeBlob(newBytes);

  if (isMarkdownPath(file.path)) {
    return buildTextPayload(file, oldText, newText, {
      ignoreWhitespace: whitespace === 'ignore',
      reason: 'markdown',
    });
  }

  return runStructuralOrFallback(deps, file, oldText, newText, whitespace);
}

function decodeBlob(bytes: Uint8Array | null): string {
  if (bytes === null) return '';
  return new TextDecoder('utf-8').decode(bytes);
}

function isMarkdownPath(path: string): boolean {
  const lower = path.toLowerCase();
  return MARKDOWN_EXTENSIONS.some((ext) => lower.endsWith(ext));
}

async function runStructuralOrFallback(
  deps: DiffDeps,
  file: PrFile,
  oldText: string,
  newText: string,
  whitespace: WhitespaceMode,
): Promise<DiffPayload> {
  const dir = await mkdtemp(join(deps.tmpDir, 'panopticon-difft-'));
  try {
    const oldPath = join(dir, 'old', file.previousPath ?? file.path);
    const newPath = join(dir, 'new', file.path);
    await Promise.all([
      mkdir(dirname(oldPath), { recursive: true }),
      mkdir(dirname(newPath), { recursive: true }),
    ]);
    await Promise.all([
      writeFile(oldPath, oldText, 'utf-8'),
      writeFile(newPath, newText, 'utf-8'),
    ]);
    const result = await runDifft(oldPath, newPath);
    if (result.language.startsWith('Text')) {
      return buildTextPayload(file, oldText, newText, {
        ignoreWhitespace: whitespace === 'ignore',
        reason: result.language,
      });
    }
    return buildStructuralPayload(file, oldText, newText, result);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}
