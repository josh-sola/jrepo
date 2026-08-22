import { constants, readFileSync } from "node:fs";
import { access as fsAccess } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

function readKetchVersion(): string {
  const manifestUrl = new URL("../../ketch-release.json", import.meta.url);
  const manifest = JSON.parse(readFileSync(manifestUrl, "utf8"));
  if (typeof manifest.version !== "string") {
    throw new Error("The ketch release manifest must include a version string.");
  }
  return manifest.version;
}

export const KETCH_VERSION = readKetchVersion();
export const MAX_KETCH_DIAGNOSTIC_CHARS = 2_000;

const CACHE_ROOT = join("node_modules", ".cache", "jpi", "ketch");

export type KetchExecResult = {
  stdout?: string;
  stderr?: string;
  code?: number | null;
  killed?: boolean;
};

export type KetchExecOptions = {
  signal?: AbortSignal;
  timeout?: number;
};

export type KetchExec = (command: string, args: string[], options: KetchExecOptions) => Promise<KetchExecResult>;
export type KetchAccess = (path: string) => Promise<void>;
export type KetchResolver = (signal?: AbortSignal) => Promise<string>;

export type KetchRunOptions = {
  signal?: AbortSignal;
  timeoutMs: number;
};

export type KetchRunner = {
  runJson(args: string[], options: KetchRunOptions): Promise<unknown>;
};

export type KetchPlatform = {
  directory: string;
  executableName: string;
};

export type ResolveKetchOptions = {
  access?: KetchAccess;
  env?: NodeJS.ProcessEnv;
  packageRoot?: string;
  platform?: NodeJS.Platform;
  arch?: string;
};

export type CreateKetchRunnerOptions = ResolveKetchOptions & {
  exec: KetchExec;
  resolver?: KetchResolver;
};

function defaultPackageRoot(): string {
  return join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
}

async function defaultAccess(path: string): Promise<void> {
  await fsAccess(path, constants.X_OK);
}

function isExecutableNotFound(error: unknown): boolean {
  return Boolean(error) && typeof error === "object" && "code" in error && (error as { code?: unknown }).code === "ENOENT";
}

async function pathExists(path: string, access: KetchAccess): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

export function mapKetchPlatform(platform: NodeJS.Platform = process.platform, arch: string = process.arch): KetchPlatform | undefined {
  if (platform !== "darwin" && platform !== "linux" && platform !== "win32") return undefined;
  if (arch !== "arm64" && arch !== "x64") return undefined;

  const releaseArch = arch === "x64" ? "x86_64" : arch;
  return {
    directory: `${platform}-${releaseArch}`,
    executableName: platform === "win32" ? "ketch.exe" : "ketch",
  };
}

export function getPackageKetchPath(options: ResolveKetchOptions = {}): string | undefined {
  const platform = mapKetchPlatform(options.platform, options.arch);
  if (!platform) return undefined;

  return join(
    options.packageRoot ?? defaultPackageRoot(),
    CACHE_ROOT,
    KETCH_VERSION,
    platform.directory,
    platform.executableName,
  );
}

function getPathExecutableNames(platform: NodeJS.Platform, env: NodeJS.ProcessEnv): string[] {
  if (platform !== "win32") return ["ketch"];

  const extensions = (env.PATHEXT || ".EXE;.CMD;.BAT;.COM")
    .split(";")
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean);
  const names = extensions.map((extension) => `ketch${extension.startsWith(".") ? extension : `.${extension}`}`);
  names.push("ketch");
  return [...new Set(names)];
}

export async function findKetchOnPath(options: ResolveKetchOptions = {}): Promise<string | undefined> {
  const access = options.access ?? defaultAccess;
  const env = options.env ?? process.env;
  const platform = options.platform ?? process.platform;
  const pathValue = env.PATH ?? "";
  if (!pathValue) return undefined;

  const pathDelimiter = platform === "win32" ? ";" : ":";
  const executableNames = getPathExecutableNames(platform, env);
  for (const rawDir of pathValue.split(pathDelimiter)) {
    const dir = rawDir || ".";
    for (const name of executableNames) {
      const candidate = join(dir, name);
      if (await pathExists(candidate, access)) return candidate;
    }
  }

  return undefined;
}

