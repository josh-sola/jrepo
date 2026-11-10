import type { ServerConfig } from '../../config.ts';
import { isRecord } from '../../env.ts';
import type { Found, FoundServer, HarnessName } from './types.ts';

export type Converted =
  | {
      readonly config: ServerConfig;
      readonly knownOauth?: boolean | undefined;
    }
  | { readonly skip: string };

export const skip = (reason: string): Converted => ({ skip: reason });

export const isStringRecord = (
  value: unknown,
): value is Record<string, string> =>
  isRecord(value) && Object.values(value).every((v) => typeof v === 'string');

export const isStringArray = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((v) => typeof v === 'string');

export const httpConfig = (
  url: string,
  headers: Record<string, string>,
): Converted => {
  const parsed = URL.parse(url);
  if (parsed === null || !/^https?:$/.test(parsed.protocol)) {
    return skip(`"${url}" is not an http or https URL`);
  }
  return { config: { kind: 'http', url: parsed, headers, oauth: false } };
};

const VARIABLE = /\$\{([A-Za-z_][A-Za-z0-9_]*)\}/g;
const WITH_DEFAULT = /\$\{[^}]*:-/;

const stringsOf = (config: ServerConfig): string[] =>
  config.kind === 'stdio'
    ? [
        config.command,
        ...config.args,
        ...Object.values(config.env),
        ...(config.cwd === undefined ? [] : [config.cwd]),
      ]
    : [config.url.href, ...Object.values(config.headers)];

/**
 * Runs `convert` on each entry. A server that cannot be converted, or that
 * uses a `${VAR:-default}` value that cm cannot expand, is skipped with a reason.
 */
export const collectServers = (
  harness: HarnessName,
  entries: ReadonlyArray<readonly [string, unknown]>,
  convert: (entry: unknown) => Converted,
): Found => {
  const servers: FoundServer[] = [];
  const skipped: string[] = [];
  const left = (name: string, reason: string) =>
    skipped.push(`${harness}: skipped "${name}" because ${reason}.`);
  for (const [name, entry] of entries) {
    const converted = convert(entry);
    if ('skip' in converted) {
      left(name, converted.skip);
      continue;
    }
    const texts = stringsOf(converted.config);
    if (texts.some((text) => WITH_DEFAULT.test(text))) {
      left(name, 'it uses a ${VAR:-default} value, which cm does not support');
      continue;
    }
    const variables = new Set(
      texts.flatMap((text) => [...text.matchAll(VARIABLE)].map((m) => m[1])),
    );
    servers.push({
      harness,
      name,
      config: converted.config,
      knownOauth: converted.knownOauth,
      notes:
        variables.size === 0
          ? []
          : [`needs ${[...variables].join(', ')} set when you run cm`],
    });
  }
  return { servers, skipped };
};

export const parseFailure = (harness: HarnessName, file: string): Found => ({
  servers: [],
  skipped: [`${harness}: skipped ${file} because it could not be parsed.`],
});
