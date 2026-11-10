import assert from 'node:assert/strict';
import {mkdirSync,rmdirSync,readFileSync} from 'node:fs';
import {join} from 'node:path';
import {cmd,evaluate,reload,close} from './tier1-cdp.mjs';
let staging;
let created=false;
try {
 await reload();
 const dir=await evaluate('__TAURI__.path.appDataDir()');
 assert.ok(dir.endsWith('app.postlite.tier1verify20261110'));
 await evaluate(`(async()=>{const v=__verify;v.state.settings.general.autoSave=false;v.state.settings.general.restoreLastSession=true;v.addTab(v.newRequest({url:'http://127.0.0.1:8100/api/echo'}));v.persist();await v.flushWorkspace();v.openAppSettings();})()`);
 staging=join(dir,'workspace.json.staging');
 mkdirSync(staging);created=true;
 await evaluate(`__TAURI__.event.emitTo('main','tauri://close-requested')`);
 const failed=await evaluate(`new Promise((resolve,reject)=>{const start=Date.now();const t=setInterval(()=>{if(document.querySelector('#error').textContent.includes('Could not close safely')){clearInterval(t);resolve(!document.body.inert && document.querySelector('#preferences-message').textContent.includes('Could not close safely'));}else if(Date.now()-start>5000){clearInterval(t);reject(new Error('close failure not surfaced'));}},30);})`);
 assert.equal(failed,true);
 console.log('PASS failed close leaves window usable and displays save error');
 rmdirSync(staging);created=false;
 await evaluate(`document.querySelector('#app-settings-dialog').close();document.querySelector('#url').value='http://127.0.0.1:8100/close-flush-marker';document.querySelector('#url').dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('#url').focus();`);
 assert.equal(await evaluate(`document.querySelector('#save-state').textContent==='Unsaved'`),true);
 await cmd('Runtime.evaluate',{expression:`void __TAURI__.event.emitTo('main','tauri://close-requested')`});
 const deadline=Date.now()+5000;
 let done=false;
 while(Date.now()<deadline){
  try{const targets=await(await fetch('http://127.0.0.1:9235/json/list')).json();if(!targets.some(t=>t.type==='page')){done=true;break;}}
  catch{done=true;break;}
  await new Promise(r=>setTimeout(r,50));
 }
 assert.ok(done,'native window must close after flush');
 const saved=JSON.parse(readFileSync(join(dir,'workspace.json'),'utf8'));
 assert.equal(saved.settings.general.autoSave,false);
 assert.ok(saved.workspaces.some(w=>w.tabs.some(t=>t.url.endsWith('/close-flush-marker'))));
 console.log('PASS native close flushes focused typing with autosave off before destroying window');
}finally{
 if(created)rmdirSync(staging);
 close();
}
