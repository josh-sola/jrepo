import { Hono } from 'hono';
import type {
  HoverLanguage,
  HoverResponse,
  HoverStatusResponse,
  ServerState,
} from '../../shared/hover.ts';
import type { PrFile, PrSummary } from '../../shared/github.ts';
import type { HoverServers } from '../hover/servers.ts';
import { languageForPath } from '../hover/servers.ts';
import type { TreeManager } from '../hover/trees.ts';

export interface HoverRouterDeps {
  trees: TreeManager;
  servers: HoverServers;
  loadPr(
    owner: string,
    repo: string,
    number: number,
  ): Promise<{ pr: PrSummary; files: PrFile[] }>;
}

// Mounted under `/api/pr/:owner/:repo/:number/hover`, so the params exist
// at runtime but this sub-router's types do not declare them.
function requireParam(value: string | undefined, name: string): string {
  if (value === undefined) {
    throw new Error(`hoverRouter: missing route param "${name}"`);
  }
  return value;
}

interface HoverQuery {
  path: string;
  line: number;
  character: number;
}

function parseHoverQuery(
  path: string | undefined,
  line: string | undefined,
  character: string | undefined,
): HoverQuery | null {
  if (path === undefined || path.length === 0) return null;
  if (line === undefined || !/^\d+$/.test(line)) return null;
  if (character === undefined || !/^\d+$/.test(character)) return null;
  return { path, line: Number(line), character: Number(character) };
}

function languagesInDiff(files: PrFile[]): HoverLanguage[] {
  const languages = new Set<HoverLanguage>();
  for (const file of files) {
    const language = languageForPath(file.path);
    if (language !== 'unsupported') languages.add(language);
  }
  return [...languages];
}

export function hoverRouter(deps: HoverRouterDeps): Hono {
  return new Hono()
    .get('/status', async (c) => {
      const owner = requireParam(c.req.param('owner'), 'owner');
      const repo = requireParam(c.req.param('repo'), 'repo');
      const number = Number(requireParam(c.req.param('number'), 'number'));

      const treeState = await deps.trees.status(owner, repo, number);
      const servers = deps.servers.status({ owner, repo, number });

      const body: HoverStatusResponse = {
        tree: treeState.state,
        treeError: treeState.state === 'failed' ? treeState.error : null,
        servers,
      };
      return c.json(body);
    })
    .get('/', async (c) => {
      const owner = requireParam(c.req.param('owner'), 'owner');
      const repo = requireParam(c.req.param('repo'), 'repo');
      const number = Number(requireParam(c.req.param('number'), 'number'));

      const query = parseHoverQuery(
        c.req.query('path'),
        c.req.query('line'),
        c.req.query('character'),
      );
      if (query === null) {
        return c.json({ error: 'expected ?path=&line=&character=' }, 400);
      }

      const language = languageForPath(query.path);
      if (language === 'unsupported') {
        const body: HoverResponse = { status: 'unsupported' };
        return c.json(body);
      }

      const pr = { owner, repo, number };
      const treeState = await deps.trees.status(owner, repo, number);

      if (treeState.state === 'unsupported') {
        const body: HoverResponse = { status: 'unsupported' };
        return c.json(body);
      }

      if (treeState.state === 'none') {
        const { pr: prSummary, files } = await deps.loadPr(owner, repo, number);
        void deps.trees
          .ensureTree({
            owner,
            repo,
            number,
            headSha: prSummary.head.sha,
            languages: languagesInDiff(files),
            changedPaths: files.map((file) => file.path),
          })
          .catch(() => {});
        const body: HoverResponse = {
          status: 'preparing',
          tree: 'provisioning',
          server: 'stopped',
        };
        return c.json(body);
      }

      if (treeState.state === 'provisioning' || treeState.state === 'failed') {
        const server = deps.servers.status(pr)[language];
        const body: HoverResponse = {
          status: 'preparing',
          tree: treeState.state,
          server,
        };
        return c.json(body);
      }

      deps.trees.touch(owner, repo, number);
      const serverState: ServerState = deps.servers.status(pr)[language];

      if (serverState !== 'ready') {
        void deps.servers
          .ensureStarted(pr, language, treeState.path, query.path)
          .catch(() => {});
        const body: HoverResponse = {
          status: 'preparing',
          tree: 'ready',
          server: deps.servers.status(pr)[language],
        };
        return c.json(body);
      }

      const contents = await deps.servers.hover(
        pr,
        language,
        query.path,
        query.line,
        query.character,
      );
      const body: HoverResponse = { status: 'ready', contents };
      return c.json(body);
    });
}
