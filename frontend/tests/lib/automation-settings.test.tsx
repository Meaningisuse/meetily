import {beforeEach,expect,mock,test} from 'bun:test';
import {act,create} from 'react-test-renderer';
const original={...await import('@tauri-apps/api/core')};
const token='a'.repeat(64);const client={id:'client-1',name:'internal reader',scopes:['read'],expires_at:9999999999,revoked:false};let created=false;let selectedScopes:string[]=[];
beforeEach(()=>{created=false;selectedScopes=[];invoke.mockClear();});
const invoke=mock(async(command:string,args?:any)=>{
 if(command==='automation_status')return null;
 if(command==='automation_list_clients')return created?[client]:[];
 if(command==='automation_set_enabled')return args.enabled?{pid:1,instance_id:'instance',protocol_version:1,base_url:'http://127.0.0.1:12345/api/internal/v1'}:null;
 if(command==='automation_create_client'){selectedScopes=args.scopes;created=true;return {client:{...client,scopes:args.scopes},token};}
 if(command==='automation_revoke_client')return null;
 throw new Error(command);
});
const {AutomationSettings}=await import('../../src/components/AutomationSettings');
test('read permission disclosure, one-time token dismissal, and disable use native state',async()=>{
 let view:any;await act(async()=>{view=create(<AutomationSettings invokeNative={invoke as typeof original.invoke}/>);});
 try{
 expect(JSON.stringify(view.toJSON()).toLowerCase()).toContain('all current meetings');
 await act(async()=>{view.root.findByProps({'aria-label':'Enable local automation'}).props.onCheckedChange(true);});
 await act(async()=>view.root.findByProps({'aria-label':'Client name'}).props.onChange({target:{value:'internal reader'}}));
 const button=(label:string)=>view.root.findAllByType('button').find((b:any)=>b.children.join('')===label);
 await act(async()=>button('Create read client').props.onClick());
 expect(view.root.findByProps({'aria-label':'One-time credential'}).props.value).toBe(token);
 await act(async()=>button('I have saved it').props.onClick());
 expect(view.root.findAllByProps({'aria-label':'One-time credential'})).toHaveLength(0);
 await act(async()=>view.root.findByProps({'aria-label':'Enable local automation'}).props.onCheckedChange(false));
 expect(invoke.mock.calls.filter(([c])=>c==='automation_create_client')).toHaveLength(1);
 }finally{await act(async()=>view.unmount());}
});

test('record permission is explicit and stored as a separate credential scope',async()=>{
 let view:any;await act(async()=>{view=create(<AutomationSettings invokeNative={invoke as typeof original.invoke}/>);});
 try{
  await act(async()=>{view.root.findByProps({'aria-label':'Enable local automation'}).props.onCheckedChange(true);});
  await act(async()=>view.root.findByProps({'aria-label':'Client name'}).props.onChange({target:{value:'recording assistant'}}));
  await act(async()=>view.root.findByProps({'aria-label':'Client permission'}).props.onChange({target:{value:'record'}}));
  expect(JSON.stringify(view.toJSON())).toContain('Record access can start and stop recording');
  const createButton=view.root.findAllByType('button').find((b:any)=>b.children.join('')==='Create record client');
  await act(async()=>createButton.props.onClick());
  expect(selectedScopes).toEqual(['record']);
 }finally{await act(async()=>view.unmount());}
});
