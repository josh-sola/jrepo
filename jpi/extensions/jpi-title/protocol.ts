export const BACKGROUND_REQUEST_CHANNEL = "pi-background-tasks:request:v1";
export const BACKGROUND_RESPONSE_CHANNEL = "pi-background-tasks:response:v1";
export const BACKGROUND_TERMINAL_CHANNEL = "pi-background-tasks:terminal:v1";
export const BACKGROUND_REQUEST_SCHEMA = "pi-background-tasks.extension-request.v1";
export const BACKGROUND_RESPONSE_SCHEMA = "pi-background-tasks.extension-response.v1";
export const BACKGROUND_POLL_INTERVAL_MS = 1_000;
export const BACKGROUND_RESPONSE_TIMEOUT_MS = 3_000;

export type EventBus = {
  emit(channel: string, data: unknown): void;
  on(channel: string, handler: (data: unknown) => void): () => void;
};

export type Scheduler = {
  setInterval(callback: () => void, delay: number): unknown;
  clearInterval(timer: unknown): void;
  setTimeout(callback: () => void, delay: number): unknown;
  clearTimeout(timer: unknown): void;
};

type RecordValue = Record<string, unknown>;

function isRecord(value: unknown): value is RecordValue {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function runningStatusResponse(data: unknown, requestId: string): boolean | undefined {
  if (
    !isRecord(data)
    || data.schema_version !== BACKGROUND_RESPONSE_SCHEMA
    || data.request_id !== requestId
    || data.operation !== "status"
    || data.ok !== true
    || !isRecord(data.result)
    || !Array.isArray(data.result.tasks)
  ) return undefined;

  return data.result.tasks.some((task) => isRecord(task) && task.status === "running");
}
