import { randomUUID } from "node:crypto";
import { mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, join } from "node:path";

import { positiveInteger, type PlanterRecord, type StateEnvironment } from "./state.ts";

export function planterStateDirectory(
  environment: StateEnvironment = process.env,
  home = homedir(),
): string {
  if (typeof environment.PLANTER_STATE_DIR === "string" && environment.PLANTER_STATE_DIR.length > 0) {
    return environment.PLANTER_STATE_DIR;
  }
  if (typeof environment.CLAUDE_PLANTER_DIR === "string" && environment.CLAUDE_PLANTER_DIR.length > 0) {
    return environment.CLAUDE_PLANTER_DIR;
  }
  return join(home, ".claude", "planter");
}

export function safeSessionId(sessionId: string): string {
  return sessionId.replace(/[^A-Za-z0-9._-]/gu, "_");
}

export function planterRecordPath(directory: string, sessionId: string, pid: number): string {
  return join(directory, `pi-${safeSessionId(sessionId)}-${pid}.json`);
}

async function readObject(path: string): Promise<Record<string, unknown> | undefined> {
  try {
    const parsed: unknown = JSON.parse(await readFile(path, "utf8"));
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : undefined;
  } catch {
    return undefined;
  }
}

export class PlanterStore {
  readonly path: string;
  readonly directory: string;
  private queue: Promise<void> = Promise.resolve();
  private readonly pid: number;
  private readonly createTempId: () => string;

  constructor(
    directory: string,
    sessionId: string,
    pid: number,
    createTempId: () => string = randomUUID,
  ) {
    this.directory = directory;
    this.pid = pid;
    this.createTempId = createTempId;
    this.path = planterRecordPath(directory, sessionId, pid);
  }

  read(): Promise<Record<string, unknown> | undefined> {
    return readObject(this.path);
  }

  write(record: PlanterRecord): Promise<PlanterRecord | undefined> {
    let result: PlanterRecord | undefined;
    const operation = this.queue.then(async () => {
      try {
        result = await this.writeNow({ ...record });
      } catch {
        result = undefined;
      }
    });
    this.queue = operation;
    return operation.then(() => result);
  }

  remove(): Promise<void> {
    const operation = this.queue.then(async () => {
      try {
        await rm(this.path, { force: true });
      } catch {
        // State reporting must not disrupt Pi shutdown.
      }
    });
    this.queue = operation;
    return operation;
  }

  flush(): Promise<void> {
    return this.queue;
  }

  private async writeNow(record: PlanterRecord): Promise<PlanterRecord> {
    await mkdir(this.directory, { recursive: true });
    const disk = await readObject(this.path);
    const diskTab = positiveInteger(disk?.tab);
    if (diskTab !== undefined) record.tab = diskTab;

    const temp = join(
      dirname(this.path),
      `.${basename(this.path)}.${this.pid}.${this.createTempId()}.tmp`,
    );
    try {
      await writeFile(temp, `${JSON.stringify(record)}\n`, { encoding: "utf8", flag: "wx" });
      await rename(temp, this.path);
      return record;
    } finally {
      await rm(temp, { force: true }).catch(() => {});
    }
  }
}
