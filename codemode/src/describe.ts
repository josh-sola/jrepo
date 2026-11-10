import { CodeMode } from '@opencode/codemode';
import { UsageError } from './errors.ts';
import type { ToolTree } from './tools.ts';

const MAX_SUMMARY = 100;
// About 1k tokens. Past this an agent should list names first, then describe
// only the tools it needs.
const MAX_FULL_LISTING = 4096;

const summary = (description: string): string => {
  const line = description.trim().split('\n')[0]?.trim() ?? '';
  const sentence = /^.*?[.!?](?=\s|$)/.exec(line)?.[0] ?? line;
  return sentence.length > MAX_SUMMARY
    ? `${sentence.slice(0, MAX_SUMMARY - 1).trimEnd()}…`
    : sentence;
};

const toolCount = (count: number) =>
  `${count} ${count === 1 ? 'tool' : 'tools'}`;

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

export const describeServer = (tree: ToolTree, server: string): string => {
  const catalog = catalogOf({ [server]: tree[server] ?? {} });
  const full = [
    `${server}: ${toolCount(catalog.length)}.`,
    ...catalog.map(
      (tool) => `// ${summary(tool.description)}\n${tool.signature}`,
    ),
  ].join('\n\n');
  if (Buffer.byteLength(full) <= MAX_FULL_LISTING) return full;
  const rows = catalog.map((tool) => ({
    name: tool.path.slice(server.length + 1),
    text: summary(tool.description),
  }));
  const width = Math.max(...rows.map((row) => row.name.length)) + 2;
  return [
    `${server}: ${toolCount(catalog.length)}. Run "cm describe ${server}.<tool>" for one tool's signature.`,
    ...rows.map((row) => `${row.name.padEnd(width)}${row.text}`.trimEnd()),
  ].join('\n');
};

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
