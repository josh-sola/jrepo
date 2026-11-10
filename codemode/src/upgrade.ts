import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { isRecord } from './env.ts';
import { UsageError, errorMessage } from './errors.ts';

export const REPO = 'josh-sola/jrepo';
const TAG_PREFIX = 'cm-v';

export type Release = {
  readonly tag_name: string;
  readonly draft: boolean;
  readonly prerelease: boolean;
};

const isRelease = (value: unknown): value is Release =>
  isRecord(value) &&
  typeof value.tag_name === 'string' &&
  typeof value.draft === 'boolean' &&
  typeof value.prerelease === 'boolean';

type Version = {
  readonly core: readonly [number, number, number];
  readonly pre: string | undefined;
};

const parseVersion = (text: string): Version | undefined => {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/.exec(text);
  if (!match) return undefined;
  return {
    core: [Number(match[1]), Number(match[2]), Number(match[3])],
    pre: match[4],
  };
};

/** A prerelease sorts before its release. */
export const compareVersions = (a: string, b: string): number => {
  const left = parseVersion(a);
  const right = parseVersion(b);
  if (!left || !right) {
    throw new Error(`Cannot compare versions "${a}" and "${b}".`);
  }
  for (const i of [0, 1, 2] as const) {
    const diff = left.core[i] - right.core[i];
    if (diff !== 0) return Math.sign(diff);
  }
  if (left.pre === right.pre) return 0;
  if (left.pre === undefined) return 1;
  if (right.pre === undefined) return -1;
  return left.pre < right.pre ? -1 : 1;
};

/** Skips drafts and prereleases, and drops the `cm-v` prefix. */
export const latestVersion = (
  releases: ReadonlyArray<Release>,
): string | undefined => {
  let latest: string | undefined;
  for (const release of releases) {
    if (release.draft || release.prerelease) continue;
    if (!release.tag_name.startsWith(TAG_PREFIX)) continue;
    const version = release.tag_name.slice(TAG_PREFIX.length);
    const parsed = parseVersion(version);
    if (!parsed || parsed.pre !== undefined) continue;
    if (latest === undefined || compareVersions(version, latest) > 0) {
      latest = version;
    }
  }
  return latest;
};

/** Reads both `sha256sum` and `shasum -a 256` lines. */
export const checksumFor = (sums: string, file: string): string | undefined => {
  for (const line of sums.split('\n')) {
    const match = /^([0-9a-fA-F]{64}) [ *](.+)$/.exec(line.trim());
    if (match?.[2] === file) return match[1]?.toLowerCase();
  }
  return undefined;
};

export type UpgradeEnv = Readonly<Record<string, string | undefined>>;

const apiBase = (env: UpgradeEnv) =>
  env.CM_GITHUB_API ?? 'https://api.github.com';

const downloadBase = (env: UpgradeEnv) =>
  env.CM_DOWNLOAD_BASE ?? `https://github.com/${REPO}/releases/download`;

const get = async (
  url: string,
  headers: Record<string, string> = {},
): Promise<Response> => {
  let response: Response;
  try {
    response = await fetch(url, {
      headers: { 'user-agent': 'cm', ...headers },
    });
  } catch (error) {
    throw new Error(`Cannot reach ${url}: ${errorMessage(error)}`);
  }
  if (!response.ok) {
    throw new Error(`${url} answered HTTP ${response.status}.`);
  }
  return response;
};

export const fetchLatestVersion = async (env: UpgradeEnv): Promise<string> => {
  const headers: Record<string, string> = {
    accept: 'application/vnd.github+json',
  };
  if (env.GITHUB_TOKEN) headers.authorization = `Bearer ${env.GITHUB_TOKEN}`;
  const url = `${apiBase(env)}/repos/${REPO}/releases?per_page=100`;
  const body: unknown = await (await get(url, headers)).json();
  if (!Array.isArray(body)) {
    throw new Error(`${url} did not return a list of releases.`);
  }
  const latest = latestVersion(body.filter(isRelease));
  if (latest === undefined) throw new Error('No cm release is published yet.');
  return latest;
};

// Without `gh` only the checksum is checked, and it cannot prove where the
// file came from, since SHA256SUMS ships in the same release.
const verifyAttestation = (file: string): void => {
  const result = spawnSync(
    'gh',
    ['attestation', 'verify', file, '--repo', REPO],
    { encoding: 'utf8' },
  );
  if (
    result.error &&
    'code' in result.error &&
    result.error.code === 'ENOENT'
  ) {
    process.stderr.write(
      'gh not found; skipped the build attestation check for install.sh.\n',
    );
    return;
  }
  if (result.status !== 0) {
    throw new Error(
      `The build attestation for install.sh did not verify.\n${result.stderr}`,
    );
  }
};

const runInstaller = (script: string, env: UpgradeEnv): Promise<number> =>
  new Promise((resolve, reject) => {
    const child = spawn('sh', [script], { env, stdio: 'inherit' });
    child.on('error', reject);
    child.on('exit', (code) => resolve(code ?? 1));
  });

export const upgrade = async (options: {
  readonly current: string;
  readonly check: boolean;
  readonly env: UpgradeEnv;
  readonly print: (text: string) => Promise<void>;
}): Promise<number> => {
  const { current, check, env, print } = options;
  if (!process.versions.bun) {
    throw new UsageError('This cm runs from a checkout; update with git pull.');
  }
  const latest = await fetchLatestVersion(env);
  if (check) {
    await print(`Installed: ${current}\nLatest: ${latest}`);
    return 0;
  }
  if (compareVersions(latest, current) <= 0) {
    await print(`cm ${current} is up to date.`);
    return 0;
  }
  const base = `${downloadBase(env)}/${TAG_PREFIX}${latest}`;
  const [script, sums] = await Promise.all([
    get(`${base}/install.sh`).then((r) => r.text()),
    get(`${base}/SHA256SUMS`).then((r) => r.text()),
  ]);
  const expected = checksumFor(sums, 'install.sh');
  const actual = createHash('sha256').update(script).digest('hex');
  if (expected !== actual) {
    throw new Error(`install.sh for ${latest} does not match its checksum.`);
  }
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cm-upgrade-'));
  try {
    const file = path.join(dir, 'install.sh');
    fs.writeFileSync(file, script);
    verifyAttestation(file);
    const code = await runInstaller(file, { ...env, CM_VERSION: latest });
    if (code !== 0) throw new Error(`install.sh exited with code ${code}.`);
    return 0;
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
};
