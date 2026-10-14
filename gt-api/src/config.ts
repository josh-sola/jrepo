import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { resolve } from 'node:path';

export interface RepoConfig {
  owner: string;
  repo: string;
  trunk: string;
  // The repo's local Graphite checkout. Only its Graphite files are read, so
  // it can be mounted read-only. Without it, stacks come from GitHub alone.
  checkout?: string;
}

export interface Config {
  port: number;
  dataDir: string;
  repos: RepoConfig[];
}

const DEFAULT_PORT = 7434;
const DEFAULT_DATA_DIR = '~/.local/share/gt-api';
const DEFAULT_CONFIG_PATH = '~/.config/gt-api/config.json';

const CONFIG_EXAMPLE = `{
  "repos": [
    { "owner": "your-org", "repo": "your-repo", "trunk": "main" }
  ]
}`;

class ConfigError extends Error {}

export function expandHome(path: string): string {
  if (path === '~') return homedir();
  if (path.startsWith('~/')) return resolve(homedir(), path.slice(2));
  return path;
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isRepoConfig(value: unknown): value is RepoConfig {
  if (typeof value !== 'object' || value === null) return false;
  const repo = value as Record<string, unknown>;
  return (
    isString(repo.owner) &&
    isString(repo.repo) &&
    isString(repo.trunk) &&
    (repo.checkout === undefined || isString(repo.checkout))
  );
}

export function configPath(): string {
  return expandHome(process.env.GT_API_CONFIG || DEFAULT_CONFIG_PATH);
}

export function loadConfig(path: string = configPath()): Config {
  if (!existsSync(path)) {
    throw new ConfigError(
      `gt-api config not found at ${path}. Create it with at least:\n${CONFIG_EXAMPLE}`,
    );
  }
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(path, 'utf-8'));
  } catch (cause) {
    throw new ConfigError(
      `gt-api config at ${path} is not valid JSON: ${String(cause)}`,
    );
  }
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ConfigError(`gt-api config at ${path} must be a JSON object.`);
  }
  const { repos } = raw as Record<string, unknown>;
  if (!Array.isArray(repos) || !repos.every(isRepoConfig)) {
    throw new ConfigError(
      `gt-api config at ${path}: "repos" must be an array of { owner, repo, trunk, checkout? }. Example:\n${CONFIG_EXAMPLE}`,
    );
  }

  const envPort = process.env.GT_API_PORT;
  if (envPort !== undefined && !/^\d+$/.test(envPort)) {
    throw new ConfigError(`GT_API_PORT must be a number, got "${envPort}".`);
  }
  const port = envPort === undefined ? DEFAULT_PORT : Number(envPort);

  const dataDir = expandHome(process.env.GT_API_DATA_DIR ?? DEFAULT_DATA_DIR);
  mkdirSync(dataDir, { recursive: true });

  return {
    port,
    dataDir,
    repos: repos.map((entry) => ({
      owner: entry.owner,
      repo: entry.repo,
      trunk: entry.trunk,
      ...(entry.checkout === undefined
        ? {}
        : { checkout: expandHome(entry.checkout) }),
    })),
  };
}

export function repoKey(owner: string, repo: string): string {
  return `${owner}/${repo}`.toLowerCase();
}

// GitHub treats owner and repo names as case-insensitive, so lookups do too.
export function indexRepos(repos: RepoConfig[]): Map<string, RepoConfig> {
  return new Map(
    repos.map((entry) => [repoKey(entry.owner, entry.repo), entry]),
  );
}
