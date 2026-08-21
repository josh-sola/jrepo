import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

import { CUSTOM_COMPONENT_PREFIX, isCustomComponentId } from "./custom.ts";
import {
  DEFAULT_STATUS_LINE_FORMAT,
  isJpiComponentId,
  type StatusLineFormat,
} from "./layout.ts";

export type StatusLineConfig = {
  format: StatusLineFormat;
  disabledStatuses: ReadonlySet<string>;
};

export type StatusLineConfigResult = {
  config: StatusLineConfig;
  path?: string;
  missing?: boolean;
  problem?: string;
};

export type ReadTextFile = (path: string) => Promise<string>;

export function createDefaultStatusLineConfig(): StatusLineConfig {
  return {
    format: DEFAULT_STATUS_LINE_FORMAT,
    disabledStatuses: new Set(),
  };
}

function invalidConfig(problem: string): StatusLineConfigResult {
  return { config: createDefaultStatusLineConfig(), problem };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function expandHome(path: string, homeDirectory: string): string {
  if (path === "~") return homeDirectory;
  if (path.startsWith("~/")) return join(homeDirectory, path.slice(2));
  return path;
}

export function getStatusLineConfigPath(
  env: NodeJS.ProcessEnv = process.env,
  homeDirectory: string = homedir(),
): string {
  const agentDirectory = env.PI_CODING_AGENT_DIR?.trim() || "~/.pi/agent";
  return join(expandHome(agentDirectory, homeDirectory), "status-line.json");
}

export function parseStatusLineConfigText(rawText: string): StatusLineConfigResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(rawText);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return invalidConfig(`invalid JSON: ${message}`);
  }

  if (!isRecord(parsed)) {
    return invalidConfig("status-line.json must contain a JSON object");
  }

  let format = DEFAULT_STATUS_LINE_FORMAT;
  if (parsed.format !== undefined) {
    if (!Array.isArray(parsed.format)) {
      return invalidConfig("format must be an array of lines");
    }

    const parsedFormat: string[][] = [];
    for (let lineIndex = 0; lineIndex < parsed.format.length; lineIndex += 1) {
      const line = parsed.format[lineIndex];
      if (!Array.isArray(line)) {
        return invalidConfig(`format[${lineIndex}] must be an array of component IDs`);
      }

      const parsedLine: string[] = [];
      for (let componentIndex = 0; componentIndex < line.length; componentIndex += 1) {
        const componentId = line[componentIndex];
        if (typeof componentId !== "string" || componentId.trim() === "") {
          return invalidConfig(`format[${lineIndex}][${componentIndex}] must be a non-blank string`);
        }
        if (componentId.startsWith("@jpi/") && !isJpiComponentId(componentId)) {
          return invalidConfig(
            `format[${lineIndex}][${componentIndex}] has unknown reserved ID ${componentId}`,
          );
        }
        if (componentId.startsWith(CUSTOM_COMPONENT_PREFIX) && !isCustomComponentId(componentId)) {
          return invalidConfig(
            `format[${lineIndex}][${componentIndex}] has a blank @custom: path`,
          );
        }
        parsedLine.push(componentId);
      }
      parsedFormat.push(parsedLine);
    }
    format = parsedFormat;
  }

  const disabledStatuses = new Set<string>();
  if (parsed.disabledStatuses !== undefined) {
    if (!Array.isArray(parsed.disabledStatuses)) {
      return invalidConfig("disabledStatuses must be an array");
    }

    for (let index = 0; index < parsed.disabledStatuses.length; index += 1) {
      const entry = parsed.disabledStatuses[index];
      if (typeof entry !== "string" || entry.trim() === "") {
        return invalidConfig(`disabledStatuses[${index}] must be a non-blank string`);
      }
      disabledStatuses.add(entry);
    }
  }

  return {
    config: { format, disabledStatuses },
  };
}

export async function loadStatusLineConfig(
  path: string,
  readTextFile: ReadTextFile = (filePath) => readFile(filePath, "utf8"),
): Promise<StatusLineConfigResult> {
  try {
    return { ...parseStatusLineConfigText(await readTextFile(path)), path };
  } catch (error) {
    if (isRecord(error) && error.code === "ENOENT") {
      return { config: createDefaultStatusLineConfig(), path, missing: true };
    }
    const message = error instanceof Error ? error.message : String(error);
    return {
      config: createDefaultStatusLineConfig(),
      path,
      problem: `could not read config: ${message}`,
    };
  }
}
