import type { StatusLineConfig } from "./config.ts";
import { CUSTOM_COMPONENT_PREFIX, customOccurrenceKey } from "./custom.ts";
import type { PullRequestMetadata, RepositoryMetadata } from "./data.ts";
import type { JpiComponentId } from "./layout.ts";

const ESC = "\x1b[";
const RESET = `${ESC}0m`;
const BOLD = `${ESC}1m`;
const DIM = `${ESC}2m`;
const UNDERLINE = `${ESC}4m`;
const UNDERLINE_OFF = `${ESC}24m`;
const SEPARATOR = `${DIM} · ${RESET}`;
const LINE_PREFIX = " ";

export type WidthHelpers = {
  visibleWidth(text: string): number;
  truncateToWidth(text: string, width: number, ellipsis?: string): string;
};

export type FooterSnapshot = {
  modelName: string;
  contextPercent?: number;
  repository: RepositoryMetadata;
  statuses: ReadonlyMap<string, string>;
  customOutputs?: ReadonlyMap<string, string>;
  config: StatusLineConfig;
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

function formatStatusSegments(
  statuses: ReadonlyMap<string, string>,
  disabledStatuses: ReadonlySet<string>,
): string[] {
  return [...statuses.entries()]
    .filter(([key]) => !disabledStatuses.has(key))
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([, text]) => sanitizeStatusText(text))
    .filter(Boolean);
}

export function formatStatuses(
  statuses: ReadonlyMap<string, string>,
  disabledStatuses: ReadonlySet<string> = new Set(),
): string | undefined {
  return joinSegments(formatStatusSegments(statuses, disabledStatuses));
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

function formatModel(modelName: string): string {
  return `${BOLD}${color(139, modelName)}${RESET}`;
}

function formatContext(contextPercent?: number): string | undefined {
  if (contextPercent === undefined || !Number.isFinite(contextPercent)) return undefined;
  const rounded = Math.round(contextPercent);
  return color(contextColor(rounded), `ctx ${rounded}%`);
}

export function formatModelLine(modelName: string, contextPercent?: number): string {
  return joinSegments([formatModel(modelName), formatContext(contextPercent)])!;
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

function formatLocalComponent(
  componentId: Exclude<JpiComponentId, "@jpi/slot">,
  snapshot: FooterSnapshot,
): string | undefined {
  const repository = snapshot.repository;
  switch (componentId) {
    case "@jpi/model":
      return formatModel(snapshot.modelName);
    case "@jpi/context":
      return formatContext(snapshot.contextPercent);
    case "@jpi/repository":
      return repository.repo ? `${BOLD}${color(109, repository.repo)}${RESET}` : undefined;
    case "@jpi/worktree":
      return repository.worktree
        ? color(repository.worktree.color, repository.worktree.name)
        : undefined;
    case "@jpi/branch":
      return repository.branch;
    case "@jpi/pull-request":
      return repository.pullRequest ? formatPullRequest(repository.pullRequest) : undefined;
    case "@jpi/stack":
      return repository.stack
        ? `${DIM}stack ${repository.stack.position}/${repository.stack.total}${RESET}`
        : undefined;
  }
}

function resolveComponent(
  componentId: string,
  lineIndex: number,
  componentIndex: number,
  snapshot: FooterSnapshot,
): string[] {
  if (componentId.startsWith(CUSTOM_COMPONENT_PREFIX)) {
    const value = snapshot.customOutputs?.get(customOccurrenceKey(lineIndex, componentIndex));
    if (value === undefined) return [];
    const formatted = sanitizeStatusText(value);
    return formatted ? [formatted] : [];
  }
  if (componentId === "@jpi/slot") {
    return formatStatusSegments(snapshot.statuses, snapshot.config.disabledStatuses);
  }
  if (componentId.startsWith("@jpi/")) {
    const value = formatLocalComponent(componentId as Exclude<JpiComponentId, "@jpi/slot">, snapshot);
    return value ? [value] : [];
  }

  const value = snapshot.statuses.get(componentId);
  if (value === undefined) return [];
  const formatted = sanitizeStatusText(value);
  return formatted ? [formatted] : [];
}

function fitLine(line: string, width: number, helpers: WidthHelpers): string | undefined {
  const safeWidth = Math.max(0, Math.floor(width));
  const prefixWidth = helpers.visibleWidth(LINE_PREFIX);
  if (safeWidth < prefixWidth) return undefined;
  const contentWidth = safeWidth - prefixWidth;
  const content = helpers.visibleWidth(line) <= contentWidth
    ? line
    : helpers.truncateToWidth(line, contentWidth, `${DIM}...${RESET}`);
  return `${LINE_PREFIX}${content}`;
}

export function renderFooter(snapshot: FooterSnapshot, width: number, helpers: WidthHelpers): string[] {
  const lines: string[] = [];
  for (let lineIndex = 0; lineIndex < snapshot.config.format.length; lineIndex += 1) {
    const configuredLine = snapshot.config.format[lineIndex]!;
    const segments = configuredLine.flatMap((componentId, componentIndex) => (
      resolveComponent(componentId, lineIndex, componentIndex, snapshot)
    ));
    const line = joinSegments(segments);
    if (!line) continue;
    const fittedLine = fitLine(line, width, helpers);
    if (fittedLine !== undefined) lines.push(fittedLine);
  }
  return lines;
}
