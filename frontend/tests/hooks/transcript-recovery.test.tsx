import { afterAll, beforeEach, expect, mock, test } from 'bun:test';
import { act, create } from 'react-test-renderer';
const core={...await import('@tauri-apps/api/core')};
const originalDB={...await import('../../src/services/indexedDBService')};
const originalStorage={...await import('../../src/services/storageService')};
const originalLanguage={...await import('../../src/lib/summary-language-preferences')};
let failed=false;
const markSaved=mock(async()=>{});
const save=mock(async(..._args:any[])=>({meeting_id:'saved'}));
const invoke=mock(async(command:string)=>{
  if(command==='recover_transcription_backlog'){
    if(failed)throw new Error('Checksum failed; source retained');
    return [{text:'last action',sequence_id:2,timestamp:'00:02',confidence:1,audio_start_time:2,audio_end_time:3,duration:1}];
  }
  if(command==='has_audio_checkpoints')return false;
  if(command==='recover_audio_from_checkpoints')return {status:'success'};
  if(command==='cleanup_checkpoints')return null;
  throw new Error(command);
});
mock.module('@tauri-apps/api/core',()=>({...core,invoke}));
mock.module('../../src/services/indexedDBService',()=>({indexedDBService:{
  getAllMeetings:async()=>[{meetingId:'item',title:'meeting',folderPath:'/recordings/test',lastUpdated:Date.now()-60000}],
  getMeetingMetadata:async()=>({meetingId:'item',title:'meeting',folderPath:'/recordings/test'}),
  getTranscripts:async()=>[{text:'first',sequenceId:1,timestamp:'00:01',confidence:1}],
  markMeetingSaved:markSaved,
}}));
mock.module('../../src/services/storageService',()=>({storageService:{saveMeeting:save}}));
mock.module('../../src/lib/summary-language-preferences',()=>({...originalLanguage,applyPinnedSummaryLanguageToMeeting:async()=>{}}));
const {useTranscriptRecovery}=await import('../../src/hooks/useTranscriptRecovery');
let hook:ReturnType<typeof useTranscriptRecovery>;
function View(){hook=useTranscriptRecovery();return null;}
beforeEach(()=>{failed=false;save.mockClear();markSaved.mockClear();invoke.mockClear();});
afterAll(()=>{
 mock.module('@tauri-apps/api/core',()=>core);mock.module('../../src/services/indexedDBService',()=>originalDB);
 mock.module('../../src/services/storageService',()=>originalStorage);mock.module('../../src/lib/summary-language-preferences',()=>originalLanguage);
});
test('native replay includes the durable last action before meeting acceptance',async()=>{
 let view:any;await act(async()=>{view=create(<View/>);});
 try{await act(async()=>{await hook.recoverMeeting('item');});
 expect(save.mock.calls[0][1].map((t:any)=>t.text)).toEqual(['first','last action']);expect(markSaved).toHaveBeenCalledTimes(1);
 }finally{await act(async()=>view.unmount());}
});
test('failed native replay never marks recovery saved or deletes checkpoints',async()=>{
 failed=true;let view:any;await act(async()=>{view=create(<View/>);});
 try{await act(async()=>{await expect(hook.recoverMeeting('item')).rejects.toThrow('Checksum');});
 expect(save).not.toHaveBeenCalled();expect(markSaved).not.toHaveBeenCalled();expect(invoke.mock.calls.some(([c])=>c==='cleanup_checkpoints')).toBe(false);
 }finally{await act(async()=>view.unmount());}
});

test('a missing audio checkpoint must not hide the durable transcript folder',async()=>{
 let view:any;await act(async()=>{view=create(<View/>);});
 try{await act(async()=>{await hook.checkForRecoverableTranscripts();});expect(hook.recoverableMeetings[0].folderPath).toBe('/recordings/test');}
 finally{await act(async()=>view.unmount());}
});
