import { graphql } from '@octokit/graphql';
import { Octokit } from '@octokit/rest';
import { createOctokitGitHubApi } from './api.ts';
import type { GitHubApi } from './api.ts';

// Reads the OAuth token `gh` already holds, so panopticon needs no
// credentials of its own. Boot code calls this once; wrappers take the
// token or the built clients as constructor parameters instead of reading
// it again.
export async function readGhToken(): Promise<string> {
  const proc = Bun.spawn(['gh', 'auth', 'token'], {
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(`gh auth token failed: ${stderr.trim()}`);
  }
  const token = stdout.trim();
  if (!token) {
    throw new Error('gh auth token returned an empty token');
  }
  return token;
}

export function createGitHubApi(token: string): GitHubApi {
  const rest = new Octokit({ auth: token });
  const graphqlClient = graphql.defaults({
    headers: { authorization: `token ${token}` },
  });
  return createOctokitGitHubApi(rest, graphqlClient);
}
