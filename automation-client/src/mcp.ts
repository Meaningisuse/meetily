#!/usr/bin/env node
import { McpServer } from '@modelcontextprotocol/server';
import { serveStdio } from '@modelcontextprotocol/server/stdio';
import { pathToFileURL } from 'node:url';
import { z } from 'zod';
import { ConnectionError, connect, type ConnectionOptions } from './connection.js';
import { AutomationClient } from './http.js';

function pageOptions(limit?: number, cursor?: string) {
  return {
    ...(limit === undefined ? {} : { limit }),
    ...(cursor === undefined ? {} : { cursor }),
  };
}

export function createMcpServer(client: AutomationClient): McpServer {
  const server = new McpServer({ name: 'meetily-internal', version: '0.1.0' });
  const pageSchema = z.object({
    limit: z.number().int().min(1).max(200).optional(),
    cursor: z.string().max(1024).optional(),
  });
  server.registerTool('list_meetings', {
    title: 'List meetings',
    description: 'Read one bounded page of the current user\'s local meetings. Use next_cursor to request a later page.',
    inputSchema: pageSchema,
    annotations: { readOnlyHint: true },
  }, async ({ limit, cursor }) => ({ content: [{ type: 'text', text: JSON.stringify(await client.listMeetings(pageOptions(limit, cursor))) }] }));
  server.registerTool('search_meetings', {
    title: 'Search meetings',
    description: 'Search transcript text locally and return bounded snippets and segment times.',
    inputSchema: z.object({ query: z.string().min(1).max(256), limit: z.number().int().min(1).max(200).optional(), cursor: z.string().max(1024).optional() }),
    annotations: { readOnlyHint: true },
  }, async ({ query, limit, cursor }) => ({ content: [{ type: 'text', text: JSON.stringify(await client.searchMeetings(query, pageOptions(limit, cursor))) }] }));
  server.registerTool('get_transcript', {
    title: 'Get transcript page',
    description: 'Read one bounded page of transcript segments for a meeting.',
    inputSchema: z.object({ meeting_id: z.string().min(1).max(256), limit: z.number().int().min(1).max(200).optional(), cursor: z.string().max(1024).optional() }),
    annotations: { readOnlyHint: true },
  }, async ({ meeting_id, limit, cursor }) => ({ content: [{ type: 'text', text: JSON.stringify(await client.getTranscript(meeting_id, pageOptions(limit, cursor))) }] }));
  server.registerTool('get_summary', {
    title: 'Get meeting summary',
    description: 'Read the saved summary and current source revision for a meeting.',
    inputSchema: z.object({ meeting_id: z.string().min(1).max(256) }),
    annotations: { readOnlyHint: true },
  }, async ({ meeting_id }) => ({ content: [{ type: 'text', text: JSON.stringify(await client.getSummary(meeting_id)) }] }));
  server.registerTool('get_job', {
    title: 'Get enhancement job status',
    description: 'Read a bounded status record for a meeting enhancement job.',
    inputSchema: z.object({ job_id: z.string().min(1).max(256) }),
    annotations: { readOnlyHint: true },
  }, async ({ job_id }) => ({ content: [{ type: 'text', text: JSON.stringify(await client.getJob(job_id)) }] }));
  return server;
}

export async function startMcp(options: ConnectionOptions = {}): Promise<void> {
  const connected = await connect(options);
  const client = new AutomationClient(connected);
  serveStdio(() => createMcpServer(client), { onerror: (error) => process.stderr.write(`Meetily MCP protocol error: ${error.message}\n`) });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  startMcp().catch((error: unknown) => {
    if (error instanceof ConnectionError) {
      process.stderr.write(`${error.message}\n`);
      process.exitCode = error.code === 'INSTANCE_UNAVAILABLE' ? 3 : error.code.startsWith('AUTH_') ? 4 : 1;
    } else {
      process.stderr.write('Meetily MCP could not start.\n');
      process.exitCode = 1;
    }
  });
}
