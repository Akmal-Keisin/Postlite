import assert from 'node:assert/strict';
import { evaluate, close, reload } from './tier1-cdp.mjs';

const results = [];
const check = async (name, expression) => {
  const value = await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  results.push(name);
  console.log(`PASS ${name}`);
};
const installErrorTrap = () => evaluate(`(() => {
  window.__runtimeErrors=[]; window.addEventListener('error', e=>__runtimeErrors.push(e.message));
})()`);
const resetState = () => evaluate(`(() => {
  const v=__verify;
  v.state.collections=[]; v.state.environments=[]; v.state.tabs=[]; v.state.activeTabId=null;
  v.state.settings=v.normalizeSettings(); v.applyAppSettings(); v.persist();
})()`);
const click = (selector) => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
const settle = () => evaluate('__verify.flushWorkspace().then(()=>true)');
// Dialog close handlers run asynchronously and cascade through saveToCollection /
// persist / openExampleDialog, so a zero-delay timer is not a sufficient sync.
const waitFor = (expression, timeout = 2000) => evaluate(`new Promise((resolve, reject) => {
  const deadline = Date.now() + ${timeout};
  const tick = () => {
    let ok = false;
    try { ok = !!(${expression}); } catch {}
    if (ok) { resolve(true); return; }
    if (Date.now() >= deadline) { reject(new Error('waitFor timed out')); return; }
    setTimeout(tick, 25);
  };
  tick();
})`);

