import { describe, expect, test } from 'bun:test';
import { buildGitEnv, createGit } from './git.ts';

const decode = (bytes: Uint8Array) => new TextDecoder().decode(bytes);

describe('buildGitEnv', () => {
  test('numbers the config entries and keeps the token out of them', () => {
    const env = buildGitEnv({
      token: 'secret-token',
      safeDirectories: ['/graphite/a', '/graphite/b'],
    });

    expect(env.GIT_CONFIG_COUNT).toBe('3');
    expect(env.GIT_CONFIG_KEY_0).toBe('credential.https://github.com.helper');
    expect(env.GIT_CONFIG_KEY_1).toBe('safe.directory');
    expect(env.GIT_CONFIG_VALUE_1).toBe('/graphite/a');
    expect(env.GIT_CONFIG_VALUE_2).toBe('/graphite/b');
    expect(env.GIT_CONFIG_VALUE_0).not.toContain('secret-token');
    expect(env.GIT_TERMINAL_PROMPT).toBe('0');
  });
});

describe('createGit', () => {
  test('hands the token to git as a github.com credential', async () => {
    const git = createGit({ token: 'secret-token', safeDirectories: [] });

    const { stdout, exitCode } = await git.run(['credential', 'fill'], {
      stdin: 'protocol=https\nhost=github.com\n\n',
    });

    expect(exitCode).toBe(0);
    expect(decode(stdout)).toContain('username=x-access-token');
    expect(decode(stdout)).toContain('password=secret-token');
  });

  test('trusts the configured safe directories', async () => {
    const git = createGit({
      token: 't',
      safeDirectories: ['/graphite/monorepo'],
    });

    const { stdout } = await git.run(['config', '--get-all', 'safe.directory']);

    expect(decode(stdout).trim().split('\n')).toContain('/graphite/monorepo');
  });

  test('runOrThrow reports the failing command', async () => {
    const git = createGit({ token: 't', safeDirectories: [] });

    await expect(git.runOrThrow(['no-such-subcommand'])).rejects.toThrow(
      /git no-such-subcommand failed/,
    );
  });
});
