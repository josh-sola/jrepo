import { describe, expect, it } from 'vitest';
import { serverFailure } from '../src/errors.ts';

const withCode = (message: string, code: string) =>
  Object.assign(new Error(message), { code });

describe('serverFailure', () => {
  it('points to cm auth when the daemon reports an authentication failure', () => {
    const error = withCode(
      'MCP operation failed (CLIENT_HTTP_AUTHENTICATION); the request was not replayed.',
      'CLIENT_HTTP_AUTHENTICATION',
    );
    expect(serverFailure('dd', error)).toMatch(
      /needs a login\. Run: cm auth dd/,
    );
  });

  it('points to cm auth for an in-process 401', () => {
    expect(serverFailure('dd', new Error('HTTP 401 Unauthorized'))).toMatch(
      /Run: cm auth dd/,
    );
  });

  it('reports other failures without the login hint', () => {
    const message = serverFailure(
      'fs',
      withCode('MCP operation failed (CONNECTION_CLOSED)', 'CONNECTION_CLOSED'),
    );
    expect(message).toMatch(/^Server "fs" failed:/);
    expect(message).not.toMatch(/cm auth/);
  });
});
