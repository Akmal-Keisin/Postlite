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

try {
  await reload();
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
    const future=['saveExample','commitExample','openExampleDialog','viewExample','exitExampleView','deleteExample','renderExamples','renderJsonTreeInto'];
    return Object.keys(v).length>0
      && Object.keys(v).every(n=>future.includes(n) ? v[n]===undefined : typeof v[n]!=='undefined')
      && ['state','persist','flushWorkspace','workspacePayload','normExamples','saveInPlace','applySavedPayload','render'].every(n=>typeof v[n]!=='undefined')
      && typeof __verify.normPostmanExample==='function';
  })()`);
  await check('production code has no no-op example globals', `['saveExample','commitExample','openExampleDialog','viewExample','exitExampleView','deleteExample','renderExamples','renderJsonTreeInto'].every(n=>typeof window[n]==='undefined')`);

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
    `(() => {const lang=(ct)=>{const r=__verify.newRequest({name:'r',examples:[{name:'e',status:200,body:'{"a":1}',headers:[{key:'Content-Type',value:ct}]}]});const c=__verify.newCollection('c',{items:[r]});return __verify.toPostmanCollection(c).item[0].response[0]._postman_previewlanguage;};return lang('text/html')==='text' && lang('application/xml')==='xml';})()`);

  console.log(`\n${results.length} checks passed`);
} finally {
  await close();
}
