import { describeConnectionIssue } from 'mcporter';

export class UsageError extends Error {
  override name = 'UsageError';
}

export class ServerError extends Error {
  override name = 'ServerError';
}

export const errorMessage = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

// The daemon relays failures as a bare code, which describeConnectionIssue ignores.
const DAEMON_AUTH_CODE = 'CLIENT_HTTP_AUTHENTICATION';

const needsLogin = (error: unknown): boolean =>
  (error instanceof Error &&
    'code' in error &&
    error.code === DAEMON_AUTH_CODE) ||
  describeConnectionIssue(error).kind === 'auth';

export const serverFailure = (server: string, error: unknown): string => {
  const message = errorMessage(error);
  return needsLogin(error)
    ? `Server "${server}" needs a login. Run: cm auth ${server} (${message})`
    : `Server "${server}" failed: ${message}`;
};
