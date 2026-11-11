/* P6 smoke: drawer shell + Code move. Gate = both links, one panel, no clipping.
   Run against the isolated debug build (identifier app.postlite.tier1verify20261110). */
import assert from 'node:assert/strict';
import { cmd, evaluate, reload, screenshot, close } from './tier1-cdp.mjs';

const lit = (v) => JSON.stringify(v);
let passed = 0;
const check = async (name, expression) => {
  const value = await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  passed++;
  console.log(`PASS ${name}`);
};
const click = (sel) => evaluate(`document.querySelector(${lit(sel)}).click()`);
const press = async (key, code, vk) => {
  const base = { key, code, ...(vk ? { windowsVirtualKeyCode: vk } : {}) };
  await cmd('Input.dispatchKeyEvent', { type: 'keyDown', ...base });
  await cmd('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
};
const escape = () => press('Escape', 'Escape', 27);
const setUrl = (url) => evaluate(`(() => { const i=document.querySelector('#url'); i.value=${lit(url)}; i.dispatchEvent(new Event('input',{bubbles:true})); return true; })()`);
const selectTarget = (id) => evaluate(`(() => { const s=document.querySelector('#code-target'); s.value=${lit(id)}; s.dispatchEvent(new Event('change',{bubbles:true})); return true; })()`);
const shot = async (name) => { await evaluate(`new Promise(r=>setTimeout(r,340))`); await screenshot(name); };
// The drawer pushes the layout via a 200ms padding transition; let it settle
// before measuring geometry.
const settle = () => evaluate(`new Promise(r=>setTimeout(r,300))`);

try {
  await reload();
  await check('isolated storage', `(await __TAURI__.path.appDataDir()).includes('app.postlite.tier1verify20261110')`);

  // Seed a request with a URL so the Code pane has a real snippet.
  await evaluate(`(() => { const v=__verify; v.addTab(v.newRequest({ method:'POST', url:'http://127.0.0.1:8100/api/echo', headers:[{key:'Content-Type',value:'application/json',enabled:true}], body:{mode:'raw',raw:'{"name":"rina"}',contentType:'application/json'} })); return true; })()`);

  await check('old Code UI is gone', `!document.querySelector('#code-btn') && !document.querySelector('#code-menu') && !document.querySelector('#code-dialog')`);
  await check('two drawer links at right of req-tabs', `(() => { const tabs=document.querySelector('#req-tabs'); const links=[...tabs.querySelectorAll('.drawer-link')]; return links.length===2 && links[0].id==='cookies-link' && links[1].id==='code-link' && tabs.querySelector('.tabs-spacer')!==null; })()`);
  await check('drawer starts closed', `!document.querySelector('#drawer').open && !document.querySelector('.main').classList.contains('drawer-open')`);

  // --- Code link opens the drawer ---
  await click('#code-link');
  await check('Code opens drawer non-modally', `(() => { const d=document.querySelector('#drawer'); return d.open && !document.body.inert && document.querySelector('.main').classList.contains('drawer-open'); })()`);
  await check('Code pane visible, Cookies hidden', `(() => { const c=document.querySelector('[data-drawer-pane="code"]'), k=document.querySelector('[data-drawer-pane="cookies"]'); return !c.hidden && k.hidden; })()`);
  await check('title is Code', `document.querySelector('#drawer-title').textContent==='Code'`);
  await check('Code link active + aria-expanded', `(() => { const l=document.querySelector('#code-link'); return l.classList.contains('is-active') && l.getAttribute('aria-expanded')==='true' && document.querySelector('#cookies-link').getAttribute('aria-expanded')==='false'; })()`);
  await check('drawer is fixed 320px', `Math.round(document.querySelector('#drawer').getBoundingClientRect().width)===320`);
  await check('snippet pane keeps region role + tabindex', `(() => { const o=document.querySelector('#code-output'); return o.getAttribute('role')==='region' && o.getAttribute('tabindex')==='0'; })()`);
  await check('footer has Salin / Impor cURL / Ekspor', `(() => { const f=document.querySelector('[data-drawer-pane="code"] .drawer-foot'); return !!f.querySelector('#code-copy') && !!f.querySelector('#code-import') && !!f.querySelector('#code-export'); })()`);
  await check('snippet renders the request URL', `document.querySelector('#code-output').textContent.includes('http://127.0.0.1:8100/api/echo')`);
  await check('target select populated', `document.querySelector('#code-target').options.length===8`);
  await check('focus lands in the drawer', `document.querySelector('#drawer').contains(document.activeElement)`);

  // --- No clipping: drawer sits right of Send and the URL box ---
  await settle();
  await check('drawer does not overlap Send', `(() => { const d=document.querySelector('#drawer').getBoundingClientRect(), s=document.querySelector('#send').getBoundingClientRect(); return d.left >= s.right - 1; })()`);
  await check('drawer does not clip the URL box', `(() => { const d=document.querySelector('#drawer').getBoundingClientRect(), u=document.querySelector('.urlbox').getBoundingClientRect(); return u.right <= d.left + 1 && u.width > 120; })()`);
  await check('response pane still visible beside drawer', `(() => { const r=document.querySelector('#res-pane').getBoundingClientRect(), d=document.querySelector('#drawer').getBoundingClientRect(); return r.width > 100 && r.right <= d.left + 1; })()`);
  await shot('tier2b-drawer-code.png');

  // --- Switch target regenerates ---
  await selectTarget('py-requests');
  await check('target switch regenerates snippet', `document.querySelector('#code-output').textContent.includes('requests')`);
  await selectTarget('curl');

  // --- Swap to Cookies in place (never stacks) ---
  await click('#cookies-link');
  await check('only one drawer element exists', `document.querySelectorAll('#drawer').length===1`);
  await check('Cookies swaps in place', `(() => { const d=document.querySelector('#drawer'); const c=document.querySelector('[data-drawer-pane="code"]'), k=document.querySelector('[data-drawer-pane="cookies"]'); return d.open && c.hidden && !k.hidden && document.querySelector('#drawer-title').textContent==='Cookies'; })()`);
  await check('Cookies link active, Code inactive', `document.querySelector('#cookies-link').classList.contains('is-active') && !document.querySelector('#code-link').classList.contains('is-active')`);
  await settle();
  await check('still 320px after swap', `Math.round(document.querySelector('#drawer').getBoundingClientRect().width)===320`);
  await shot('tier2b-drawer-cookies.png');

  // --- Swap back to Code ---
  await click('#code-link');
  await check('swap back to Code', `!document.querySelector('[data-drawer-pane="code"]').hidden && document.querySelector('[data-drawer-pane="cookies"]').hidden`);

  // --- Esc closes and returns focus to the opener ---
  await escape();
  await check('Esc closes drawer', `!document.querySelector('#drawer').open && !document.querySelector('.main').classList.contains('drawer-open')`);
  await check('focus returns to opener link', `document.activeElement===document.querySelector('#code-link')`);
  await check('links reset after close', `!document.querySelector('#code-link').classList.contains('is-active') && document.querySelector('#code-link').getAttribute('aria-expanded')==='false'`);

  // --- Clicking the open link toggles it closed ---
  await click('#cookies-link');
  await check('Cookies opens', `document.querySelector('#drawer').open`);
  await click('#cookies-link');
  await check('clicking open link closes it', `!document.querySelector('#drawer').open`);

  // --- Header x closes ---
  await click('#code-link');
  await click('#drawer-close');
  await check('header x closes drawer', `!document.querySelector('#drawer').open`);

  // --- Impor cURL opens the existing modal over the non-modal drawer ---
  await click('#code-link');
  await click('#code-import');
  await check('cURL import modal opens', `document.querySelector('#curl-dialog').open`);
  await check('drawer stays open behind modal', `document.querySelector('#drawer').open`);
  await escape(); // closes the modal (native dialog cancel)
  await check('Esc closed modal, drawer remains', `!document.querySelector('#curl-dialog').open && document.querySelector('#drawer').open`);
  await escape(); // now closes the drawer
  await check('second Esc closes drawer', `!document.querySelector('#drawer').open`);

  // --- Empty request: copy/export disabled, reason shown ---
  await evaluate(`(() => { const i=document.querySelector('#url'); i.value=''; i.dispatchEvent(new Event('input',{bubbles:true})); return true; })()`);
  await click('#code-link');
  await check('no URL disables copy + export', `document.querySelector('#code-copy').disabled && document.querySelector('#code-export').disabled`);
  await check('no URL shows the reason', `document.querySelector('#code-note').textContent.includes('URL')`);
  await check('focus falls back to close when target disabled', `document.activeElement===document.querySelector('#drawer-close')`);
  await shot('tier2b-drawer-empty.png');

  // --- Non-modal: Ctrl+Enter still works while drawer is open ---
  await setUrl('http://127.0.0.1:8100/api/echo');
  await evaluate(`(() => { document.querySelector('#code-link').focus(); return true; })()`);
  await cmd('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, modifiers: 2 });
  await cmd('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, modifiers: 2 });
  await evaluate(`new Promise(r=>setTimeout(r,800))`);
  await check('Ctrl+Enter sends while drawer open', `/200/.test(document.querySelector('#status').textContent)`);
  await check('drawer still open after send', `document.querySelector('#drawer').open`);
  await check('open Code pane refreshes when URL changes', `(() => { const i=document.querySelector('#url'); i.value='http://127.0.0.1:8100/api/changed'; i.dispatchEvent(new Event('input',{bubbles:true})); return document.querySelector('#code-output').textContent.includes('/api/changed') && !document.querySelector('#code-copy').disabled; })()`);

  // --- Light theme sanity ---
  await evaluate(`(() => { document.documentElement.dataset.theme='light'; return true; })()`);
  await settle();
  await shot('tier2b-drawer-light.png');
  await check('light theme drawer still 320px + open', `document.querySelector('#drawer').open && Math.round(document.querySelector('#drawer').getBoundingClientRect().width)===320`);
  await evaluate(`(() => { document.documentElement.dataset.theme='dark'; return true; })()`);

  await escape();
  console.log(`\n${passed} checks passed`);
} finally {
  close();
}