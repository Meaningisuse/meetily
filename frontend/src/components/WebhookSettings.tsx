'use client';
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

type Webhook = { id: string; url: string; kinds: string[]; enabled: boolean; created_at: number };
type Created = { webhook: Webhook; secret: string };
const kinds = ['recording.stopped', 'transcript.completed', 'summary.completed', 'summary.failed'];
const errorText = (error: unknown) => typeof error === 'object' && error !== null && 'message' in error ? String(error.message) : String(error);

export function WebhookSettings({ invokeNative = invoke }: { invokeNative?: typeof invoke } = {}) {
  const [items, setItems] = useState<Webhook[]>([]);
  const [url, setUrl] = useState('');
  const [approvals, setApprovals] = useState('');
  const [selected, setSelected] = useState<string[]>(['transcript.completed', 'summary.completed', 'summary.failed']);
  const [created, setCreated] = useState<Created | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const refresh = async () => setItems(await invokeNative<Webhook[]>('automation_list_webhooks'));
  useEffect(() => { refresh().catch((e) => setError(errorText(e))); }, []);
  const create = async () => {
    setBusy(true); setError(''); setCreated(null);
    try {
      const value = await invokeNative<Created>('automation_create_webhook', {
        url,
        approvedHosts: approvals.split(',').map((item) => item.trim()).filter(Boolean),
        kinds: selected,
      });
      setCreated(value); setUrl(''); await refresh();
    } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  const setEnabled = async (id: string, enabled: boolean) => {
    setBusy(true); setError('');
    try { await invokeNative('automation_set_webhook_enabled', { id, enabled }); await refresh(); }
    catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  const remove = async (id: string) => {
    setBusy(true); setError('');
    try { await invokeNative('automation_delete_webhook', { id }); await refresh(); }
    catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  };
  return <div className="rounded border p-4 space-y-3" aria-labelledby="webhook-heading">
    <h3 id="webhook-heading" className="font-medium">Signed webhooks</h3>
    <p className="text-sm text-gray-600">HTTPS only. Approve the exact destination hostname; add a specific IP only for an intentionally private test endpoint. Secrets are stored in Keychain and shown once.</p>
    {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
    <label className="block text-sm">Endpoint URL<input aria-label="Webhook endpoint URL" value={url} onChange={(e) => setUrl(e.target.value)} className="mt-1 w-full border rounded px-3 py-2" placeholder="https://hooks.example.com/meetily" maxLength={2048}/></label>
    <label className="block text-sm">Approved hostnames or IPs, comma separated<input aria-label="Approved webhook targets" value={approvals} onChange={(e) => setApprovals(e.target.value)} className="mt-1 w-full border rounded px-3 py-2" placeholder="hooks.example.com" maxLength={2048}/></label>
    <fieldset className="flex flex-wrap gap-x-4 gap-y-2 text-sm"><legend className="mb-2">Events</legend>{kinds.map((kind) => <label key={kind} className="flex items-center gap-1"><input type="checkbox" checked={selected.includes(kind)} onChange={(e) => setSelected((old) => e.target.checked ? [...old, kind] : old.filter((v) => v !== kind))}/>{kind}</label>)}</fieldset>
    <button disabled={busy || !url.trim() || !selected.length} onClick={create} className="border rounded px-3 py-2 disabled:opacity-50">Add webhook and generate secret</button>
    {created && <div className="border border-amber-300 rounded p-3 space-y-2"><p className="text-sm">Copy this signing secret now. It will not be shown again.</p><code className="block break-all">{created.secret}</code><button className="border rounded px-3 py-1" onClick={() => setCreated(null)}>I saved the secret</button></div>}
    <ul className="space-y-2">{items.map((item) => <li key={item.id} className="flex flex-wrap items-center justify-between gap-2 border-t pt-2 text-sm"><span className="min-w-0 break-all">{item.url}<span className="block text-gray-500">{item.kinds.join(', ')}</span></span><span className="flex gap-2"><button disabled={busy} onClick={() => setEnabled(item.id, !item.enabled)} className="border rounded px-2 py-1">{item.enabled ? 'Disable' : 'Enable'}</button><button disabled={busy} onClick={() => remove(item.id)} className="border rounded px-2 py-1">Delete</button></span></li>)}</ul>
  </div>;
}
