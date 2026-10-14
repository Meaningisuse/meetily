'use client';
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Switch } from './ui/switch';
type Discovery = { pid: number; instance_id: string; base_url: string; protocol_version: number };
type Client = { id: string; name: string; scopes: string[]; expires_at: number; revoked: boolean };
const errorText = (error: unknown) => typeof error === 'object' && error !== null && 'message' in error ? String(error.message) : String(error);
export function AutomationSettings({ invokeNative = invoke }: { invokeNative?: typeof invoke } = {}) {
 const [discovery,setDiscovery]=useState<Discovery|null>(null);
 const [clients,setClients]=useState<Client[]>([]);
 const [name,setName]=useState('');const [days,setDays]=useState(7);
 const [scope,setScope]=useState<'read'|'write'|'record'>('read');
 const [oneTime,setOneTime]=useState<{client:Client;token:string}|null>(null);
 const [busy,setBusy]=useState(false);const [error,setError]=useState('');
 useEffect(()=>{let mounted=true;Promise.all([invokeNative<Discovery|null>('automation_status'),invokeNative<Client[]>('automation_list_clients')]).then(([server,list])=>{if(mounted){setDiscovery(server);setClients(list);}}).catch(e=>{if(mounted)setError(errorText(e));});return()=>{mounted=false;};},[]);
 const refresh=async()=>setClients(await invokeNative<Client[]>('automation_list_clients'));
 const toggle=async(enabled:boolean)=>{setBusy(true);setError('');try{setDiscovery(await invokeNative<Discovery|null>('automation_set_enabled',{enabled}));if(!enabled)setOneTime(null);}catch(e){setError(errorText(e));}finally{setBusy(false);}};
 const create=async()=>{setBusy(true);setError('');setOneTime(null);try{const value=await invokeNative<{client:Client;token:string}>('automation_create_client',{name,scopes:[scope],expiresAt:Math.floor(Date.now()/1000)+days*86400});setOneTime(value);setName('');await refresh();}catch(e){setError(errorText(e));}finally{setBusy(false);}};
 const scopeDisclosure=scope==='read'?'Read access includes meeting text and summaries for all current meetings. It excludes audio and model credentials.':scope==='write'?'Write access can start summary jobs and cancel them. It cannot control recording or export files.':'Record access can start and stop recording through the same local desktop recording session. It cannot read meeting data or start summary jobs.';
 const revoke=async(id:string)=>{setBusy(true);setError('');try{await invokeNative('automation_revoke_client',{clientId:id});if(oneTime?.client.id===id)setOneTime(null);await refresh();}catch(e){setError(errorText(e));}finally{setBusy(false);}};
 return <section className="bg-white rounded-lg border p-6 space-y-4" aria-labelledby="automation-heading">
  <div className="flex items-center justify-between"><h2 id="automation-heading" className="text-lg font-semibold">Local automation</h2><Switch aria-label="Enable local automation" checked={!!discovery} disabled={busy} onCheckedChange={toggle}/></div>
  <p className="text-sm text-gray-600">Off by default. Create separate least-privilege credentials for each client. {scopeDisclosure}</p>
  {error&&<p role="alert" className="text-sm text-red-700">{error}</p>}
  {discovery&&<><p className="text-sm break-all">Local endpoint: <code>{discovery.base_url}</code></p>
   <div className="flex flex-wrap gap-2"><input aria-label="Client name" value={name} maxLength={100} onChange={e=>setName(e.target.value)} placeholder="Client name" className="border rounded px-3 py-2"/><select aria-label="Client permission" value={scope} onChange={e=>setScope(e.target.value as 'read'|'write'|'record')} className="border rounded px-2"><option value="read">Read</option><option value="write">Write</option><option value="record">Record</option></select><select aria-label="Credential expiry" value={days} onChange={e=>setDays(Number(e.target.value))} className="border rounded px-2"><option value={1}>1 day</option><option value={7}>7 days</option><option value={30}>30 days</option></select><button disabled={busy||!name.trim()} onClick={create} className="border rounded px-3 py-2 disabled:opacity-50">Create {scope} client</button></div>
   {oneTime&&<div className="border border-amber-300 rounded p-3 space-y-2"><p>Copy this credential now. It is shown only once and is also stored in your system Keychain.</p><p className="text-sm">Client ID: {oneTime.client.id}</p><textarea aria-label="One-time credential" readOnly value={oneTime.token} className="w-full border rounded p-2 font-mono"/><button onClick={()=>setOneTime(null)} className="border rounded px-3 py-1">I have saved it</button></div>}
  </>}
  <ul className="space-y-2">{clients.map(client=><li key={client.id} className="flex items-center justify-between gap-2 text-sm"><span>{client.name} · {client.scopes.join(', ')} · {client.revoked?'Revoked':client.expires_at<=Date.now()/1000?'Expired':`Expires ${new Date(client.expires_at*1000).toLocaleDateString()}`}</span>{!client.revoked&&<button disabled={busy} onClick={()=>revoke(client.id)} className="border rounded px-3 py-1">Revoke</button>}</li>)}</ul>
 </section>;
}