export async function resolveKetchExecutable(options: ResolveKetchOptions = {}, signal?: AbortSignal): Promise<string> {
  throwIfAborted(signal);

  const access = options.access ?? defaultAccess;
  const packagePath = getPackageKetchPath(options);
  if (packagePath && await pathExists(packagePath, access)) return packagePath;

  const pathExecutable = await findKetchOnPath(options);
  if (pathExecutable) return pathExecutable;

  throw new Error(
    "Could not find ketch. The package-owned ketch binary is missing, which can happen when npm lifecycle scripts are disabled. Install ketch on PATH for local development or reinstall this package with lifecycle scripts enabled.",
  );
}

export function truncateDiagnostic(value: string, maxChars = MAX_KETCH_DIAGNOSTIC_CHARS): string {
  const normalized = value.replace(/\0/g, "").trim();
  if (normalized.length <= maxChars) return normalized;
  return `${normalized.slice(0, Math.max(0, maxChars - 1))}…`;
}

function formatDiagnostics(stderr: string | undefined): string {
  const diagnostics = truncateDiagnostic(stderr ?? "");
  if (!diagnostics) return "";
  return `\nDiagnostics:\n${diagnostics}`;
}

function messageFromError(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

function throwIfAborted(signal: AbortSignal | undefined): void {
  if (signal?.aborted) throw new Error("The web request was cancelled.");
}

function parseJsonOutput(result: KetchExecResult): unknown {
  const stdout = typeof result.stdout === "string" ? result.stdout.trim() : "";
  if (!stdout) throw new Error(`Ketch returned no JSON output.${formatDiagnostics(result.stderr)}`);

  try {
    return JSON.parse(stdout);
  } catch {
    throw new Error(`Ketch returned malformed JSON output.${formatDiagnostics(result.stderr)}`);
  }
}

function exitCodeError(code: number | null | undefined, result: KetchExecResult): Error {
  const diagnostics = formatDiagnostics(result.stderr);

  if (code === 2) {
    return new Error(`Ketch rejected the request or this extension called ketch with invalid arguments.${diagnostics}`);
  }
  if (code === 3) {
    return new Error(`Ketch could not find the page or requested content.${diagnostics}`);
  }
  if (code === 4) {
    return new Error(`Ketch is temporarily unavailable. Try again later.${diagnostics}`);
  }
  if (code === 5) {
    return new Error(`Ketch needs an optional capability that is not configured. Configure that capability or use a simpler page.${diagnostics}`);
  }
  if (code === 6) {
    return new Error("Ketch cancelled the request.");
  }
  if (typeof code === "number") {
    return new Error(`Ketch failed with exit code ${code}.${diagnostics}`);
  }
  return new Error(`Ketch ended without a usable exit code.${diagnostics}`);
}

export function createKetchRunner(options: CreateKetchRunnerOptions): KetchRunner {
  const resolver = options.resolver ?? ((signal?: AbortSignal) => resolveKetchExecutable(options, signal));

  return {
    async runJson(args: string[], runOptions: KetchRunOptions): Promise<unknown> {
      const executable = await resolver(runOptions.signal);

      for (let attempt = 0; attempt < 2; attempt += 1) {
        throwIfAborted(runOptions.signal);

        let result: KetchExecResult;
        try {
          result = await options.exec(executable, [...args], {
            signal: runOptions.signal,
            timeout: runOptions.timeoutMs,
          });
        } catch (error) {
          if (runOptions.signal?.aborted) throwIfAborted(runOptions.signal);
          if (isExecutableNotFound(error)) {
            throw new Error(
              "Could not run ketch. The resolved executable was not found. Reinstall this package or install ketch on PATH for local development.",
            );
          }
          throw new Error(`Could not run ketch: ${truncateDiagnostic(messageFromError(error))}`);
        }

        throwIfAborted(runOptions.signal);

        if (result.killed) {
          throw new Error("Ketch was terminated or timed out before it finished. Try again or use a narrower request.");
        }

        const code = result.code;
        if (code === 0) return parseJsonOutput(result);
        if (code === 4 && attempt === 0) continue;
        throw exitCodeError(code, result);
      }

      throw new Error("Ketch is temporarily unavailable. Try again later.");
    },
  };
}
