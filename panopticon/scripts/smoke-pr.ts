#!/usr/bin/env bun
// Live smoke test for the GitHub and git layer against a real PR. Reads
// with `gh`'s own token and clones into a scratch data dir; it never
// writes to GitHub or to panopticon's real data directory.
//
// Usage: bun scripts/smoke-pr.ts <owner> <repo> <number>
// The clone's data dir defaults to ./.smoke-data; override it with
// PANOPTICON_SMOKE_DATA_DIR (used for the recorded smoke run in the report).

import { join } from 'node:path';
import { expandHome } from '../src/server/config.ts';
import { RepoStores } from '../src/server/git/stores.ts';
import { createGitHubApi, readGhToken } from '../src/server/github/client.ts';
import { loadPr } from '../src/server/routes/pr.ts';

async function main(): Promise<void> {
  const [owner, repo, numberArg] = process.argv.slice(2);
  if (!owner || !repo || !numberArg) {
    console.error('usage: bun scripts/smoke-pr.ts <owner> <repo> <number>');
    process.exitCode = 1;
    return;
  }
  const number = Number(numberArg);
  if (!Number.isInteger(number) || number <= 0) {
    console.error(`"${numberArg}" is not a positive PR number`);
    process.exitCode = 1;
    return;
  }

  const token = await readGhToken();
  const github = createGitHubApi(token);

  const dataDir = process.env.PANOPTICON_SMOKE_DATA_DIR
    ? expandHome(process.env.PANOPTICON_SMOKE_DATA_DIR)
    : join(process.cwd(), '.smoke-data');
  const referenceClone = process.env.PANOPTICON_SMOKE_REFERENCE_CLONE
    ? expandHome(process.env.PANOPTICON_SMOKE_REFERENCE_CLONE)
    : null;
  const trunk = process.env.PANOPTICON_SMOKE_TRUNK ?? 'master';
  const stores = new RepoStores(dataDir, {
    [`${owner}/${repo}`]: {
      trunk,
      ...(referenceClone ? { referenceClone } : {}),
    },
  });

  const [{ pr, files }, threads] = await Promise.all([
    loadPr({ github, stores, trunkFor: () => trunk }, owner, repo, number),
    github.listReviewThreads(owner, repo, number),
  ]);

  console.log(`#${pr.number} ${pr.title}`);
  console.log(`state: ${pr.state}`);
  console.log(`files: ${files.length}`);
  console.log(`threads: ${threads.length}`);

  const firstThread = threads[0];
  if (firstThread) {
    console.log(
      `first thread: ${firstThread.path}:${firstThread.line ?? firstThread.originalLine}`,
    );
  } else {
    console.log('first thread: (none)');
  }

  const firstFile = files[0];
  if (firstFile) {
    console.log(
      `first file: ${firstFile.path} oldOid=${firstFile.oldOid ?? '(none)'} newOid=${firstFile.newOid ?? '(none)'}`,
    );
  } else {
    console.log('first file: (none)');
  }
}

await main();
