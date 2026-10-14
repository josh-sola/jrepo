import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { resolve } from 'node:path';

export type CollapseReason =
  | 'generated'
  | 'test'
  | 'lockfile'
  | 'snapshot'
  | 'large';

export interface CollapseRule {
  glob: string;
  reason: CollapseReason;
}

export type HoverProfile = 'node' | 'python';

// One shell command that prepares a hover tree. `cwd` is relative to the
// tree's root.
export interface HoverStep {
  profile: HoverProfile;
  run: string;
  cwd?: string;
}

export interface RepoConfig {
  // Where the repo's local Graphite checkout lives; only its Graphite files
  // are read.
  referenceClone?: string;
  trunk: string;
  // Hover is on for a repo only when this is set.
  hover?: { steps: HoverStep[] };
}

export interface Config {
  githubLogin: string;
  port: number;
  dataDir: string;
  pollIntervalSeconds: number;
  collapse: { rules: CollapseRule[] };
  hover: { maxTrees: number };
  repos: Record<string, RepoConfig>;
}

const DEFAULT_PORT = 7433;
const DEFAULT_POLL_INTERVAL_SECONDS = 60;
const DEFAULT_MAX_TREES = 3;
const DEFAULT_DATA_DIR = '~/.local/share/panopticon';

const DEFAULT_COLLAPSE_RULES: CollapseRule[] = [
  { glob: '**/*.test.ts', reason: 'test' },
  { glob: '**/*.test.tsx', reason: 'test' },
  { glob: '**/tests/**', reason: 'test' },
  { glob: '**/test_*.py', reason: 'test' },
  { glob: '**/*_test.py', reason: 'test' },
  { glob: '**/conftest.py', reason: 'test' },
  { glob: '**/pnpm-lock.yaml', reason: 'lockfile' },
  { glob: '**/uv.lock', reason: 'lockfile' },
  { glob: '**/bun.lock', reason: 'lockfile' },
  { glob: '**/__snapshots__/**', reason: 'snapshot' },
];

const CONFIG_EXAMPLE = `{
  "githubLogin": "your-github-login",
  "repos": {
    "owner/name": { "trunk": "main" }
  }
}`;

export function expandHome(path: string): string {
  if (path === '~') return homedir();
  if (path.startsWith('~/')) return resolve(homedir(), path.slice(2));
  return path;
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function isPositiveInteger(value: unknown): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= 1;
}

function isCollapseReason(value: unknown): value is CollapseReason {
  return (
    value === 'generated' ||
    value === 'test' ||
    value === 'lockfile' ||
    value === 'snapshot' ||
    value === 'large'
  );
}

function isCollapseRule(value: unknown): value is CollapseRule {
  if (typeof value !== 'object' || value === null) return false;
  const rule = value as Record<string, unknown>;
  return isString(rule.glob) && isCollapseReason(rule.reason);
}

function isHoverStep(value: unknown): value is HoverStep {
  if (typeof value !== 'object' || value === null) return false;
  const step = value as Record<string, unknown>;
  return (
    (step.profile === 'node' || step.profile === 'python') &&
    isString(step.run) &&
    (step.cwd === undefined || isString(step.cwd))
  );
}

function isHoverConfig(value: unknown): value is { steps: HoverStep[] } {
  if (typeof value !== 'object' || value === null) return false;
  const { steps } = value as Record<string, unknown>;
  return Array.isArray(steps) && steps.every(isHoverStep);
}

function isRepoConfig(value: unknown): value is RepoConfig {
  if (typeof value !== 'object' || value === null) return false;
  const repo = value as Record<string, unknown>;
  if (!isString(repo.trunk)) return false;
  if (repo.hover !== undefined && !isHoverConfig(repo.hover)) return false;
  if (repo.referenceClone !== undefined && !isString(repo.referenceClone))
    return false;
  return true;
}

class ConfigError extends Error {}

function readConfigFile(path: string): Record<string, unknown> {
  if (!existsSync(path)) {
    throw new ConfigError(
      `panopticon config not found at ${path}. Create it with at least:\n${CONFIG_EXAMPLE}`,
    );
  }
  const raw = readFileSync(path, 'utf-8');
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (cause) {
    throw new ConfigError(
      `panopticon config at ${path} is not valid JSON: ${String(cause)}`,
    );
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new ConfigError(
      `panopticon config at ${path} must be a JSON object.`,
    );
  }
  return parsed as Record<string, unknown>;
}

