import { expect, test } from 'bun:test';
import { StdioServerTransport } from '@modelcontextprotocol/server/stdio';
import { PassThrough } from 'node:stream';
import type { ConnectedInstance } from '../src/connection.js';
import { createMcpServer } from '../src/mcp.js';
import { AutomationClient } from '../src/http.js';

test('stdio exposes only the five read tools and sends JSON-RPC on stdout', async () => {
  const instance: ConnectedInstance = {
    discovery: { pid: 1, instance_id: '01234567-89ab-cdef-0123-456789abcdef', base_url: 'http://127.0.0.1:32123/api/internal/v1', protocol_version: 1 },
    baseUrl: new URL('http://127.0.0.1:32123/api/internal/v1'),
    token: 'a'.repeat(64),
  };
  const client = new AutomationClient(instance, (async () => Response.json({ items: [], next_cursor: null })) as typeof fetch);
  const server = createMcpServer(client);
  const input = new PassThrough();
  const output = new PassThrough();
  let buffer = '';
  const messages: Array<Record<string, unknown>> = [];
  const waiters: Array<(message: Record<string, unknown>) => void> = [];
  output.on('data', (chunk: Buffer) => {
    buffer += chunk.toString('utf8');
    let end = buffer.indexOf('\n');
    while (end >= 0) {
      const line = buffer.slice(0, end).trim();
      buffer = buffer.slice(end + 1);
      if (line) {
        const value = JSON.parse(line) as Record<string, unknown>;
        const waiter = waiters.shift();
        if (waiter) waiter(value);
        else messages.push(value);
      }
      end = buffer.indexOf('\n');
    }
  });
  const nextMessage = () => new Promise<Record<string, unknown>>((resolve) => {
    const existing = messages.shift();
    if (existing) resolve(existing);
    else waiters.push(resolve);
  });
  const transport = new StdioServerTransport(input, output);
  await server.connect(transport);
  try {
    const initialized = nextMessage();
    input.write(`${JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '1.0.0' } } })}\n`);
    const initReply = await initialized;
    expect(initReply.id).toBe(1);
    input.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' })}\n`);
    const listed = nextMessage();
    input.write(`${JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} })}\n`);
    const reply = await listed;
    expect(reply.id).toBe(2);
    const result = reply.result as { tools: Array<{ name: string }> };
    expect(result.tools.map((tool) => tool.name).sort()).toEqual(['get_job', 'get_summary', 'get_transcript', 'list_meetings', 'search_meetings']);
    expect(buffer).toBe('');
  } finally {
    await server.close();
  }
});
