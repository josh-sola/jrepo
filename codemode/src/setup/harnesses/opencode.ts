import fs from 'node:fs';
import path from 'node:path';
import { isRecord } from '../../env.ts';
import { UsageError } from '../../errors.ts';
import { readIfExists, type Dirs } from '../files.ts';
import { loadJsonObject, parseJsonc, saveJsonEdit } from '../json.ts';
import {
  collectServers,
  httpConfig,
  isStringArray,
  isStringRecord,
  parseFailure,
  skip,
  type Converted,
} from './read.ts';
import type { Found, Harness } from './types.ts';

const ALLOW_PATTERN = 'cm *';

const configFiles = ({ configHome }: Dirs): [string, string] => {
  const dir = path.join(configHome, 'opencode');
  return [path.join(dir, 'opencode.json'), path.join(dir, 'opencode.jsonc')];
};

const ENV_REFERENCE = /\{env:([A-Za-z_][A-Za-z0-9_]*)\}/g;

const swapEnv = (text: string): string =>
  text.replace(ENV_REFERENCE, (_match, name: string) => '${' + name + '}');

const swapEnvIn = (record: Record<string, string>): Record<string, string> =>
  Object.fromEntries(
    Object.entries(record).map(([key, text]) => [key, swapEnv(text)]),
  );

const hasFileReference = (
  ...values: Array<string[] | Record<string, string>>
) =>
  values.some((value) =>
    Object.values(value).some((text) => text.includes('{file:')),
  );

const convert = (entry: unknown): Converted => {
  if (!isRecord(entry)) return skip('its entry is not an object');
  if (entry.enabled === false) return skip('it is disabled');
  if (entry.type === 'remote') {
    const headers = entry.headers ?? {};
    if (typeof entry.url !== 'string' || !isStringRecord(headers)) {
      return skip('its url or headers are malformed');
    }
    if (hasFileReference(headers, [entry.url])) {
      return skip('it reads a value from a {file:...} reference');
    }
    const result = httpConfig(entry.url, swapEnvIn(headers));
    if ('skip' in result) return result;
    const knownOauth =
      entry.oauth === false ? false : isRecord(entry.oauth) ? true : undefined;
    return { ...result, knownOauth };
  }
  if (entry.type !== 'local') return skip('its type is not local or remote');
  const env = entry.environment ?? {};
  const cwd = entry.cwd;
  if (
    !isStringArray(entry.command) ||
    !isStringRecord(env) ||
    (cwd !== undefined && typeof cwd !== 'string')
  ) {
    return skip('its command, environment or cwd are malformed');
  }
  if (hasFileReference(entry.command, env)) {
    return skip('it reads a value from a {file:...} reference');
  }
  const [command, ...args] = entry.command.map(swapEnv);
  if (command === undefined) return skip('its command is empty');
  return {
    config: {
      kind: 'stdio',
      command,
      args,
      env: swapEnvIn(env),
      cwd,
    },
  };
};

const allowsCm = (permission: unknown): boolean => {
  if (permission === 'allow') return true;
  if (!isRecord(permission)) return false;
  const { bash } = permission;
  return (
    bash === 'allow' || (isRecord(bash) && bash[ALLOW_PATTERN] === 'allow')
  );
};

export const opencode: Harness = {
  name: 'opencode',

  async readServers(dirs) {
    const results: Found[] = [];
    for (const file of configFiles(dirs)) {
      const text = readIfExists(file);
      if (text === undefined) continue;
      const data = parseJsonc(text);
      if (!isRecord(data)) {
        results.push(parseFailure('opencode', file));
        continue;
      }
      const servers = isRecord(data.mcp) ? data.mcp : {};
      results.push(
        collectServers('opencode', Object.entries(servers), convert),
      );
    }
    return {
      servers: results.flatMap((r) => r.servers),
      skipped: results.flatMap((r) => r.skipped),
    };
  },

  skillPath: ({ home }) =>
    path.join(home, '.agents', 'skills', 'cm', 'SKILL.md'),

  async allowCm(dirs) {
    const [json, jsonc] = configFiles(dirs);
    const file = fs.existsSync(json) || !fs.existsSync(jsonc) ? json : jsonc;
    const { text, data } = loadJsonObject(file);
    if (allowsCm(data.permission)) return undefined;
    const { permission } = data;
    // Later rules win in OpenCode, so a blanket rule goes first.
    if (typeof permission === 'string') {
      saveJsonEdit(file, text, ['permission'], {
        '*': permission,
        bash: { [ALLOW_PATTERN]: 'allow' },
      });
    } else if (isRecord(permission) && typeof permission.bash === 'string') {
      saveJsonEdit(file, text, ['permission', 'bash'], {
        '*': permission.bash,
        [ALLOW_PATTERN]: 'allow',
      });
    } else if (permission !== undefined && !isRecord(permission)) {
      throw new UsageError(
        `${file}: "permission" is not an object or a string.`,
      );
    } else {
      saveJsonEdit(file, text, ['permission', 'bash', ALLOW_PATTERN], 'allow');
    }
    return { file, summary: `allowed bash "${ALLOW_PATTERN}"` };
  },
};
