/* §M smoke: masking on editable surfaces (header grid + Auth) and the copy path.
   Gate = a masked row is editable while masked, the eye reveals one row only, and copy
   yields real values. Run against the isolated debug build with tier1-server.mjs on 8100. */
import assert from 'node:assert/strict';
import { evaluate, reload, screenshot, close } from './tier1-cdp.mjs';

const lit = (v) => JSON.stringify(v);
let passed = 0;
const check = async (name, expression) => {
  const value = await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  passed++;
  console.log(`PASS ${name}`);
};
const click = (sel) => evaluate(`document.querySelector(${lit(sel)}).click()`);
const shot = async (name) => { await evaluate(`new Promise(r=>setTimeout(r,300))`); await screenshot(name); };
const wait = (ms) => evaluate(`new Promise(r=>setTimeout(r,${ms}))`);
const setReq = (fn) => evaluate(`(() => { const req=__verify.state.tabs.find(t=>t.id===__verify.state.activeTabId).req; (${fn})(req); __verify.render(); return true; })()`);
const headerRow = (key) => evaluate(`(() => { const tr=[...document.querySelectorAll('#headers-table tbody tr')].find(r=>r.querySelector('[data-field="key"]').value===${lit(key)}); if(!tr) return null; const i=tr.querySelector('[data-field="value"]'); const e=tr.querySelector('[data-act="reveal-row"]'); return { type:i.type, value:i.value, eye:!!e, eyeHidden:e?e.hidden:null, pressed:e?e.getAttribute('aria-pressed'):null }; })()`);

