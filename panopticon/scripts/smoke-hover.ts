#!/usr/bin/env bun
// Live smoke test for hover: provisions a real wt tree for an open PR,
// starts the right language server, prints one hover result, then tears
// the tree down and confirms wt has forgotten it. Provisioning is heavy
// (pnpm install or uv sync), so this waits up to 20 minutes.
//
// Usage: bun scripts/smoke-hover.ts <owner> <repo> <number> <path> <line> <character>
// `line` and `character` are 0-based, UTF-16 code units, on the PR's head
// side (the same contract the hover route expects).
//
// Env overrides:
//   PANOPTICON_SMOKE_DATA_DIR        default ./.smoke-data
//   PANOPTICON_SMOKE_WT_REPO         default <repo>
//   PANOPTICON_SMOKE_REFERENCE_CLONE default ~/repos/<repo>

import { join } from 'node:path';
import { expandHome } from '../src/server/config.ts';
import { openDb } from '../src/server/db.ts';
import { createGitHubApi, readGhToken } from '../src/server/github/client.ts';
import {
  createHoverServers,
  languageForPath,
} from '../src/server/hover/servers.ts';
import { TreeManager } from '../src/server/hover/trees.ts';
import { BunWtRunner } from '../src/server/hover/wt.ts';

const POLL_INTERVAL_MS = 10_000;
const MAX_WAIT_MS = 20 * 60 * 1000;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main(): Promise<void> {
  const [owner, repo, numberArg, path, lineArg, characterArg] =
    process.argv.slice(2);
  if (
    !owner ||
    !repo ||
    !numberArg ||
    !path ||
    lineArg === undefined ||
    characterArg === undefined
  ) {
    console.error(
      'usage: bun scripts/smoke-hover.ts <owner> <repo> <number> <path> <line> <character>',
    );
    process.exitCode = 1;
    return;
  }
  const number = Number(numberArg);
  const line = Number(lineArg);
  const character = Number(characterArg);
  if (!Number.isInteger(number) || number <= 0) {
    console.error(`"${numberArg}" is not a positive PR number`);
    process.exitCode = 1;
    return;
  }

  const language = languageForPath(path);
  if (language === 'unsupported') {
    console.error(`"${path}" has no configured hover language`);
    process.exitCode = 1;
    return;
  }

  const dataDir = process.env.PANOPTICON_SMOKE_DATA_DIR
    ? expandHome(process.env.PANOPTICON_SMOKE_DATA_DIR)
    : join(process.cwd(), '.smoke-data');
  const wtRepo = process.env.PANOPTICON_SMOKE_WT_REPO ?? repo;
  const referenceClone = process.env.PANOPTICON_SMOKE_REFERENCE_CLONE
    ? expandHome(process.env.PANOPTICON_SMOKE_REFERENCE_CLONE)
    : expandHome(`~/repos/${repo}`);

  const db = openDb(join(dataDir, 'smoke-hover.sqlite'));
  const runner = new BunWtRunner();
  const servers = createHoverServers();
  const trees = new TreeManager({
    db,
    runner,
    repos: {
      [`${owner}/${repo}`]: { wtRepo, referenceClone, trunk: 'master' },
    },
    stopServers: (o, r, n) => servers.stopAll({ owner: o, repo: r, number: n }),
  });

  const github = createGitHubApi(await readGhToken());
  const pr = await github.getPull(owner, repo, number);
  const fileList = await github.listPullFiles(owner, repo, number);

  console.log(`#${pr.number} ${pr.title}`);
  console.log(`head: ${pr.head.sha}`);

  await trees.ensureTree({
    owner,
    repo,
    number,
    headSha: pr.head.sha,
    languages: [language],
    changedPaths: fileList.files.map((file) => file.path),
  });

  const start = Date.now();
  let treeState = await trees.status(owner, repo, number);
  while (treeState.state === 'provisioning' || treeState.state === 'none') {
    const elapsed = Math.round((Date.now() - start) / 1000);
    if (Date.now() - start > MAX_WAIT_MS) {
      throw new Error(
        `tree did not become ready within ${MAX_WAIT_MS / 1000}s (last state: ${treeState.state})`,
      );
    }
    console.log(
      `waiting for tree... (${treeState.state}, ${elapsed}s elapsed)`,
    );
    await sleep(POLL_INTERVAL_MS);
    treeState = await trees.status(owner, repo, number);
  }
  if (treeState.state !== 'ready') {
    throw new Error(
      `tree ended in unexpected state: ${JSON.stringify(treeState)}`,
    );
  }
  const treePath = treeState.path;
  console.log(
    `tree ready at ${treePath} (${Math.round((Date.now() - start) / 1000)}s)`,
  );

  const prKey = { owner, repo, number };
  await servers.ensureStarted(prKey, language, treePath, path);

  const serverStart = Date.now();
  let serverState = servers.status(prKey)[language];
  while (serverState === 'starting') {
    if (Date.now() - serverStart > MAX_WAIT_MS) {
      throw new Error(
        `${language} server did not become ready within ${MAX_WAIT_MS / 1000}s`,
      );
    }
    await sleep(POLL_INTERVAL_MS);
    serverState = servers.status(prKey)[language];
  }
  if (serverState !== 'ready') {
    throw new Error(`${language} server ended in state: ${serverState}`);
  }
  console.log(`${language} server ready`);

  const hover = await servers.hover(prKey, language, path, line, character);
  console.log('--- hover result ---');
  console.log(hover ?? '(no hover contents)');
  console.log('--------------------');

  await trees.teardown(owner, repo, number);

  const remaining = await runner.run(['tree', 'ls', '--json']);
  const stillListed = remaining.stdout.includes(`panopticon-pr-${number}`);
  console.log(
    `teardown confirmed: tree ${stillListed ? 'STILL LISTED (unexpected)' : 'no longer listed'}`,
  );
}

await main();
