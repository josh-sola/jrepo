import path from 'node:path';
import { parse } from 'smol-toml';
import { isRecord } from '../../env.ts';
import { readIfExists, writeFile } from '../files.ts';
import {
  collectServers,
  httpConfig,
  isStringArray,
  isStringRecord,
  parseFailure,
  skip,
  type Converted,
} from './read.ts';
import type { Harness } from './types.ts';

const ALLOW_RULE = 'prefix_rule(pattern=["cm"], decision="allow")';

const variable = (name: string) => '${' + name + '}';

// A relative command, such as one a desktop app injects, only works from its own cwd.
const isRelativePath = (command: string): boolean =>
  command.includes('/') &&
  !path.isAbsolute(command) &&
  !command.startsWith('~');

const convert = (entry: unknown): Converted => {
  if (!isRecord(entry)) return skip('its entry is not a table');
  if (entry.enabled === false) return skip('it is disabled');
  if (typeof entry.url === 'string') {
    if ('http_headers_helper' in entry) {
      return skip('it uses http_headers_helper');
    }
    if (entry.auth === 'chatgpt') return skip('it uses ChatGPT sign-in');
    const headers = entry.http_headers ?? {};
    const fromEnv = entry.env_http_headers ?? {};
    if (!isStringRecord(headers) || !isStringRecord(fromEnv)) {
      return skip('its headers are malformed');
    }
    const bearer = entry.bearer_token_env_var;
    if (bearer !== undefined && typeof bearer !== 'string') {
      return skip('its bearer_token_env_var is malformed');
    }
    const result = httpConfig(entry.url, {
      ...headers,
      ...Object.fromEntries(
        Object.entries(fromEnv).map(([header, name]) => [
          header,
          variable(name),
        ]),
      ),
      ...(bearer === undefined
        ? {}
        : { Authorization: `Bearer ${variable(bearer)}` }),
    });
    return 'skip' in result
      ? result
      : { ...result, knownOauth: entry.auth === 'oauth' ? true : undefined };
  }
  if (typeof entry.command !== 'string') {
    return skip('it has no command or url');
  }
  if (isRelativePath(entry.command)) {
    return skip('its command is a relative path');
  }
  const args = entry.args ?? [];
  const env = entry.env ?? {};
  const passed = entry.env_vars ?? [];
  const cwd = entry.cwd;
  if (
    !isStringArray(args) ||
    !isStringRecord(env) ||
    !isStringArray(passed) ||
    (cwd !== undefined && typeof cwd !== 'string')
  ) {
    return skip('its args, env or cwd are malformed');
  }
  return {
    config: {
      kind: 'stdio',
      command: entry.command,
      args,
      env: {
        ...Object.fromEntries(passed.map((name) => [name, variable(name)])),
        ...env,
      },
      cwd,
    },
  };
};

export const codex: Harness = {
  name: 'codex',

  async readServers({ home }) {
    const file = path.join(home, '.codex', 'config.toml');
    const text = readIfExists(file);
    if (text === undefined) return { servers: [], skipped: [] };
    let data: unknown;
    try {
      data = parse(text);
    } catch {
      return parseFailure('codex', file);
    }
    const servers =
      isRecord(data) && isRecord(data.mcp_servers) ? data.mcp_servers : {};
    return collectServers('codex', Object.entries(servers), convert);
  },

  skillPath: ({ home }) =>
    path.join(home, '.agents', 'skills', 'cm', 'SKILL.md'),

  async allowCm({ home }) {
    const file = path.join(home, '.codex', 'rules', 'default.rules');
    const text = readIfExists(file) ?? '';
    if (text.split('\n').some((line) => line.trim() === ALLOW_RULE)) {
      return undefined;
    }
    const separator = text === '' || text.endsWith('\n') ? '' : '\n';
    writeFile(file, `${text}${separator}${ALLOW_RULE}\n`, { backup: true });
    return { file, summary: 'allowed cm' };
  },
};