try {
  await reload();
  await check('isolated storage', `(await __TAURI__.path.appDataDir()).includes('app.postlite.tier1verify20261110')`);
  await evaluate(`(() => { __verify.state.settings.privacy.maskSensitiveHeaders=true; __verify.addTab(__verify.newRequest({ method:'GET', url:'http://127.0.0.1:8100/api/echo', headers:[
    {key:'Authorization',value:'Bearer tok-123',enabled:true},
    {key:'X-Api-Key',value:'key-abc',enabled:true},
    {key:'Content-Type',value:'application/json',enabled:true},
    {key:'X-Plain',value:'hello',enabled:true} ] })); return true; })()`);

  // --- Header grid masking follows sensitiveName ---
  let h = await headerRow('Authorization');
  assert.equal(h.type, 'password'); assert.equal(h.value, 'Bearer tok-123'); assert.equal(h.eye, true); assert.equal(h.eyeHidden, false);
  h = await headerRow('X-Api-Key');
  assert.equal(h.type, 'password', 'X-Api-Key contains "key" so it is sensitive');
  h = await headerRow('Content-Type');
  assert.equal(h.type, 'text'); assert.equal(h.eyeHidden, true, 'non-sensitive rows hide the eye');
  h = await headerRow('X-Plain');
  assert.equal(h.type, 'text');
  passed++; console.log('PASS sensitive headers masked, plain headers visible, eye only on masked');

  // --- A masked row is still editable in place ---
  await evaluate(`(() => { const tr=[...document.querySelectorAll('#headers-table tbody tr')].find(r=>r.querySelector('[data-field="key"]').value==='Authorization'); const i=tr.querySelector('[data-field="value"]'); i.value='Bearer edited'; i.dispatchEvent(new Event('input',{bubbles:true})); return true; })()`);
  h = await headerRow('Authorization');
  assert.equal(h.value, 'Bearer edited', 'typing into a masked cell updates the value');
  assert.equal(h.type, 'password', 'editing must not unmask the row');
  const stored = await evaluate(`__verify.state.tabs.find(t=>t.id===__verify.state.activeTabId).req.headers.find(x=>x.key==='Authorization').value`);
  assert.equal(stored, 'Bearer edited');
  passed++; console.log('PASS masked row editable while masked, value persisted');

  // --- Eye reveals one row only ---
  await click('#req-tabs .tab[data-tab="headers"]');
  await evaluate(`(() => { const tr=[...document.querySelectorAll('#headers-table tbody tr')].find(r=>r.querySelector('[data-field="key"]').value==='Authorization'); tr.querySelector('[data-act="reveal-row"]').click(); return true; })()`);
  h = await headerRow('Authorization');
  assert.equal(h.type, 'text'); assert.equal(h.pressed, 'true');
  const other = await headerRow('X-Api-Key');
  assert.equal(other.type, 'password', 'the eye must not reveal sibling rows');
  passed++; console.log('PASS eye reveals only its own row');
  await shot('tier2m-headers-revealed.png');

  // --- Reveal is not sticky: a re-render remasks ---
  await evaluate(`__verify.render()`);
  h = await headerRow('Authorization');
  assert.equal(h.type, 'password'); assert.equal(h.pressed, 'false');
  passed++; console.log('PASS reveal resets on re-render');

  // --- Renaming a key re-masks in place ---
  await evaluate(`(() => { const tr=[...document.querySelectorAll('#headers-table tbody tr')].find(r=>r.querySelector('[data-field="key"]').value==='X-Plain'); const k=tr.querySelector('[data-field="key"]'); k.value='Authorization'; k.dispatchEvent(new Event('input',{bubbles:true})); return true; })()`);
  h = await headerRow('Authorization');
  // two rows now named Authorization; the renamed one must be masked
  const renamed = await evaluate(`(() => { const trs=[...document.querySelectorAll('#headers-table tbody tr')].filter(r=>r.querySelector('[data-field="key"]').value==='Authorization'); return trs.map(r=>r.querySelector('[data-field="value"]').type); })()`);
  assert.ok(renamed.every(t => t === 'password'), `renaming into a sensitive name masks, got ${renamed}`);
  passed++; console.log('PASS renaming a header into a sensitive name re-masks in place');

  // --- Auth tab: secrets masked, identifiers visible ---
  await setReq(`req => { req.auth = { type:'bearer', bearer:{token:'secret-tok'}, basic:{username:'',password:''}, apikey:{key:'',value:'',addTo:'header'} }; }`);
  await click('#req-tabs .tab[data-tab="auth"]');
  await check('bearer token masked with eye', `(() => { const i=document.querySelector('#req-auth-fields [data-k="token"]'); const e=document.querySelector('#req-auth-fields [data-act="reveal-auth"]'); return i.type==='password' && i.value==='secret-tok' && e && !e.hidden; })()`);
  await evaluate(`document.querySelector('#req-auth-fields [data-act="reveal-auth"]').click()`);
  await check('auth eye reveals the token', `document.querySelector('#req-auth-fields [data-k="token"]').type==='text'`);

  await setReq(`req => { req.auth.type='basic'; req.auth.basic={username:'rina',password:'pw-secret'}; }`);
  await check('basic username visible, password masked', `(() => { const u=document.querySelector('#req-auth-fields [data-k="username"]'), p=document.querySelector('#req-auth-fields [data-k="password"]'); return u.type==='text' && p.type==='password'; })()`);
  await setReq(`req => { req.auth.type='apikey'; req.auth.apikey={key:'X-Key',value:'val-secret',addTo:'header'}; }`);
  await check('apikey Key visible, Value masked', `(() => { const k=document.querySelector('#req-auth-fields [data-k="key"]'), v=document.querySelector('#req-auth-fields [data-k="value"]'); return k.type==='text' && v.type==='password'; })()`);
  await shot('tier2m-auth-masked.png');

  // --- Privacy off unmasks everything editable ---
  await evaluate(`(() => { __verify.state.settings.privacy.maskSensitiveHeaders=false; __verify.render(); return true; })()`);
  await check('privacy off: header values plaintext, eyes hidden', `[...document.querySelectorAll('#headers-table [data-field="value"]')].every(i=>i.type==='text') && [...document.querySelectorAll('#headers-table [data-act="reveal-row"]')].every(e=>e.hidden)`);
  await check('privacy off: auth value plaintext', `document.querySelector('#req-auth-fields [data-k="value"]').type==='text'`);
  await evaluate(`(() => { __verify.state.settings.privacy.maskSensitiveHeaders=true; __verify.render(); return true; })()`);

  // --- Variables grid keeps its secret checkbox, no reveal eye added ---
  await check('variables grid unchanged (no reveal eye)', `document.querySelectorAll('#vars-table [data-act="reveal-row"]').length===0`);

  // --- The copy path carries real values; the table still shows masks ---
  await evaluate(`__verify.send()`);
  await wait(500);
  await click('#res-tabs .tab[data-tab="res-headers"]');
  const copied = await evaluate(`__verify.responseCopyText()`);
  assert.ok(copied.includes('authorization: synthetic-auth'), `copy must carry the real header, got: ${copied}`);
  assert.ok(copied.includes('x-api-key: synthetic-key'), `copy must carry the real api key, got: ${copied}`);
  assert.ok(!copied.includes('••••'), `copy must not carry masks, got: ${copied}`);
  passed++; console.log('PASS response-header copy yields real values');
  await check('response table still displays masks', `document.querySelector('#res-headers').textContent.includes('••••')`);
  await shot('tier2m-response-masked.png');

  // --- Light theme sanity ---
  await evaluate(`(() => { document.documentElement.dataset.theme='light'; return true; })()`);
  await click('#req-tabs .tab[data-tab="headers"]');
  await shot('tier2m-headers-light.png');
  await evaluate(`(() => { document.documentElement.dataset.theme='dark'; return true; })()`);

  console.log(`\n${passed} checks passed`);
} finally {
  close();
}