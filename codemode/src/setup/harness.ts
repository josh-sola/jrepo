import fs from 'node:fs';
import { configPath, loadConfig } from '../config.ts';
import { HARNESSES } from './harnesses/index.ts';
import type { HarnessName } from './harnesses/types.ts';
import { readIfExists, writeFile, type Change, type Dirs } from './files.ts';

const MAX_LISTED = 25;

export const configuredServers = (env: NodeJS.ProcessEnv): string[] =>
  fs.existsSync(configPath(env)) ? [...loadConfig(env).servers.keys()] : [];

// The description must not contain ": ", which plain YAML does not allow.
export const skillText = (servers: ReadonlyArray<string>): string => {
  const shown = servers.slice(0, MAX_LISTED).join(', ');
  const more =
    servers.length > MAX_LISTED
      ? ` and ${servers.length - MAX_LISTED} more`
      : '';
  const available =
    servers.length === 0 ? '' : ` Available servers are ${shown}${more}.`;
  return `---
name: cm
description: Call MCP server tools through the cm CLI.${available} Use it whenever a task needs one of them.
---

Run \`cm guide\` once before your first \`cm run\`. It explains how to find
tools and write a program. \`cm describe\` lists the servers.
`;
};

export const writeSkill = (
  file: string,
  servers: ReadonlyArray<string>,
): Change | undefined => {
  const text = skillText(servers);
  if (readIfExists(file) === text) return undefined;
  writeFile(file, text, { backup: false });
  return { file, summary: 'wrote the cm skill' };
};

/** Rewrites each installed cm skill so its server list stays current. */
export const refreshSkills = (
  dirs: Dirs,
  servers: ReadonlyArray<string>,
): Change[] => {
  const files = new Set(
    Object.values(HARNESSES).map((harness) => harness.skillPath(dirs)),
  );
  return [...files]
    .filter((file) => fs.existsSync(file))
    .flatMap((file) => writeSkill(file, servers) ?? []);
};

export const setupHarness = async (options: {
  readonly name: HarnessName;
  readonly env: NodeJS.ProcessEnv;
  readonly dirs: Dirs;
  readonly print: (text: string) => Promise<void>;
}): Promise<number> => {
  const { name, env, dirs, print } = options;
  const harness = HARNESSES[name];
  const changes = [
    writeSkill(harness.skillPath(dirs), configuredServers(env)),
    await harness.allowCm(dirs),
  ].flatMap((change) => change ?? []);
  await print(
    changes.length === 0
      ? `No change. cm is already set up for ${name}.`
      : changes.map(({ file, summary }) => `${file}: ${summary}.`).join('\n'),
  );
  return 0;
};
