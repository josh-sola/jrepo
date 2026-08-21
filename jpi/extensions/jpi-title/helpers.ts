import { basename } from "node:path";

export const IDLE_INDICATOR = "⏹";
export const ACTIVE_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"] as const;
export const SPINNER_INTERVAL_MS = 80;

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

export function sanitizeTitlePart(value: unknown): string {
  if (typeof value !== "string") return "";
  return value.replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ").trim();
}

export function sessionIndicator(
  sessionName: unknown,
  worktreeName: unknown,
  cwd: string,
): string {
  return sanitizeTitlePart(sessionName)
    || sanitizeTitlePart(worktreeName)
    || sanitizeTitlePart(basename(cwd));
}

export async function loadWorktreeName(
  exec: ExecCommand,
  cwd: string,
  signal?: AbortSignal,
): Promise<string | undefined> {
  try {
    const options: { cwd: string; timeout: number; signal?: AbortSignal } = {
      cwd,
      timeout: 3_000,
    };
    if (signal) options.signal = signal;
    const result = await exec("wt", ["tree", "name", "--path", cwd], options);
    if (result.code !== 0 || result.killed) return undefined;
    return sanitizeTitlePart(result.stdout) || undefined;
  } catch {
    return undefined;
  }
}
