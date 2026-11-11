/* P7 smoke: Cookie Manager content. Gate = the drawer reflects a real /set + /echo
   exchange against the loopback server (grouping, masking, expiry, chips, filter,
   delete, path/Secure scoping). Run against the isolated debug build
   (identifier app.postlite.tier1verify20261110) with tier1-server.mjs serving 8100/8101. */
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
const shot = async (name) => { await evaluate(`new Promise(r=>setTimeout(r,340))`); await screenshot(name); };
const wait = (ms) => evaluate(`new Promise(r=>setTimeout(r,${ms}))`);

const sendTo = async (url) => {
  // Set the request directly: the #url input handler keeps stale query params when
  // the query is cleared, so driving req.url/req.params avoids that existing quirk.
  await evaluate(`(() => {
    const u = new URL(${lit(url)});
    const tab = __verify.state.tabs.find(t=>t.id===__verify.state.activeTabId);
    tab.req.url = u.origin + u.pathname;
    tab.req.params = [...u.searchParams.entries()].map(([key,value])=>({key,value,enabled:true}));
    document.querySelector('#url').value = ${lit(url)};
    return true;
  })()`);
  await evaluate(`__verify.send()`);
  await wait(420); // let the response land and the open drawer auto-refresh
};
const rows = () => evaluate(`[...document.querySelectorAll('.cookie-row')].map(r=>({
  domain:r.dataset.domain, path:r.dataset.path, name:r.dataset.name,
  value:r.querySelector('.cookie-value').value, type:r.querySelector('.cookie-value').type,
  expiry:r.querySelector('.cookie-expiry').textContent,
  session:r.querySelector('.cookie-expiry').classList.contains('is-session'),
  chips:[...r.querySelectorAll('.chip')].map(c=>c.textContent)
}))`);
const groups = () => evaluate(`[...document.querySelectorAll('.cookie-group-head')].map(h=>({
  domain:h.dataset.domain, count:h.querySelector('.count').textContent, pressed:h.getAttribute('aria-pressed')
}))`);
const rowNamed = async (name, domain) => (await rows()).find(r => r.name === name && (!domain || r.domain === domain));
const lastCookie = async () => {
  const body = await evaluate(`(() => { const t=__verify.state.tabs.find(t=>t.id===__verify.state.activeTabId); return t?.response?.body ?? ''; })()`);
  try { return JSON.parse(body).cookie ?? ''; } catch { return ''; }
};
const setMask = async (on) => {
  await evaluate(`(() => { __verify.state.settings.privacy.maskSensitiveHeaders=${on}; __verify.renderCookies(); return true; })()`);
};

