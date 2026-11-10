import type { ServerConfig } from '../../config.ts';
import type { Change, Dirs } from '../files.ts';

export const HARNESS_NAMES = ['claude', 'codex', 'opencode'] as const;
export type HarnessName = (typeof HARNESS_NAMES)[number];

export type FoundServer = {
  readonly harness: HarnessName;
  readonly name: string;
  readonly config: ServerConfig;
  /** `true` when the harness says the server uses OAuth, `false` when it says it does not. */
  readonly knownOauth: boolean | undefined;
  readonly notes: ReadonlyArray<string>;
};

export type Found = {
  readonly servers: ReadonlyArray<FoundServer>;
  /** One plain-English line for each server or file that was left out. */
  readonly skipped: ReadonlyArray<string>;
};

export type Harness = {
  readonly name: HarnessName;
  readServers(dirs: Dirs): Promise<Found>;
  skillPath(dirs: Dirs): string;
  /** Adds the rule that lets the agent run `cm`. Returns `undefined` when it is already there. */
  allowCm(dirs: Dirs): Promise<Change | undefined>;
};