try {
  await reload();
  // Fail fast if this harness is ever pointed at a stubbed/shimmed environment.
  await check('running against the real isolated Tauri app',
    `(await __TAURI__.path.appDataDir()).includes('app.postlite.tier3verify20261116')`);
  await installErrorTrap();
  await resetState();

  await check('examples default to an empty array',
    `JSON.stringify(__verify.newRequest({}).examples)==='[]'`);
  await check('example names are trimmed and capped at 80 chars',
    `__verify.normExamples([{name:'  '+('x'.repeat(100))+'  '}])[0].name.length===80 && __verify.normExamples([{}])[0].name==='Example'`);
  await check('malformed examples are dropped and fields normalized',
    `(() => {const ex=__verify.normExamples([null,7,{status:'201',headers:[{key:'X',value:3}],body:'ok',elapsedMs:'12'}]);return ex.length===1 && ex[0].status===201 && ex[0].headers[0].value==='3' && ex[0].body==='ok' && ex[0].elapsedMs===12 && typeof ex[0].id==='string';})()`);

  // Guard regression: an old tab with no `examples` field must not wipe stored examples.
  await check('saveInPlace preserves stored examples for a tab missing the field', `(async () => {
    const v=__verify;
    v.state.collections=[]; v.state.environments=[]; v.state.tabs=[]; v.state.activeTabId=null;
    const req=v.newRequest({name:'Guard', examples:[{name:'Kept', body:'original'}]});
    v.state.collections=[v.newCollection('C', {items:[structuredClone(req)]})];
    const tab={id:'old-tab', req:structuredClone(req), response:null};
    delete tab.req.examples; // old tab persisted before examples existed
    v.state.tabs=[tab]; v.state.activeTabId=tab.id;
    const saved=v.saveInPlace(tab.req);
    const stored=v.state.collections[0].items[0];
    return saved===true && Array.isArray(stored.examples) && stored.examples.length===1
      && stored.examples[0].name==='Kept' && stored.examples[0].body==='original';
  })()`);

  // An explicit [] is an intentional deletion, not an absent field.
  await check('explicit empty examples clear stored examples on save', `(async () => {
    const v=__verify;
    v.state.collections=[]; v.state.tabs=[]; v.state.activeTabId=null;
    const req=v.newRequest({name:'Clear', examples:[{name:'Old', body:'gone'}]});
    v.state.collections=[v.newCollection('C', {items:[structuredClone(req)]})];
    const tab={id:'clear-tab', req:structuredClone(req), response:null};
    v.state.tabs=[tab]; v.state.activeTabId=tab.id;
    tab.req.examples=[]; // deliberate deletion of every example
    const saved=v.saveInPlace(tab.req);
    const stored=v.state.collections[0].items[0];
    return saved===true && Array.isArray(stored.examples) && stored.examples.length===0;
  })()`);

  // Hydration must copy stored examples into an old tab that has no field.
  await check('hydrateWorkspace inherits stored examples for a tab missing the field', `(async () => {
    const v=__verify;
    const stored=v.newRequest({name:'OldTab', examples:[{name:'Inherited', body:'kept'}]});
    const tabReq=structuredClone(stored); delete tabReq.examples;
    const payload={
      settings:v.normalizeSettings(), activeWorkspaceId:'ws-old',
      workspaces:[{ id:'ws-old', name:'Old', collections:[{id:'c-old', name:'C', items:[structuredClone(stored)]}], environments:[], tabs:[tabReq], activeTabIndex:0 }],
    };
    v.applySavedPayload(payload, 'replace');
    v.enterWorkspace(v.state.workspaces.find(w=>w.id===v.state.activeWorkspaceId));
    const tab=v.state.tabs[0];
    return !!tab && Array.isArray(tab.req.examples) && tab.req.examples.length===1 && tab.req.examples[0].name==='Inherited';
  })()`);

  // Persisted payload (and the disk round trip) must carry examples.
  await check('examples are present in workspacePayload after persist and flush', `(async () => {
    const v=__verify;
    v.state.collections=[]; v.state.environments=[]; v.state.tabs=[]; v.state.activeTabId=null;
    const req=v.newRequest({name:'Persist', examples:[{name:'Persisted', body:'data'}]});
    v.state.collections=[v.newCollection('C', {items:[structuredClone(req)]})];
    const tab={id:'persist-tab', req:structuredClone(req), response:null};
    v.state.tabs=[tab]; v.state.activeTabId=tab.id;
    v.persist();
    await v.flushWorkspace();
    const ws=v.workspacePayload().workspaces.find(w=>w.id===v.state.activeWorkspaceId);
    const disk=JSON.parse(await __TAURI__.core.invoke('load_workspace')).workspaces
      .find(w=>w.id===v.state.activeWorkspaceId);
    return !!ws && ws.collections[0].items[0].examples?.[0]?.name==='Persisted'
      && ws.tabs[0].examples?.[0]?.name==='Persisted'
      && !!disk && disk.collections[0].items[0].examples?.[0]?.name==='Persisted'
      && disk.tabs[0].examples?.[0]?.name==='Persisted';
  })()`);

  // Real page reload proves the examples survive the full save/load cycle.
  await reload();
  await check('examples survive a page reload', `(() => {
    const v=__verify;
    const node=v.state.collections[0]?.items[0];
    const tab=v.state.tabs.find(t=>t.req?.name==='Persist');
    return !!node && node.examples?.[0]?.name==='Persisted'
      && !!tab && tab.req.examples?.[0]?.name==='Persisted';
  })()`);
  // Reinstall the error trap and reset harness state so later task checks start clean.
  await installErrorTrap();
  await resetState();
  await check('runtime error trap is reinstalled after reload', `Array.isArray(window.__runtimeErrors) && window.__runtimeErrors.length===0`);

  // Hook exposure: existing harnesses still see every implemented hook; names
  // not implemented yet resolve to undefined instead of aborting the object.
  await check('all exposed hooks resolve (implemented defined, future ones undefined)', `(() => {
    const v=__verify;
    const future=['viewExample','exitExampleView','deleteExample','renderJsonTreeInto'];
    return Object.keys(v).length>0
      && Object.keys(v).every(n=>future.includes(n) ? v[n]===undefined : typeof v[n]!=='undefined')
      && ['state','persist','flushWorkspace','workspacePayload','normExamples','saveInPlace','applySavedPayload','render'].every(n=>typeof v[n]!=='undefined')
      && typeof __verify.normPostmanExample==='function';
  })()`);
  await check('production code has no no-op example globals', `['viewExample','exitExampleView','deleteExample','renderJsonTreeInto'].every(n=>typeof window[n]==='undefined')`);
  await check('save-example hooks are implemented as real functions', `['saveExample','commitExample','openExampleDialog','renderExamples'].every(n=>typeof __verify[n]==='function')`);

  await check('Postman response examples import into request.examples',
    `(() => {const c=__verify.fromPostman({info:{name:'x'},item:[{name:'r',request:'http://x',response:[{name:'OK',code:200,status:'OK',body:'{"a":1}',header:[{key:'Content-Type',value:'application/json'}],responseTime:9,size:7,time:'2026-01-01T00:00:00.000Z'}]}]});const ex=c.items[0].examples;return ex.length===1 && ex[0].name==='OK' && ex[0].status===200 && ex[0].statusText==='OK' && ex[0].body==='{"a":1}' && ex[0].elapsedMs===9 && ex[0].headers[0].key==='Content-Type';})()`);
  await check('string headers and missing fields import safely',
    `(() => {const c=__verify.fromPostman({item:[{name:'r',request:'http://x',response:[{header:['A: b','c'],body:null}]}]});const ex=c.items[0].examples[0];return ex.headers.length===2 && ex.headers[0].key==='A' && ex.headers[0].value==='b' && ex.headers[1].key==='c' && ex.headers[1].value==='' && ex.body==='' && ex.name==='Example';})()`);
  await check('examples export to item.response with Postman fields',
    `(() => {const r=__verify.newRequest({name:'r',examples:[{name:'OK',status:200,statusText:'OK',body:'{"a":1}',headers:[{key:'Content-Type',value:'application/json'}],elapsedMs:9,savedAt:'2026-01-01T00:00:00.000Z',sizeBytes:7}]});const c=__verify.newCollection('c',{items:[r]});const ex=__verify.toPostmanCollection(c).item[0].response;return ex.length===1 && ex[0].name==='OK' && ex[0].code===200 && ex[0].status==='OK' && ex[0].body==='{"a":1}' && ex[0].responseTime===9 && ex[0].size===7 && ex[0]._postman_previewlanguage==='json' && ex[0].originalRequest.method===r.method && ex[0].header[0].key==='Content-Type';})()`);
  await check('export round-trip preserves example name, body and status',
    `(() => {const r=__verify.newRequest({name:'r',examples:[{name:'Created',status:201,statusText:'Created',body:'{"id":1}'}]});const c=__verify.newCollection('c',{items:[r]});const back=__verify.fromPostman(JSON.parse(JSON.stringify(__verify.toPostmanCollection(c))));const ex=back.items[0].examples[0];return ex.name==='Created' && ex.status===201 && ex.statusText==='Created' && ex.body==='{"id":1}';})()`);
  await check('malformed Postman response entries are dropped on import',
    `(() => {const c=__verify.fromPostman({item:[{name:'r',request:'http://x',response:[null,7,{name:'Real'}]}]});const ex=c.items[0].examples;return ex.length===1 && ex[0].name==='Real';})()`);
  await check('preview language respects non-JSON content types',
    `(() => {const lang=(ct)=>{const r=__verify.newRequest({name:'r',examples:[{name:'e',status:200,body:'{"a":1}',headers:[{key:'Content-Type',value:ct}]}]});const c=__verify.newCollection('c',{items:[r]});return __verify.toPostmanCollection(c).item[0].response[0]._postman_previewlanguage;};return lang('text/html')==='text' && lang('application/xml')==='xml' && lang('application/javascript')==='javascript';})()`);

  await check('example buttons, popover, banner and dialog exist',
    `['#save-example','#examples-btn','#examples-pop','#examples-list','#example-banner','#example-banner-name','#example-exit','#example-dialog','#example-name','#json-tools','#json-collapse-all','#json-expand-all'].every(s=>!!document.querySelector(s))
      && document.querySelector('#examples-btn').getAttribute('aria-controls')==='examples-pop'
      && !document.querySelector('#examples-btn').hasAttribute('aria-haspopup')
      && document.querySelector('#examples-pop').getAttribute('role')==='group'
      && document.querySelector('#examples-pop').getAttribute('aria-label')==='Saved examples'
      && !document.querySelector('#example-banner').hasAttribute('role')
      && document.querySelector('#example-banner [role="status"][aria-live="polite"]')?.textContent.includes('Viewing example:')
      && !document.querySelector('#example-banner [role="status"]').contains(document.querySelector('#example-exit'))
      && document.querySelector('#json-tools').getAttribute('aria-controls')==='pretty'`);
  await check('example controls start hidden/disabled appropriately',
    `document.querySelector('#examples-btn').hidden && document.querySelector('#examples-pop').hidden && document.querySelector('#example-banner').hidden && document.querySelector('#json-tools').hidden && document.querySelector('#save-example').disabled`);

  // Task 4: saving a response as a named example, including the chain into the
  // save-request dialog when the tab is not yet stored in a collection.
  await check('save example is disabled before a response', `document.querySelector('#save-example').disabled`);
  await evaluate(`(() => {const v=__verify; const t=v.addTab(v.newRequest({name:'Save me',url:'http://127.0.0.1:8100/api/echo'})); t.response={status:200,status_text:'OK',headers:[{key:'Content-Type',value:'application/json'}],body:'{"ok":true}',elapsed_ms:7,size_bytes:9,truncated:false,note:null,results:[],limitKB:2000}; v.renderResponse(); return true;})()`);
  await check('save example enables after a successful response', `!document.querySelector('#save-example').disabled`);
  await click('#save-example');
  await check('unsaved request opens save dialog first', `document.querySelector('#save-dialog').open`);
  await evaluate(`(() => {const d=document.querySelector('#save-dialog'); d.querySelector('#save-name').value='Example target'; d.close('save'); return true;})()`);
  await waitFor(`document.querySelector('#example-dialog').open`);
  await check('saving the request chains into the example dialog', `document.querySelector('#example-dialog').open && __verify.state.collections.length===1`);
  await evaluate(`(() => {document.querySelector('#example-name').value='Happy path'; document.querySelector('#example-dialog').close('save'); return true;})()`);
  await waitFor(`__verify.state.collections[0]?.items[0]?.examples?.length===1`);
  await check('example is saved on both tab request and stored node',
    `(() => {const t=__verify.state.tabs[0]; const stored=__verify.state.collections[0].items[0]; return t.req.examples.length===1 && stored.examples.length===1 && stored.examples[0].name==='Happy path' && stored.examples[0].body==='{"ok":true}' && stored.examples[0].status===200;})()`);
  await settle();
  await check('examples button shows a count', `document.querySelector('#examples-btn').hidden===false && document.querySelector('#examples-count').textContent==='1'`);

  console.log(`\n${results.length} checks passed`);
} finally {
  await close();
}
