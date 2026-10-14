import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runCli } from '../src/cli.js';

const originalFetch = globalThis.fetch;
const originalToken = process.env.MEETILY_AUTOMATION_TOKEN;
const originalClientId = process.env.MEETILY_AUTOMATION_CLIENT_ID;
let root = '';
let discoveryPath = '';
const instance = '01234567-89ab-cdef-0123-456789abcdef';
const token = 'a'.repeat(64);

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'meetily-cli-'));
  discoveryPath = join(root, 'discovery.json');
  await writeFile(discoveryPath, JSON.stringify({ pid: process.pid, instance_id: instance, base_url: 'http://127.0.0.1:32123/api/internal/v1', protocol_version: 1 }));
  delete process.env.MEETILY_AUTOMATION_CLIENT_ID;
  process.env.MEETILY_AUTOMATION_TOKEN = token;
});

afterEach(async () => {
  globalThis.fetch = originalFetch;
  if (originalToken === undefined) delete process.env.MEETILY_AUTOMATION_TOKEN;
  else process.env.MEETILY_AUTOMATION_TOKEN = originalToken;
  if (originalClientId === undefined) delete process.env.MEETILY_AUTOMATION_CLIENT_ID;
  else process.env.MEETILY_AUTOMATION_CLIENT_ID = originalClientId;
  await rm(root, { recursive: true, force: true });
});

describe('CLI behavior', () => {
  test('writes result JSON to stdout and never includes the credential', async () => {
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith('/health')) return Response.json({ instance_id: instance, protocol_version: 1 });
      return Response.json({ items: [{ id: 'm1', title: 'Planning' }], next_cursor: null });
    }) as typeof fetch;
    let stdout = '';
    let stderr = '';
    const code = await runCli(['meetings', '--discovery', discoveryPath], { stdout: (value) => { stdout += value; }, stderr: (value) => { stderr += value; } });
    expect(code).toBe(0);
    expect(JSON.parse(stdout).items[0].id).toBe('m1');
    expect(stdout).not.toContain(token);
    expect(stderr).toBe('');
  });

  test('uses documented usage, unavailable, and authorization exit codes', async () => {
    const io = { stdout: () => {}, stderr: () => {} };
    expect(await runCli(['unknown'], io)).toBe(2);
    expect(await runCli(['meetings', '--discovery', join(root, 'missing.json')], io)).toBe(3);
    globalThis.fetch = (async () => Response.json({ instance_id: instance, protocol_version: 1 })) as typeof fetch;
    delete process.env.MEETILY_AUTOMATION_TOKEN;
    expect(await runCli(['meetings', '--discovery', discoveryPath], io)).toBe(4);
    process.env.MEETILY_AUTOMATION_TOKEN = token;
    globalThis.fetch = (async (input: RequestInfo | URL) => String(input).endsWith('/health')
      ? Response.json({ instance_id: instance, protocol_version: 1 })
      : Response.json({ error: { code: 'UNAUTHORIZED' } }, { status: 401 })) as typeof fetch;
    expect(await runCli(['meetings', '--discovery', discoveryPath], io)).toBe(4);
  });
});
