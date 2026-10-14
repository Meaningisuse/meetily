import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';
import type { Discovery } from './contracts.js';

const execFileAsync = promisify(execFile);
const KEYCHAIN_SERVICE = 'com.meetily.internal-automation';
const API_PATH = '/api/internal/v1';

export type ConnectionFailureCode = 'INSTANCE_UNAVAILABLE' | 'AUTH_REQUIRED' | 'AUTH_REJECTED' | 'API_ERROR';

export class ConnectionError extends Error {
  constructor(public readonly code: ConnectionFailureCode, message: string, public readonly status?: number) {
    super(message);
    this.name = 'ConnectionError';
  }
}

export interface ConnectionOptions {
  discoveryPath?: string;
  clientId?: string;
  token?: string;
}

export interface ConnectedInstance {
  discovery: Discovery;
  baseUrl: URL;
  token: string;
}

export function defaultDiscoveryPath(): string {
  if (process.platform === 'darwin') return join(homedir(), 'Library', 'Application Support', 'com.meetily.ai', 'automation', 'discovery.json');
  if (process.platform === 'win32') return join(process.env.APPDATA ?? join(homedir(), 'AppData', 'Roaming'), 'com.meetily.ai', 'automation', 'discovery.json');
  return join(process.env.XDG_DATA_HOME ?? join(homedir(), '.local', 'share'), 'com.meetily.ai', 'automation', 'discovery.json');
}

async function readToken(clientId: string): Promise<string> {
  if (process.platform !== 'darwin') throw new ConnectionError('AUTH_REQUIRED', 'Keychain token lookup is supported on macOS; set MEETILY_AUTOMATION_TOKEN for a temporary local session.');
  try {
    const { stdout } = await execFileAsync('/usr/bin/security', ['find-generic-password', '-s', KEYCHAIN_SERVICE, '-a', clientId, '-w'], { timeout: 3000, maxBuffer: 4096, windowsHide: true });
    const token = stdout.trim();
    if (/^[a-f0-9]{64}$/i.test(token)) return token;
  } catch {
    // Do not expose keychain stderr, which can contain account metadata.
  }
  throw new ConnectionError('AUTH_REQUIRED', 'No saved Meetily automation credential was found for this client id.');
}

function validateDiscovery(value: unknown): Discovery {
  if (typeof value !== 'object' || value === null) throw new ConnectionError('INSTANCE_UNAVAILABLE', 'Meetily discovery data is invalid.');
  const candidate = value as Partial<Discovery>;
  if (!Number.isSafeInteger(candidate.pid) || (candidate.pid ?? 0) <= 0 || typeof candidate.instance_id !== 'string' || !/^[0-9a-f-]{36}$/i.test(candidate.instance_id) || candidate.protocol_version !== 1 || typeof candidate.base_url !== 'string') {
    throw new ConnectionError('INSTANCE_UNAVAILABLE', 'Meetily discovery data is invalid or uses an unsupported protocol.');
  }
  let baseUrl: URL;
  try { baseUrl = new URL(candidate.base_url); } catch { throw new ConnectionError('INSTANCE_UNAVAILABLE', 'Meetily discovery URL is invalid.'); }
  if (baseUrl.protocol !== 'http:' || baseUrl.hostname !== '127.0.0.1' || !baseUrl.port || baseUrl.username || baseUrl.password || baseUrl.pathname !== API_PATH || baseUrl.search || baseUrl.hash) {
    throw new ConnectionError('INSTANCE_UNAVAILABLE', 'Meetily discovery URL must be a local loopback endpoint.');
  }
  return candidate as Discovery;
}

export async function connect(options: ConnectionOptions = {}): Promise<ConnectedInstance> {
  const discoveryPath = options.discoveryPath ?? process.env.MEETILY_AUTOMATION_DISCOVERY ?? defaultDiscoveryPath();
  let discovery: Discovery;
  try {
    const raw = await readFile(discoveryPath, 'utf8');
    if (raw.length > 4096) throw new Error('too large');
    discovery = validateDiscovery(JSON.parse(raw) as unknown);
  } catch (error) {
    if (error instanceof ConnectionError) throw error;
    throw new ConnectionError('INSTANCE_UNAVAILABLE', `Meetily is unavailable or discovery cannot be read (${dirname(discoveryPath)}).`);
  }

  const baseUrl = new URL(discovery.base_url);
  try {
    const response = await fetch(new URL('health', `${baseUrl.href}/`), { signal: AbortSignal.timeout(3000), redirect: 'error' });
    if (!response.ok) throw new Error('health rejected');
    const health = await response.json() as { instance_id?: string; protocol_version?: number };
    if (health.instance_id !== discovery.instance_id || health.protocol_version !== 1) throw new Error('stale instance');
  } catch {
    throw new ConnectionError('INSTANCE_UNAVAILABLE', 'Meetily is closed or the saved discovery record is stale.');
  }

  const clientId = options.clientId ?? process.env.MEETILY_AUTOMATION_CLIENT_ID;
  const token = options.token ?? process.env.MEETILY_AUTOMATION_TOKEN ?? (clientId ? await readToken(clientId) : undefined);
  if (!token || !/^[a-f0-9]{64}$/i.test(token)) throw new ConnectionError('AUTH_REQUIRED', 'Provide MEETILY_AUTOMATION_CLIENT_ID for Keychain lookup or a temporary MEETILY_AUTOMATION_TOKEN.');
  return { discovery, baseUrl, token };
}
