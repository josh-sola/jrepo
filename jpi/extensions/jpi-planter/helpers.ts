import { basename, join } from "node:path";

import type { StateEnvironment } from "./state.ts";

export const LABEL_HOOK_TIMEOUT_MS = 3_000;
export const MAX_LABEL_LENGTH = 512;

export type ExecResult = {
  stdout?: string;
  code?: number | null;
  killed?: boolean;
};

export type ExecCommand = (
  command: string,
  args: string[],
  options: { cwd: string; timeout: number; signal?: AbortSignal },
) => Promise<ExecResult>;

type LabelOptions = {
  environment: StateEnvironment;
  getSessionName(): string | undefined;
  exec: ExecCommand;
  stateDirectory: string;
  cwd: string;
  signal?: AbortSignal;
};

export function sanitizeLabelLine(value: unknown): string {
  if (typeof value !== "string") return "";
  const firstLine = value.split(/[\r\n]/u, 1)[0] ?? "";
  return firstLine
    .replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ")
    .trim()
    .slice(0, MAX_LABEL_LENGTH);
}

export async function resolvePlanterLabel(options: LabelOptions): Promise<string> {
  const environmentLabel = options.environment.PLANTER_LABEL;
  if (typeof environmentLabel === "string" && environmentLabel.length > 0) {
    const label = sanitizeLabelLine(environmentLabel);
    if (label) return label;
  }

  const sessionName = options.getSessionName();
  if (typeof sessionName === "string" && sessionName.length > 0) {
    const label = sanitizeLabelLine(sessionName);
    if (label) return label;
  }

  try {
    const execOptions: { cwd: string; timeout: number; signal?: AbortSignal } = {
      cwd: options.cwd,
      timeout: LABEL_HOOK_TIMEOUT_MS,
    };
    if (options.signal) execOptions.signal = options.signal;
    const result = await options.exec(
      join(options.stateDirectory, "label-hook"),
      [options.cwd],
      execOptions,
    );
    if (result.code === 0 && !result.killed) {
      const label = sanitizeLabelLine(result.stdout);
      if (label) return label;
    }
  } catch {}
  return sanitizeLabelLine(basename(options.cwd));
}
