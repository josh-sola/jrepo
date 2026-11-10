import path from 'node:path';
import { isRecord } from '../../env.ts';
import { UsageError } from '../../errors.ts';
import { readIfExists } from '../files.ts';
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
import type { Harness } from './types.ts';

const ALLOW_RULE = 'Bash(cm *)';

const convert = (entry: unknown): Converted => {
  if (!isRecord(entry)) return skip('its entry is not an object');
  if (entry.type === 'ws') return skip('cm does not support WebSocket servers');
  if (typeof entry.url === 'string') {
    if ('headersHelper' in entry) return skip('it uses headersHelper');
    const headers = entry.headers ?? {};
    if (!isStringRecord(headers)) return skip('its headers are malformed');
    const result = httpConfig(entry.url, headers);
    return 'skip' in result
      ? result
      : { ...result, knownOauth: isRecord(entry.oauth) ? true : undefined };
  }
  if (typeof entry.command !== 'string') {
    return skip('it has no command or url');
  }
  const args = entry.args ?? [];
  const env = entry.env ?? {};
  if (!isStringArray(args) || !isStringRecord(env)) {
    return skip('its args or env are malformed');
  }
  return {
    config: {
      kind: 'stdio',
      command: entry.command,
      args,
      env,
      cwd: undefined,
    },
  };
};

export const claude: Harness = {
  name: 'claude',

  async readServers({ home }) {
    const file = path.join(home, '.claude.json');
    const text = readIfExists(file);
    if (text === undefined) return { servers: [], skipped: [] };
    const data = parseJsonc(text);
    if (!isRecord(data)) return parseFailure('claude', file);
    const servers = isRecord(data.mcpServers) ? data.mcpServers : {};
    return collectServers('claude', Object.entries(servers), convert);
  },

  skillPath: ({ home }) =>
    path.join(home, '.claude', 'skills', 'cm', 'SKILL.md'),

  async allowCm({ home }) {
    const file = path.join(home, '.claude', 'settings.json');
    const { text, data } = loadJsonObject(file);
    const permissions = data.permissions;
    const allow = isRecord(permissions) ? permissions.allow : undefined;
    if (allow !== undefined && !Array.isArray(allow)) {
      throw new UsageError(`${file}: "permissions.allow" is not a list.`);
    }
    if (allow?.includes(ALLOW_RULE)) return undefined;
    if (allow === undefined) {
      saveJsonEdit(file, text, ['permissions', 'allow'], [ALLOW_RULE]);
    } else {
      saveJsonEdit(
        file,
        text,
        ['permissions', 'allow', allow.length],
        ALLOW_RULE,
        {
          insertInArray: true,
        },
      );
    }
    return { file, summary: `allowed ${ALLOW_RULE}` };
  },
};
