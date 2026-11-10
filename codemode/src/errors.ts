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
// The code for any failed HTTP request other than a login failure. The daemon
// drops the HTTP status, so the code is all cm can see.
const DAEMON_HTTP_CODE = 'CLIENT_HTTP_NOT_IMPLEMENTED';

const hasCode = (error: unknown, code: string): boolean =>
  error instanceof Error && 'code' in error && error.code === code;

export const isHttpFailure = (error: unknown): boolean =>
  hasCode(error, DAEMON_HTTP_CODE);

export const needsLogin = (error: unknown): boolean =>
  hasCode(error, DAEMON_AUTH_CODE) ||
  describeConnectionIssue(error).kind === 'auth';

export const serverFailure = (server: string, error: unknown): string => {
  if (isHttpFailure(error)) {
    return `Server "${server}" failed with an HTTP error, even on a new connection.`;
  }
  const message = errorMessage(error);
  return needsLogin(error)
    ? `Server "${server}" needs a login. Run: cm auth ${server} (${message})`
    : `Server "${server}" failed: ${message}`;
};

export const callFailure = (server: string): string =>
  `Server "${server}" failed with an HTTP error. cm uses a new connection on the next run.`;
