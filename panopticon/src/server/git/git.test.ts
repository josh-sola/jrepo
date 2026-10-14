import { describe, expect, test } from 'bun:test';
import { buildGitEnv, createGit } from './git.ts';

const decode = (bytes: Uint8Array) => new TextDecoder().decode(bytes);

describe('buildGitEnv', () => {
  test('numbers the config entries and keeps the token out of them', () => {
    const env = buildGitEnv({ token: 'secret-token' });

    expect(env.GIT_CONFIG_COUNT).toBe('2');
    expect(env.GIT_CONFIG_KEY_0).toBe('credential.helper');
    expect(env.GIT_CONFIG_VALUE_0).toBe('');
    expect(env.GIT_CONFIG_KEY_1).toBe('credential.https://github.com.helper');
    expect(env.GIT_CONFIG_VALUE_1).not.toContain('secret-token');
    expect(env.GIT_TERMINAL_PROMPT).toBe('0');
  });
});

describe('createGit', () => {
  test('hands the token to git as a github.com credential', async () => {
    const git = createGit({ token: 'secret-token' });

    const { stdout, exitCode } = await git.run(['credential', 'fill'], {
      stdin: 'protocol=https\nhost=github.com\n\n',
    });

    expect(exitCode).toBe(0);
    expect(decode(stdout)).toContain('username=x-access-token');
    expect(decode(stdout)).toContain('password=secret-token');
  });

  test('runOrThrow reports the failing command', async () => {
    const git = createGit({ token: 't' });

    await expect(git.runOrThrow(['no-such-subcommand'])).rejects.toThrow(
      /git no-such-subcommand failed/,
    );
  });
});
