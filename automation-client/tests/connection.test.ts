import { afterEach, beforeEach, expect, test } from 'bun:test';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { connect, ConnectionError } from '../src/connection.js';

const originalFetch = globalThis.fetch;
const originalToken = process.env.MEETILY_AUTOMATION_TOKEN;
const instance = '01234567-89ab-cdef-0123-456789abcdef';
let root = '';
let discoveryPath = '';

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'meetily-connection-'));
  discoveryPath = join(root, 'discovery.json');
  await writeFile(discoveryPath, JSON.stringify({ pid: process.pid, instance_id: instance, base_url: 'http://127.0.0.1:32123/api/internal/v1', protocol_version: 1 }));
  process.env.MEETILY_AUTOMATION_TOKEN = 'a'.repeat(64);
});

afterEach(async () => {
  globalThis.fetch = originalFetch;
  if (originalToken === undefined) delete process.env.MEETILY_AUTOMATION_TOKEN;
  else process.env.MEETILY_AUTOMATION_TOKEN = originalToken;
  await rm(root, { recursive: true, force: true });
});

test('rejects stale discovery instance ids and missing credentials', async () => {
  globalThis.fetch = (async () => Response.json({ instance_id: 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', protocol_version: 1 })) as typeof fetch;
  await expect(connect({ discoveryPath })).rejects.toMatchObject({ code: 'INSTANCE_UNAVAILABLE' });

  globalThis.fetch = (async () => Response.json({ instance_id: instance, protocol_version: 1 })) as typeof fetch;
  delete process.env.MEETILY_AUTOMATION_TOKEN;
  await expect(connect({ discoveryPath })).rejects.toMatchObject({ code: 'AUTH_REQUIRED' });
});

test('rejects discovery records that point outside IPv4 loopback', async () => {
  await writeFile(discoveryPath, JSON.stringify({ pid: process.pid, instance_id: instance, base_url: 'http://127.0.0.2:32123/api/internal/v1', protocol_version: 1 }));
  try {
    await connect({ discoveryPath });
    throw new Error('expected invalid discovery to fail');
  } catch (error) {
    expect(error).toBeInstanceOf(ConnectionError);
    expect((error as ConnectionError).code).toBe('INSTANCE_UNAVAILABLE');
  }
});
