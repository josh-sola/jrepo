import type { PullRequestMetadata, RepositoryMetadata } from "./data.ts";

const ESC = "\x1b[";
const RESET = `${ESC}0m`;
const BOLD = `${ESC}1m`;
const DIM = `${ESC}2m`;
const UNDERLINE = `${ESC}4m`;
const UNDERLINE_OFF = `${ESC}24m`;
const SEPARATOR = `${DIM} · ${RESET}`;

export type WidthHelpers = {
  visibleWidth(text: string): number;
  truncateToWidth(text: string, width: number, ellipsis?: string): string;
};

export type FooterSnapshot = {
  modelName: string;
  contextPercent?: number;
  repository: RepositoryMetadata;
  statuses: ReadonlyMap<string, string>;
};

function color(code: number, text: string): string {
  return `${ESC}38;5;${code}m${text}${RESET}`;
}

function contextColor(percent: number): number {
  if (percent >= 80) return 174;
  if (percent >= 50) return 179;
  return 108;
}

function joinSegments(segments: Array<string | undefined>): string | undefined {
  const present = segments.filter((segment): segment is string => Boolean(segment));
  return present.length > 0 ? present.join(SEPARATOR) : undefined;
}

export function sanitizeStatusText(text: string): string {
  return text.replace(/[\r\n\t]/g, " ").replace(/ +/g, " ").trim();
}

export function formatStatuses(statuses: ReadonlyMap<string, string>): string | undefined {
  const values = [...statuses.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([, text]) => sanitizeStatusText(text))
    .filter(Boolean);
  return values.length > 0 ? values.join(" ") : undefined;
}

export function formatPullRequest(pullRequest: PullRequestMetadata): string {
  const label = pullRequest.draft
    ? `${DIM}#${pullRequest.number} draft${RESET}`
    : `#${pullRequest.number}`;
  if (!pullRequest.url) return label;
  const open = `\x1b]8;;${pullRequest.url}\x1b\\`;
  const close = "\x1b]8;;\x1b\\";
  return `${open}${UNDERLINE}${label}${UNDERLINE_OFF}${close}`;
}

export function formatModelLine(modelName: string, contextPercent?: number): string {
  const model = `${BOLD}${color(139, modelName)}${RESET}`;
  if (contextPercent === undefined || !Number.isFinite(contextPercent)) return model;
  const rounded = Math.round(contextPercent);
  return joinSegments([model, color(contextColor(rounded), `ctx ${rounded}%`)])!;
}

export function formatRepositoryLine(repository: RepositoryMetadata): string | undefined {
  return joinSegments([
    repository.repo ? `${BOLD}${color(109, repository.repo)}${RESET}` : undefined,
    repository.worktree ? color(repository.worktree.color, repository.worktree.name) : undefined,
    repository.branch,
    repository.pullRequest ? formatPullRequest(repository.pullRequest) : undefined,
    repository.stack ? `${DIM}stack ${repository.stack.position}/${repository.stack.total}${RESET}` : undefined,
  ]);
}

function fitLine(line: string, width: number, helpers: WidthHelpers): string {
  const safeWidth = Math.max(0, Math.floor(width));
  if (helpers.visibleWidth(line) <= safeWidth) return line;
  return helpers.truncateToWidth(line, safeWidth, `${DIM}...${RESET}`);
}

export function renderFooter(snapshot: FooterSnapshot, width: number, helpers: WidthHelpers): string[] {
  const lines = [formatModelLine(snapshot.modelName, snapshot.contextPercent)];
  const repositoryLine = formatRepositoryLine(snapshot.repository);
  if (repositoryLine) lines.push(repositoryLine);
  const statusLine = formatStatuses(snapshot.statuses);
  if (statusLine) lines.push(statusLine);
  return lines.map((line) => fitLine(line, width, helpers));
}
