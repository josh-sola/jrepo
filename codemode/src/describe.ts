import { CodeMode } from '@opencode/codemode';
import { UsageError } from './errors.ts';
import type { ToolTree } from './tools.ts';

const MAX_SUMMARY = 120;

const firstLine = (text: string): string => {
  const line = text.split('\n')[0]?.trim() ?? '';
  return line.length > MAX_SUMMARY
    ? `${line.slice(0, MAX_SUMMARY - 1).trimEnd()}…`
    : line;
};

const catalogOf = (tree: ToolTree) => CodeMode.make({ tools: tree }).catalog;

export const describeServers = (
  tree: ToolTree,
  failures: ReadonlyMap<string, string> = new Map(),
): string => {
  const lines = Object.entries(tree).map(
    ([server, tools]) => `${server} (${Object.keys(tools).length} tools)`,
  );
  for (const [server, message] of failures) {
    lines.push(`${server} (unavailable)\n  ${message}`);
  }
  return lines.join('\n');
};

export const describeServer = (tree: ToolTree, server: string): string =>
  catalogOf({ [server]: tree[server] ?? {} })
    .map((tool) => `// ${firstLine(tool.description)}\n${tool.signature}`)
    .join('\n\n');

export const describeTool = (
  tree: ToolTree,
  server: string,
  tool: string,
): string => {
  const entry = catalogOf({ [server]: tree[server] ?? {} }).find(
    (candidate) => candidate.path === `${server}.${tool}`,
  );
  if (entry === undefined) {
    const names = Object.keys(tree[server] ?? {}).join(', ');
    throw new UsageError(
      `Server "${server}" has no tool named "${tool}". Its tools: ${names}.`,
    );
  }
  const description = entry.description.trim();
  return description === ''
    ? entry.signature
    : `${description}\n\n${entry.signature}`;
};