try {
  await reload();
  await check('isolated storage', `(await __TAURI__.path.appDataDir()).includes('app.postlite.tier1verify20261110')`);
  await check('a workspace is active', `!!__verify.state.activeWorkspaceId`);

  // Clean slate: purge this workspace's jar so earlier runs cannot leak rows in.
  await evaluate(`__TAURI__.core.invoke('cookie_purge',{workspaceId:__verify.state.activeWorkspaceId})`);
  await evaluate(`(() => { __verify.addTab(__verify.newRequest({ method:'GET', url:'http://127.0.0.1:8100/set' })); return true; })()`);

  // --- Empty state ---
  await click('#cookies-link');
  await wait(300);
  await check('Cookies drawer opens', `document.querySelector('#drawer').open && !document.querySelector('[data-drawer-pane="cookies"]').hidden`);
  await check('empty state shown when jar is empty', `!document.querySelector('#cookie-empty').hidden && document.querySelector('#cookie-list').children.length===0`);
  await check('footer disabled when nothing to filter', `document.querySelector('#cookie-delete-domain').disabled && document.querySelector('#cookie-show-all').disabled`);
  await shot('tier2b-cookies-empty.png');

  // --- Seed one persistent cookie, then prove the expired delete removes it ---
  await sendTo('http://127.0.0.1:8100/set?stage=seed');
  await check('seed plants gone=old (auto-refresh on send)', `(await __verify.loadCookies(), true)`);
  let r = await rowNamed('gone', '127.0.0.1');
  assert.ok(r, 'gone=old should be stored after seed');
  assert.equal(r.value, 'old');
  assert.equal(r.session, false, 'gone=old has Max-Age, so it is persistent');
  passed++; console.log('PASS seed stores a persistent gone=old');

  await sendTo('http://127.0.0.1:8100/set');
  await evaluate(`__verify.loadCookies()`);
  r = await rowNamed('gone', '127.0.0.1');
  assert.equal(r, undefined, 'gone must be deleted by the expired Set-Cookie');
  passed++; console.log('PASS expired Set-Cookie deletes the stored cookie');

  // --- The basket: session vs persistent, chips, path ---
  const sid = await rowNamed('sid', '127.0.0.1');
  const theme = await rowNamed('theme', '127.0.0.1');
  const api = await rowNamed('api_scope', '127.0.0.1');
  assert.ok(sid && theme && api, 'sid, theme and api_scope must be stored');
  assert.equal(sid.value, 'sess-7f3a');
  assert.equal(sid.session, true, 'sid has no Max-Age/Expires, so it is a session cookie');
  assert.equal(sid.expiry, 'sesi');
  assert.ok(sid.chips.includes('HttpOnly'), 'sid HttpOnly chip');
  assert.ok(sid.chips.includes('lax'), 'sid SameSite=lax chip');
  assert.equal(theme.session, false);
  assert.notEqual(theme.expiry, 'sesi', 'theme shows a real expiry date');
  assert.ok(theme.chips.includes('strict'), 'theme SameSite=strict chip');
  assert.equal(api.path, '/api');
  passed++; console.log('PASS session/persistent split, expiry labels and chips');

  // --- Masking is display-only: value real, type password, per-row eye ---
  await setMask(true);
  await check('masked values use type=password', `(await __verify.loadCookies(), [...document.querySelectorAll('.cookie-value')].every(i=>i.type==='password'))`);
  const sidMasked = await rowNamed('sid', '127.0.0.1');
  assert.equal(sidMasked.value, 'sess-7f3a', 'masking must not change the underlying value');
  assert.equal(sidMasked.type, 'password');
  passed++; console.log('PASS masked input still holds the real value');

  // Reveal one row only.
  await evaluate(`(() => { const row=[...document.querySelectorAll('.cookie-row')].find(r=>r.dataset.name==='sid'); row.querySelector('.cookie-eye').click(); return true; })()`);
  await check('eye reveals only that row', `(() => { const rs=[...document.querySelectorAll('.cookie-row')]; const sid=rs.find(r=>r.dataset.name==='sid'), theme=rs.find(r=>r.dataset.name==='theme'); return sid.querySelector('.cookie-value').type==='text' && sid.querySelector('.cookie-eye').getAttribute('aria-pressed')==='true' && theme.querySelector('.cookie-value').type==='password'; })()`);

  // Copy carries the real value even while masked: spy on writeText.
  await evaluate(`(() => { window.__copied=null; const orig=navigator.clipboard.writeText.bind(navigator.clipboard); navigator.clipboard.writeText=(t)=>{window.__copied=t;return orig(t);}; return true; })()`);
  await evaluate(`(() => { const row=[...document.querySelectorAll('.cookie-row')].find(r=>r.dataset.name==='theme'); row.querySelector('.cookie-copy').click(); return true; })()`);
  await wait(120);
  await check('copy sends the real value, not dots', `window.__copied==='dark'`);

  await setMask(false);
  await check('privacy off shows plaintext values', `[...document.querySelectorAll('.cookie-value')].every(i=>i.type==='text')`);
  await setMask(true);
  await shot('tier2b-cookies-masked.png');

  // --- Second domain: grouping ---
  await sendTo('http://localhost:8100/set');
  await evaluate(`__verify.loadCookies()`);
  const g = await groups();
  await check('two domain groups', `(() => { const d=${lit(g.map(x=>x.domain))}; return d.includes('127.0.0.1') && d.includes('localhost'); })()`);
  await check('group head counts its rows', `(() => { const heads=[...document.querySelectorAll('.cookie-group-head')]; return heads.every(h=>+h.querySelector('.count').textContent === h.closest('.cookie-group').querySelectorAll('.cookie-row').length); })()`);
  await check('sub count names both domains', `/2 domains/.test(document.querySelector('#cookies-sub').textContent)`);
  await shot('tier2b-cookies-groups.png');

  // --- The gate: the jar replays on the wire, scoped by path and Secure ---
  await sendTo('http://127.0.0.1:8100/echo');
  let wire = await lastCookie();
  assert.ok(wire.includes('sid=sess-7f3a'), `echo must replay sid, got: ${wire}`);
  assert.ok(wire.includes('theme=dark'), `echo must replay theme, got: ${wire}`);
  assert.ok(!wire.includes('api_scope'), `api_scope is Path=/api and must not go to /echo, got: ${wire}`);
  // 127.0.0.1 is a trustworthy origin (W3C), so cookie_store allows Secure cookies over http
  // loopback. Secure-withholding over http for a non-loopback host is covered by the Rust unit
  // tests (example.com); it cannot be exercised against this loopback-only harness.
  assert.ok(wire.includes('secure_tok=sec-99'), `loopback is trustworthy, so Secure is sent over http, got: ${wire}`);
  passed++; console.log('PASS /echo replays sid+theme+secure_tok (trustworthy loopback), withholds api_scope by path');

  await sendTo('http://127.0.0.1:8100/api/echo');
  wire = await lastCookie();
  assert.ok(wire.includes('api_scope=1'), `api_scope must go to /api/echo, got: ${wire}`);
  passed++; console.log('PASS /api/echo receives the path-scoped api_scope');

  // --- Filter to one domain, delete it, open all ---
  await evaluate(`(() => { const h=[...document.querySelectorAll('.cookie-group-head')].find(h=>h.dataset.domain==='localhost'); h.click(); return true; })()`);
  await check('group head filters to that domain', `(() => { const bar=document.querySelector('#cookie-filter'); return !bar.hidden && document.querySelector('#cookie-filter-domain').textContent==='localhost' && [...document.querySelectorAll('.cookie-row')].every(r=>r.dataset.domain==='localhost'); })()`);
  await check('filter enables the footer', `!document.querySelector('#cookie-delete-domain').disabled && !document.querySelector('#cookie-show-all').disabled`);
  await check('filtered group head is pressed', `[...document.querySelectorAll('.cookie-group-head')].find(h=>h.dataset.domain==='localhost').getAttribute('aria-pressed')==='true'`);
  await shot('tier2b-cookies-filtered.png');

  const beforeDelete = (await rows()).filter(r => r.domain === 'localhost').length;
  await click('#cookie-delete-domain');
  await wait(150);
  await check('delete domain asks to confirm', `document.querySelector('#confirm-dialog').open && /localhost/.test(document.querySelector('#confirm-message').textContent)`);
  await click('#confirm-ok');
  await wait(350);
  await check('domain delete removes every localhost cookie', `(await __verify.loadCookies(), ![...document.querySelectorAll('.cookie-row')].some(r=>r.dataset.domain==='localhost'))`);
  await check('filter clears once its domain is gone', `document.querySelector('#cookie-filter').hidden && document.querySelector('#cookie-delete-domain').disabled`);
  assert.ok(beforeDelete > 0, 'localhost had cookies to delete');
  passed++; console.log(`PASS Hapus domain confirmed and deleted ${beforeDelete} cookies`);

  // --- Per-row delete (no confirm) ---
  const target = (await rows()).find(r => r.domain === '127.0.0.1');
  await evaluate(`(() => { const row=[...document.querySelectorAll('.cookie-row')].find(r=>r.dataset.name===${lit(target.name)}&&r.dataset.domain==='127.0.0.1'); row.querySelector('.cookie-del').click(); return true; })()`);
  await wait(350);
  await check('per-row delete removes just that cookie', `(await __verify.loadCookies(), ![...document.querySelectorAll('.cookie-row')].some(r=>r.dataset.name===${lit(target.name)}&&r.dataset.domain==='127.0.0.1'))`);

  // --- Buka semua clears an active filter ---
  await evaluate(`(() => { const h=document.querySelector('.cookie-group-head'); h.click(); return true; })()`);
  await check('a filter is active', `!document.querySelector('#cookie-filter').hidden`);
  await click('#cookie-show-all');
  await check('Buka semua clears the filter', `document.querySelector('#cookie-filter').hidden && [...document.querySelectorAll('.cookie-group-head')].every(h=>h.getAttribute('aria-pressed')==='false')`);

  // --- Secure cookie over https, withheld over http ---
  await evaluate(`(() => { __verify.state.settings.request.sslVerify=false; return true; })()`);
  await sendTo('https://localhost:8101/set');
  await evaluate(`__verify.loadCookies()`);
  const sec = await rowNamed('secure_tok', 'localhost');
  assert.ok(sec, 'secure_tok must be stored from the https exchange');
  assert.ok(sec.chips.includes('Secure'), 'secure_tok shows the Secure chip');
  assert.ok(sec.chips.includes('none'), 'secure_tok shows SameSite=none chip');
  passed++; console.log('PASS https stores secure_tok with Secure + SameSite=none chips');
  await shot('tier2b-cookies-secure.png');

  await sendTo('https://localhost:8101/echo');
  wire = await lastCookie();
  assert.ok(wire.includes('secure_tok=sec-99'), `https echo must send the Secure cookie, got: ${wire}`);
  passed++; console.log('PASS https /echo sends secure_tok');

  await sendTo('http://localhost:8100/echo');
  wire = await lastCookie();
  // localhost is likewise a trustworthy origin, so the Secure cookie is allowed over http here too.
  assert.ok(wire.includes('sid=sess-7f3a') && wire.includes('secure_tok=sec-99'), `localhost jar replays on the wire, got: ${wire}`);
  passed++; console.log('PASS http localhost /echo replays its jar (trustworthy origin)');

  // --- Workspace switch resets the filter and reloads that workspace's jar ---
  await evaluate(`(() => { const h=document.querySelector('.cookie-group-head'); if(h) h.click(); return true; })()`);
  const wsId = await evaluate(`(() => { const ws=__verify.newWorkspace({name:'P7 other'}); __verify.state.workspaces.push(ws); __verify.switchWorkspace(ws.id); return ws.id; })()`);
  await wait(350);
  await check('switching workspace clears the filter', `document.querySelector('#cookie-filter').hidden`);
  await check('a fresh workspace jar is empty', `(await __verify.loadCookies(), document.querySelector('#cookie-list').children.length===0 && !document.querySelector('#cookie-empty').hidden)`);
  await evaluate(`(() => { const first=__verify.state.workspaces.find(w=>w.id!==${lit(wsId)}); __verify.switchWorkspace(first.id); return true; })()`);
  await wait(350);
  await check('switching back reloads the original jar', `(await __verify.loadCookies(), document.querySelectorAll('.cookie-row').length>0)`);

  // --- Light theme sanity ---
  await evaluate(`(() => { document.documentElement.dataset.theme='light'; return true; })()`);
  await shot('tier2b-cookies-light.png');
  await check('light theme keeps the drawer open', `document.querySelector('#drawer').open`);
  await evaluate(`(() => { document.documentElement.dataset.theme='dark'; return true; })()`);

  console.log(`\n${passed} checks passed`);
} finally {
  close();
}