import { describe, expect, test } from 'bun:test';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { runDifft } from './difft.ts';
import { buildStructuralPayload } from './payload.ts';
import type { PrFile } from '../../shared/github.ts';

// This payload's aligned/lhsSpans/rhsSpans must agree byte-for-byte with
// planhub's push-diff.py, since the web renderer being ported here depends
// on that exact shape. The script lives in a sibling repo, so this check
// only runs when that checkout happens to be present on disk.
const PUSH_DIFF_PY = join(
  homedir(),
  'repos/toy-apps/claude-plugins/planhub/scripts/push-diff.py',
);

function run(cmd: string[], cwd: string): { stdout: string; status: number } {
  const proc = Bun.spawnSync(cmd, { cwd, stdout: 'pipe', stderr: 'pipe' });
  return { stdout: proc.stdout.toString('utf-8'), status: proc.exitCode };
}

const available = existsSync(PUSH_DIFF_PY);

describe.skipIf(!available)('planhub push-diff.py compatibility', () => {
  test('aligned, lhsSpans, and rhsSpans match planhub exactly', async () => {
    const repoDir = mkdtempSync(join(tmpdir(), 'panopticon-compat-test-'));
    try {
      const fixtures = join(import.meta.dir, '__fixtures__', 'typescript');
      const oldText = readFileSync(join(fixtures, 'old.ts'), 'utf-8');
      const newText = readFileSync(join(fixtures, 'new.ts'), 'utf-8');

      run(['git', 'init', '-q'], repoDir);
      run(['git', 'config', 'user.email', 'test@panopticon.local'], repoDir);
      run(['git', 'config', 'user.name', 'panopticon-test'], repoDir);
      await Bun.write(join(repoDir, 'math.ts'), oldText);
      run(['git', 'add', 'math.ts'], repoDir);
      run(['git', 'commit', '-q', '-m', 'base'], repoDir);
      await Bun.write(join(repoDir, 'math.ts'), newText);
      run(['git', 'add', 'math.ts'], repoDir);
      run(['git', 'commit', '-q', '-m', 'head'], repoDir);

      const log = run(['git', 'log', '--format=%H'], repoDir)
        .stdout.trim()
        .split('\n');
      const headSha = log[0];
      const baseSha = log[1];
      if (headSha === undefined || baseSha === undefined) {
        throw new Error('expected two commits in the compat test repo');
      }

      const pyScript = `
import importlib.util, json
spec = importlib.util.spec_from_file_location("push_diff", ${JSON.stringify(PUSH_DIFF_PY)})
push_diff = importlib.util.module_from_spec(spec)
spec.loader.exec_module(push_diff)
payload, _ = push_diff.generate(${JSON.stringify(baseSha)}, None, ${JSON.stringify(headSha)})
print(json.dumps(payload))
`;
      const pyResult = run(['python3', '-c', pyScript], repoDir);
      expect(pyResult.status).toBe(0);
      const planhubPayload = JSON.parse(pyResult.stdout) as {
        files: {
          aligned: [number | null, number | null][];
          lhsSpans: Record<string, unknown>;
          rhsSpans: Record<string, unknown>;
        }[];
      };
      const planhubFile = planhubPayload.files[0];
      if (planhubFile === undefined) {
        throw new Error('push-diff.py produced no files');
      }

      const result = await runDifft(
        join(fixtures, 'old.ts'),
        join(fixtures, 'new.ts'),
      );
      const file: PrFile = {
        path: 'math.ts',
        previousPath: null,
        status: 'modified',
        additions: 0,
        deletions: 0,
        oldOid: 'old-oid',
        newOid: 'new-oid',
      };
      const payload = buildStructuralPayload(file, oldText, newText, result);

      expect(payload.aligned).toEqual(planhubFile.aligned);
      expect(payload.lhsSpans).toEqual(
        planhubFile.lhsSpans as typeof payload.lhsSpans,
      );
      expect(payload.rhsSpans).toEqual(
        planhubFile.rhsSpans as typeof payload.rhsSpans,
      );
    } finally {
      rmSync(repoDir, { recursive: true, force: true });
    }
  });
});
