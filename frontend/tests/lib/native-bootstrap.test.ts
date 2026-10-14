import { expect, test } from 'bun:test';
import { fetchNativeMeetings, fetchNativeConfiguration } from '../../src/lib/native-bootstrap';

test('native_boot_without_legacy_http_addresses loads meetings and model settings', async () => {
  const calls:string[]=[];
  const invoke=async <T>(command:string):Promise<T>=> {
    calls.push(command);
    const responses:Record<string,unknown>={api_native_ready:true,api_get_meetings:[{id:'m',title:'中文会议'}],api_get_model_config:{provider:'ollama',model:'local'},api_get_transcript_config:{provider:'whisper',model:'tiny'}};
    return responses[command] as T;
  };
  expect((await fetchNativeMeetings(invoke)).meetings?.[0].title).toBe('中文会议');
  expect((await fetchNativeConfiguration(invoke)).model?.provider).toBe('ollama');
  expect(calls).toEqual(['api_native_ready','api_get_meetings','api_get_model_config','api_get_transcript_config']);
});

test('native boot waits for a managed database and propagates initialization errors', async () => {
  const calls:string[]=[];
  expect(await fetchNativeMeetings(async <T>(command:string)=>{calls.push(command);return false as T;})).toEqual({ready:false,meetings:null});
  expect(calls).toEqual(['api_native_ready']);
  await expect(fetchNativeMeetings(async ()=>{throw new Error('database unavailable');})).rejects.toThrow('database unavailable');
});
