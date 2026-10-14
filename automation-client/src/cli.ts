#!/usr/bin/env node
import { pathToFileURL } from 'node:url';
import { ConnectionError, connect } from './connection.js';
import { AutomationClient } from './http.js';

export interface CliIo {
  stdout: (text: string) => void;
  stderr: (text: string) => void;
}

interface ParsedArgs {
  command: string;
  positional: string[];
  options: { limit?: number; cursor?: string; clientId?: string; discoveryPath?: string };
}

function parseArgs(argv: string[]): ParsedArgs {
  const positional: string[] = [];
  const options: ParsedArgs['options'] = {};
  const booleanless = new Set(['--limit', '--cursor', '--client-id', '--discovery']);
  for (let index = 0; index < argv.length; index++) {
    const value = argv[index]!;
    if (!value.startsWith('--')) { positional.push(value); continue; }
    if (!booleanless.has(value)) throw new Error(`Unknown option: ${value}`);
    const next = argv[++index];
    if (next === undefined || next.startsWith('--')) throw new Error(`Missing value for ${value}`);
    if (value === '--limit') {
      const parsed = Number(next);
      if (!Number.isInteger(parsed) || parsed < 1 || parsed > 200) throw new Error('--limit must be an integer from 1 to 200');
      options.limit = parsed;
    } else if (value === '--cursor') options.cursor = next;
    else if (value === '--client-id') options.clientId = next;
    else options.discoveryPath = next;
  }
  const [command = ''] = positional;
  return { command, positional: positional.slice(1), options };
}

function usageError(io: CliIo, message: string): number {
  io.stderr(`${message}\nUsage: meetily-internal <meetings|search|transcript|summary|job> [value] [--limit 1..200] [--cursor value] [--client-id id] [--discovery path]\n`);
  return 2;
}

export async function runCli(argv: string[], io: CliIo = { stdout: (v) => process.stdout.write(v), stderr: (v) => process.stderr.write(v) }): Promise<number> {
  let parsed: ParsedArgs;
  try { parsed = parseArgs(argv); } catch (error) { return usageError(io, error instanceof Error ? error.message : 'Invalid arguments'); }
  const { command, positional, options } = parsed;
  const needsValue = ['search', 'transcript', 'summary', 'job'].includes(command);
  if (!['meetings', 'search', 'transcript', 'summary', 'job'].includes(command) || positional.length !== (needsValue ? 1 : 0)) {
    return usageError(io, 'Invalid command or arguments.');
  }
  try {
    const connection = await connect({
      ...(options.discoveryPath === undefined ? {} : { discoveryPath: options.discoveryPath }),
      ...(options.clientId === undefined ? {} : { clientId: options.clientId }),
    });
    const client = new AutomationClient(connection);
    let result: unknown;
    if (command === 'meetings') result = await client.listMeetings(options);
    else if (command === 'search') result = await client.searchMeetings(positional[0]!, options);
    else if (command === 'transcript') result = await client.getTranscript(positional[0]!, options);
    else if (command === 'summary') result = await client.getSummary(positional[0]!);
    else result = await client.getJob(positional[0]!);
    io.stdout(`${JSON.stringify(result, null, 2)}\n`);
    return 0;
  } catch (error) {
    if (error instanceof ConnectionError) {
      io.stderr(`${error.message}\n`);
      if (error.code === 'INSTANCE_UNAVAILABLE') return 3;
      if (error.code === 'AUTH_REQUIRED' || error.code === 'AUTH_REJECTED') return 4;
      return 1;
    }
    io.stderr('The Meetily automation request failed.\n');
    return 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await runCli(process.argv.slice(2));
}