export function configPath(): string {
  const override = process.env.PANOPTICON_CONFIG;
  if (override) return expandHome(override);
  return expandHome('~/.config/panopticon/config.json');
}

export function loadConfig(path: string = configPath()): Config {
  const raw = readConfigFile(path);

  if (!isString(raw.githubLogin) || raw.githubLogin.length === 0) {
    throw new ConfigError(
      `panopticon config at ${path} is missing "githubLogin". Example:\n${CONFIG_EXAMPLE}`,
    );
  }

  // PANOPTICON_PORT and PANOPTICON_DATA_DIR let `bun run dev` share the
  // installed service's config without taking its port or its database.
  const envPort = process.env.PANOPTICON_PORT;
  if (envPort !== undefined && !/^\d+$/.test(envPort)) {
    throw new ConfigError(
      `PANOPTICON_PORT must be a number, got "${envPort}".`,
    );
  }
  const port =
    envPort !== undefined
      ? Number(envPort)
      : raw.port === undefined
        ? DEFAULT_PORT
        : raw.port;
  if (!isFiniteNumber(port)) {
    throw new ConfigError(
      `panopticon config at ${path}: "port" must be a number.`,
    );
  }

  const pollIntervalSeconds =
    raw.pollIntervalSeconds === undefined
      ? DEFAULT_POLL_INTERVAL_SECONDS
      : raw.pollIntervalSeconds;
  if (!isFiniteNumber(pollIntervalSeconds)) {
    throw new ConfigError(
      `panopticon config at ${path}: "pollIntervalSeconds" must be a number.`,
    );
  }

  const dataDir = expandHome(
    process.env.PANOPTICON_DATA_DIR ??
      (isString(raw.dataDir) ? raw.dataDir : DEFAULT_DATA_DIR),
  );

  let collapseRules = DEFAULT_COLLAPSE_RULES;
  if (raw.collapse !== undefined) {
    if (typeof raw.collapse !== 'object' || raw.collapse === null) {
      throw new ConfigError(
        `panopticon config at ${path}: "collapse" must be an object.`,
      );
    }
    const rules = (raw.collapse as Record<string, unknown>).rules;
    if (rules !== undefined) {
      if (!Array.isArray(rules) || !rules.every(isCollapseRule)) {
        throw new ConfigError(
          `panopticon config at ${path}: "collapse.rules" must be an array of { glob, reason }.`,
        );
      }
      collapseRules = rules;
    }
  }

  let maxTrees = DEFAULT_MAX_TREES;
  if (raw.hover !== undefined) {
    if (typeof raw.hover !== 'object' || raw.hover === null) {
      throw new ConfigError(
        `panopticon config at ${path}: "hover" must be an object.`,
      );
    }
    const configured = (raw.hover as Record<string, unknown>).maxTrees;
    if (configured !== undefined) {
      if (!isPositiveInteger(configured)) {
        throw new ConfigError(
          `panopticon config at ${path}: "hover.maxTrees" must be a positive integer.`,
        );
      }
      maxTrees = configured;
    }
  }

  const repos: Record<string, RepoConfig> = {};
  if (raw.repos !== undefined) {
    if (
      typeof raw.repos !== 'object' ||
      raw.repos === null ||
      Array.isArray(raw.repos)
    ) {
      throw new ConfigError(
        `panopticon config at ${path}: "repos" must be an object.`,
      );
    }
    for (const [key, value] of Object.entries(
      raw.repos as Record<string, unknown>,
    )) {
      if (!isRepoConfig(value)) {
        throw new ConfigError(
          `panopticon config at ${path}: repos["${key}"] must have a "trunk" string, and "hover" must be { steps: [{ profile, run, cwd? }] } when set.`,
        );
      }
      repos[key] = {
        trunk: value.trunk,
        ...(value.hover !== undefined ? { hover: value.hover } : {}),
        ...(value.referenceClone !== undefined
          ? { referenceClone: expandHome(value.referenceClone) }
          : {}),
      };
    }
  }

  mkdirSync(dataDir, { recursive: true });

  return {
    githubLogin: raw.githubLogin,
    port,
    dataDir,
    pollIntervalSeconds,
    collapse: { rules: collapseRules },
    hover: { maxTrees },
    repos,
  };
}
