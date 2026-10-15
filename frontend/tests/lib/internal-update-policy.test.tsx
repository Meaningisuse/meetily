import React from 'react';
import { afterAll, afterEach, beforeEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { readFileSync } from 'node:fs';
import type { Update } from '@tauri-apps/plugin-updater';

// Native and portal boundaries are stubbed; the real service, hook and UI actions run below.
const originals = {
  app: { ...await import('@tauri-apps/api/app') },
  updater: { ...await import('@tauri-apps/plugin-updater') },
  process: { ...await import('@tauri-apps/plugin-process') },
  sonner: { ...await import('sonner') },
  dialog: { ...await import('../../src/components/ui/dialog') },
  image: { ...await import('next/image') },
  consent: { ...await import('../../src/components/AnalyticsConsentSwitch') },
};
let readVersion = async (): Promise<string> => '0.4.1';
const getVersion = mock(async () => readVersion());
const check = mock(async () => ({ available: true, version: '99.0.0' }));
const relaunch = mock(async () => {});
const notifyInfo = mock((_content: React.ReactNode) => {});
const notifySuccess = mock((_message: string) => {});
const notifyError = mock((_message: string) => {});
mock.module('@tauri-apps/api/app', () => ({ ...originals.app, getVersion }));
mock.module('@tauri-apps/plugin-updater', () => ({ ...originals.updater, check }));
mock.module('@tauri-apps/plugin-process', () => ({ ...originals.process, relaunch }));
mock.module('sonner', () => ({ ...originals.sonner, toast: { info: notifyInfo, success: notifySuccess, error: notifyError } }));
const Box = ({ children }: { children?: React.ReactNode }) => <div>{children}</div>;
mock.module('../../src/components/ui/dialog', () => ({
  ...originals.dialog, Dialog: ({ open, children }: { open: boolean; children: React.ReactNode }) => open ? <>{children}</> : null,
  DialogContent: Box, DialogDescription: Box, DialogFooter: Box, DialogHeader: Box, DialogTitle: Box,
}));
mock.module('next/image', () => ({ ...originals.image, default: () => null }));
mock.module('../../src/components/AnalyticsConsentSwitch', () => ({ ...originals.consent, default: () => null }));
const { UpdateService } = await import('../../src/services/updateService');
const { useUpdateCheck } = await import('../../src/hooks/useUpdateCheck');
const { UpdateDialog } = await import('../../src/components/UpdateDialog');
const { showUpdateNotification } = await import('../../src/components/UpdateNotification');
const { About } = await import('../../src/components/About');
const { UpdateCheckProvider } = await import('../../src/components/UpdateCheckProvider');

afterAll(() => {
  mock.module('@tauri-apps/api/app', () => originals.app);
  mock.module('@tauri-apps/plugin-updater', () => originals.updater);
  mock.module('@tauri-apps/plugin-process', () => originals.process);
  mock.module('sonner', () => originals.sonner);
  mock.module('../../src/components/ui/dialog', () => originals.dialog);
  mock.module('next/image', () => originals.image);
  mock.module('../../src/components/AnalyticsConsentSwitch', () => originals.consent);
});
let view: ReactTestRenderer | undefined;
const originalSetTimeout = globalThis.setTimeout;
const originalClearTimeout = globalThis.clearTimeout;
const originalWindow = globalThis.window;
const timers = new Map<number, () => void>();
beforeEach(() => {
  readVersion = async () => '0.4.1';
  getVersion.mockClear(); check.mockClear(); relaunch.mockClear(); notifyInfo.mockClear(); notifySuccess.mockClear(); notifyError.mockClear(); timers.clear();
  globalThis.setTimeout = ((callback: () => void) => { const id = timers.size + 1; timers.set(id, callback); return id; }) as typeof setTimeout;
  globalThis.clearTimeout = ((id: number) => { timers.delete(id); }) as typeof clearTimeout;
  globalThis.window = new EventTarget() as unknown as Window & typeof globalThis;
});
afterEach(async () => {
  if (view) await act(async () => view!.unmount());
  view = undefined;
  globalThis.setTimeout = originalSetTimeout;
  globalThis.clearTimeout = originalClearTimeout;
  globalThis.window = originalWindow;
});
const text = () => JSON.stringify(view!.toJSON());

test('normal and forced service checks report manual installation without contacting community feed', async () => {
  const service = new UpdateService();
  for (const forced of [false, true]) {
    const result = await service.checkForUpdates(forced);
    expect(result).toMatchObject({ available: false, currentVersion: '0.4.1', status: 'manual-install-required' });
  }
  expect(check).not.toHaveBeenCalled();
  expect(service.wasCheckedRecently()).toBe(false);
});

test('direct service installation rejects even a supplied legacy update object before side effects', async () => {
  const download = mock(async () => {});
  const install = mock(async () => {});
  const onProgress = mock(() => {});
  await expect(new UpdateService().downloadAndInstall({ download, install } as unknown as Update, onProgress)).rejects.toThrow('内部安装包');
  expect(download).not.toHaveBeenCalled();
  expect(install).not.toHaveBeenCalled();
  expect(relaunch).not.toHaveBeenCalled();
  expect(onProgress).not.toHaveBeenCalled();
});

test('startup hook schedules no update timer and an explicit request returns manual policy without update notifications', async () => {
  let state!: ReturnType<typeof useUpdateCheck>;
  const onAvailable = mock(() => {});
  function Consumer() { state = useUpdateCheck({ onUpdateAvailable: onAvailable }); return null; }
  await act(async () => { view = create(<Consumer />); });
  expect(timers.size).toBe(0);
  await act(async () => state.checkForUpdates(true));
  expect(state.updateInfo).toMatchObject({ available: false, status: 'manual-install-required' });
  expect(onAvailable).not.toHaveBeenCalled();
  expect(notifyInfo).not.toHaveBeenCalled();
  expect(check).not.toHaveBeenCalled();
});

test('opening a dialog with stale available metadata explains manual installation and offers no install action', async () => {
  await act(async () => { view = create(<UpdateDialog open onOpenChange={() => {}} updateInfo={{ available: true, currentVersion: '0.4.1', version: '99.0.0' }} />); });
  expect(text()).toContain('内部安装包');
  expect(text()).toContain('未检查');
  expect(text()).not.toContain('99.0.0');
  expect(text()).not.toContain('Download & Install');
  expect(check).not.toHaveBeenCalled();
});

test('direct notification cannot advertise an untrusted community update', () => {
  showUpdateNotification({ available: true, currentVersion: '0.4.1', version: '99.0.0' });
  expect(notifyInfo).not.toHaveBeenCalled();
});

test('About action displays internal package instructions without asserting latest version', async () => {
  await act(async () => { view = create(<About />); });
  const button = view!.root.findAllByType('button').find(node => node.props.onClick && node.children.some(child => typeof child === 'string' && /update|更新/i.test(child)));
  expect(button).toBeDefined();
  await act(async () => button!.props.onClick());
  expect(text()).toContain('内部安装包');
  expect(text()).toContain('未检查');
  expect(text()).toContain('0.4.1');
  expect(notifySuccess).not.toHaveBeenCalled();
  expect(check).not.toHaveBeenCalled();
});

test('About version read failure shows unknown without inventing a version and keeps update instructions actionable', async () => {
  readVersion = async () => { throw new Error('Native version unavailable'); };
  await act(async () => { view = create(<About />); });
  expect(text()).toContain('版本未知');
  expect(text()).not.toContain('版本读取中');
  expect(text()).not.toContain('0.4.1');
  const button = view!.root.findAllByType('button').find(node => node.children.some(child => child === '更新说明'));
  expect(button).toBeDefined();
  expect(button!.props.disabled).toBe(false);
  await act(async () => button!.props.onClick());
  expect(text()).toContain('内部安装包');
  expect(text()).not.toContain('0.4.1');
  expect(notifyError).toHaveBeenCalledWith(expect.stringContaining('Native version unavailable'));
  expect(notifySuccess).not.toHaveBeenCalled();
  expect(check).not.toHaveBeenCalled();
});

test('tray update action opens manual guidance even before any update check result exists', async () => {
  await act(async () => { view = create(<UpdateCheckProvider><span>application</span></UpdateCheckProvider>); });
  await act(async () => { window.dispatchEvent(new Event('check-updates-from-tray')); });
  expect(text()).toContain('内部安装包');
  expect(text()).toContain('未检查');
  expect(check).not.toHaveBeenCalled();
});

test('native configuration denies direct updater access and cannot resolve the community signing key or feed', () => {
  const config = JSON.parse(readFileSync(new URL('../../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
  expect(config.identifier).toBe('com.meetily.ai');
  expect(config.version).toBe('0.4.1');
  expect(config.bundle.createUpdaterArtifacts).toBe(false);
  expect(config.plugins?.updater).toBeUndefined();
  const permissions = config.app.security.capabilities.flatMap((capability: { permissions: unknown[] }) => capability.permissions);
  expect(permissions.some((permission: unknown) => typeof permission === 'string' && permission.startsWith('updater:'))).toBe(false);
});
