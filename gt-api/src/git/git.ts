export interface GitRunOptions {
  cwd?: string;
  stdin?: string;
}

export interface GitRunResult {
  stdout: Uint8Array;
  stderr: string;
  exitCode: number;
}

export interface Git {
  run(args: string[], options?: GitRunOptions): Promise<GitRunResult>;
  runOrThrow(args: string[], options?: GitRunOptions): Promise<void>;
}

export interface GitEnvOptions {
  token: string;
  // Directories owned by another user that git must still open, such as a
  // read-only checkout mounted into a container.
  safeDirectories: string[];
}

const TOKEN_ENV_VAR = 'GT_API_GIT_TOKEN';
const CREDENTIAL_HELPER = `!f() { echo username=x-access-token; echo "password=$${TOKEN_ENV_VAR}"; }; f`;

// Configures git through its environment so the token never lands in a
// gitconfig file or in a remote URL stored in a clone. The empty
// `credential.helper` first drops any helper from the user's own config, such
// as the macOS keychain, which would otherwise save the token after a fetch.
export function buildGitEnv(
  options: GitEnvOptions,
): Record<string, string | undefined> {
  const entries: [string, string][] = [
    ['credential.helper', ''],
    ['credential.https://github.com.helper', CREDENTIAL_HELPER],
    ...options.safeDirectories.map((dir): [string, string] => [
      'safe.directory',
      dir,
    ]),
  ];
  const env: Record<string, string | undefined> = {
    ...process.env,
    [TOKEN_ENV_VAR]: options.token,
    GIT_TERMINAL_PROMPT: '0',
    GIT_CONFIG_COUNT: String(entries.length),
  };
  entries.forEach(([key, value], index) => {
    env[`GIT_CONFIG_KEY_${index}`] = key;
    env[`GIT_CONFIG_VALUE_${index}`] = value;
  });
  return env;
}

export function createGit(options: GitEnvOptions): Git {
  const env = buildGitEnv(options);

  async function run(
    args: string[],
    { cwd, stdin }: GitRunOptions = {},
  ): Promise<GitRunResult> {
    const proc = Bun.spawn(['git', ...args], {
      cwd,
      env,
      stdin: 'pipe',
      stdout: 'pipe',
      stderr: 'pipe',
    });
    if (stdin !== undefined) proc.stdin.write(stdin);
    proc.stdin.end();
    const [stdout, stderr, exitCode] = await Promise.all([
      new Response(proc.stdout)
        .arrayBuffer()
        .then((buffer) => new Uint8Array(buffer)),
      new Response(proc.stderr).text(),
      proc.exited,
    ]);
    return { stdout, stderr, exitCode };
  }

  return {
    run,
    async runOrThrow(args, runOptions) {
      const { stderr, exitCode } = await run(args, runOptions);
      if (exitCode !== 0) {
        throw new Error(`git ${args.join(' ')} failed: ${stderr.trim()}`);
      }
    },
  };
}
