// A remote MCP server with one tool, "ping". POST /forget drops every session,
// so later requests that carry an old session id get 404, as the spec says.
// It prints its port on stdout once it listens.
import { randomUUID } from 'node:crypto';
import http from 'node:http';

type Message = {
  id?: unknown;
  method?: string;
  params?: { protocolVersion?: unknown };
};

const sessions = new Set<string>();

const result = (message: Message): unknown => {
  switch (message.method) {
    case 'initialize':
      return {
        protocolVersion: message.params?.protocolVersion,
        capabilities: { tools: {} },
        serverInfo: { name: 'session-server', version: '1.0.0' },
      };
    case 'tools/list':
      return { tools: [{ name: 'ping', inputSchema: { type: 'object' } }] };
    case 'tools/call':
      return { content: [{ type: 'text', text: 'pong' }] };
    default:
      return undefined;
  }
};

const send = (
  res: http.ServerResponse,
  status: number,
  body?: unknown,
  headers: Record<string, string> = {},
) => {
  res.writeHead(status, { 'content-type': 'application/json', ...headers });
  res.end(body === undefined ? undefined : JSON.stringify(body));
};

const server = http.createServer(async (req, res) => {
  if (req.url === '/forget') {
    sessions.clear();
    return send(res, 200, { ok: true });
  }
  if (req.method !== 'POST') return send(res, 405);
  const session = req.headers['mcp-session-id'];
  let text = '';
  for await (const chunk of req) text += chunk;
  const message: Message = JSON.parse(text);
  if (message.method === 'initialize') {
    const id = randomUUID();
    sessions.add(id);
    return send(
      res,
      200,
      { jsonrpc: '2.0', id: message.id, result: result(message) },
      { 'mcp-session-id': id },
    );
  }
  if (typeof session === 'string' && !sessions.has(session)) {
    return send(res, 404, {
      jsonrpc: '2.0',
      id: null,
      error: { code: -32001, message: 'Session not found' },
    });
  }
  if (message.id === undefined) return send(res, 202);
  const value = result(message);
  return send(
    res,
    200,
    value === undefined
      ? {
          jsonrpc: '2.0',
          id: message.id,
          error: { code: -32601, message: 'Method not found' },
        }
      : { jsonrpc: '2.0', id: message.id, result: value },
  );
});

server.listen(0, '127.0.0.1', () => {
  const address = server.address();
  if (address !== null && typeof address === 'object') {
    process.stdout.write(`${address.port}\n`);
  }
});
