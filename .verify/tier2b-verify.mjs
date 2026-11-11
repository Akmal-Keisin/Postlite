/* Tier 2B verify: HTTP behaviour end-to-end against the isolated debug build
   (identifier app.postlite.tier1verify20261110, devUrl 8100, CDP 9235).
   Three phases, because session-vs-persistent and the no-secret-store path need real app restarts:
     node .verify/tier2b-verify.mjs                  part 1: drawer, masking, cookie suite, persist, proxy
     node .verify/tier2b-verify.mjs --after-restart  part 2: session gone / persistent back, delete on the wire
     node .verify/tier2b-verify.mjs --no-secret      cookie suite with POSTLITE_FORCE_NO_SECRET_STORE=1
   The local forward proxy fixture lives in this process, not in tier1-server.mjs. */
import assert from 'node:assert/strict';
import { createServer, request as httpRequest } from 'node:http';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { cmd, evaluate, reload, screenshot, close } from './tier1-cdp.mjs';

const phase = process.argv.includes('--after-restart') ? 'restart'
  : process.argv.includes('--no-secret') ? 'nosecret' : 'main';

const lit = (v) => JSON.stringify(v);
let passed = 0;
const check = async (name, expression) => {
  const value = await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  passed++;
  console.log(`PASS ${name}`);
};
const click = (sel) => evaluate(`document.querySelector(${lit(sel)}).click()`);
const wait = (ms) => evaluate(`new Promise(r=>setTimeout(r,${ms}))`);
const settle = () => wait(300);
const shot = async (name) => { await wait(340); await screenshot(name); };
const escape = async () => {
  const base = { key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 };
  await cmd('Input.dispatchKeyEvent', { type: 'keyDown', ...base });
  await cmd('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
};
const setReq = (fn) => evaluate(`(() => { const tab=__verify.state.tabs.find(t=>t.id===__verify.state.activeTabId); (${fn.toString()})(tab.req); __verify.render(); return true; })()`);
const sendTo = async (url) => {
  await evaluate(`(() => {
    const u = new URL(${lit(url)});
    const tab = __verify.state.tabs.find(t=>t.id===__verify.state.activeTabId);
    tab.req.url = u.origin + u.pathname;
    tab.req.params = [...u.searchParams.entries()].map(([key,value])=>({key,value,enabled:true}));
    document.querySelector('#url').value = ${lit(url)};
    return true;
  })()`);
  await evaluate(`__verify.send()`);
  await wait(420);
};
const lastBody = async () => {
  const body = await evaluate(`(() => { const t=__verify.state.tabs.find(t=>t.id===__verify.state.activeTabId); return t?.response?.body ?? ''; })()`);
  try { return JSON.parse(body); } catch { return {}; }
};
const wire = async () => (await lastBody()).cookie ?? '';
const rows = () => evaluate(`[...document.querySelectorAll('.cookie-row')].map(r=>({
  domain:r.dataset.domain, path:r.dataset.path, name:r.dataset.name,
  value:r.querySelector('.cookie-value').value, type:r.querySelector('.cookie-value').type,
  session:r.querySelector('.cookie-expiry').classList.contains('is-session'),
  chips:[...r.querySelectorAll('.chip')].map(c=>c.textContent)
}))`);
const rowNamed = async (name, domain) => (await rows()).find(r => r.name === name && (!domain || r.domain === domain));
const openCookies = () => evaluate(`(() => { if (!document.querySelector('#drawer').open || document.querySelector('[data-drawer-pane="cookies"]').hidden) __verify.openDrawer('cookies'); return true; })()`);
const headerRow = (key) => evaluate(`(() => { const tr=[...document.querySelectorAll('#headers-table tbody tr')].find(r=>r.querySelector('[data-field="key"]').value===${lit(key)}); if(!tr) return null; const i=tr.querySelector('[data-field="value"]'); const e=tr.querySelector('[data-act="reveal-row"]'); return { type:i.type, value:i.value, eye:!!e, eyeHidden:e?e.hidden:null }; })()`);

/* The cookie suite, run twice: once with a usable secret store (main) and once with
   POSTLITE_FORCE_NO_SECRET_STORE=1 (nosecret). Everything here is jar behaviour that must
   hold in both; disk expectations are asserted outside, per phase. */
async function cookieCore() {
  await openCookies();
  await check('empty state before any exchange', `!document.querySelector('#cookie-empty').hidden && document.querySelector('#cookie-list').children.length===0`);

  await sendTo('http://127.0.0.1:8100/set?stage=seed');
  await evaluate(`__verify.loadCookies()`);
  let r = await rowNamed('gone', '127.0.0.1');
  assert.ok(r && r.value === 'old' && r.session === false, 'seed must store persistent gone=old');
  passed++; console.log('PASS seed stores a persistent gone=old');

  await sendTo('http://127.0.0.1:8100/set');
  await evaluate(`__verify.loadCookies()`);
  assert.equal(await rowNamed('gone', '127.0.0.1'), undefined, 'expired Set-Cookie must delete gone');
  passed++; console.log('PASS expired Set-Cookie deletes the stored cookie');

  const sid = await rowNamed('sid', '127.0.0.1');
  const theme = await rowNamed('theme', '127.0.0.1');
  const api = await rowNamed('api_scope', '127.0.0.1');
  assert.ok(sid && theme && api, 'basket must store sid, theme, api_scope');
  assert.equal(sid.session, true);
  assert.ok(sid.chips.includes('HttpOnly') && sid.chips.includes('lax'));
  assert.equal(theme.session, false);
  assert.ok(theme.chips.includes('strict'));
  assert.equal(api.path, '/api');
  passed++; console.log('PASS session/persistent split with chips and path');

  await check('masked cookie values keep the real bytes', `[...document.querySelectorAll('.cookie-value')].every(i=>i.type==='password') && ${lit(sid.value)}==='sess-7f3a'`);
  await evaluate(`(() => { const row=[...document.querySelectorAll('.cookie-row')].find(r=>r.dataset.name==='sid'); row.querySelector('.cookie-eye').click(); return true; })()`);
  await check('cookie eye reveals one row only', `(() => { const rs=[...document.querySelectorAll('.cookie-row')]; const a=rs.find(r=>r.dataset.name==='sid'), b=rs.find(r=>r.dataset.name==='theme'); return a.querySelector('.cookie-value').type==='text' && b.querySelector('.cookie-value').type==='password'; })()`);
  await evaluate(`(() => { window.__copied=null; navigator.clipboard.writeText=(t)=>{window.__copied=t;return Promise.resolve();}; return true; })()`);
  await evaluate(`(() => { const row=[...document.querySelectorAll('.cookie-row')].find(r=>r.dataset.name==='theme'); row.querySelector('.cookie-copy').click(); return true; })()`);
  await wait(120);
  await check('cookie copy carries the real value', `window.__copied==='dark'`);

  await sendTo('http://localhost:8100/set');
  await evaluate(`__verify.loadCookies()`);
  await check('two domain groups', `(() => { const d=[...document.querySelectorAll('.cookie-group-head')].map(h=>h.dataset.domain); return d.includes('127.0.0.1') && d.includes('localhost'); })()`);

  await sendTo('http://127.0.0.1:8100/echo');
  let w = await wire();
  assert.ok(w.includes('sid=sess-7f3a') && w.includes('theme=dark') && w.includes('secure_tok=sec-99'), `loopback echo replays the jar, got: ${w}`);
  assert.ok(!w.includes('api_scope'), `path-scoped cookie must not go to /echo, got: ${w}`);
  passed++; console.log('PASS /echo replays sid+theme+secure_tok, withholds api_scope');
  await sendTo('http://127.0.0.1:8100/api/echo');
  w = await wire();
  assert.ok(w.includes('api_scope=1'), `/api/echo must receive api_scope, got: ${w}`);
  passed++; console.log('PASS /api/echo receives the path-scoped cookie');

  await evaluate(`(() => { const h=[...document.querySelectorAll('.cookie-group-head')].find(h=>h.dataset.domain==='localhost'); h.click(); return true; })()`);
  await click('#cookie-delete-domain');
  await wait(150);
  await check('domain delete confirms with the domain name', `document.querySelector('#confirm-dialog').open && /localhost/.test(document.querySelector('#confirm-message').textContent)`);
  await click('#confirm-ok');
  await wait(350);
  await sendTo('http://localhost:8100/echo');
  assert.equal(await wire(), '', 'deleted domain must not replay on the wire');
  passed++; console.log('PASS domain delete is reflected in the next /echo');

  await evaluate(`(() => { const row=[...document.querySelectorAll('.cookie-row')].find(r=>r.dataset.name==='sid'&&r.dataset.domain==='127.0.0.1'); row.querySelector('.cookie-del').click(); return true; })()`);
  await wait(350);
  await sendTo('http://127.0.0.1:8100/echo');
  w = await wire();
  assert.ok(!w.includes('sid=') && w.includes('theme=dark'), `per-row delete must reach the wire, got: ${w}`);
  passed++; console.log('PASS per-row delete is reflected in the next /echo');

  await setReq(req => { req.headers = [{ key: 'Cookie', value: 'manual=1', enabled: true }]; });
  await sendTo('http://127.0.0.1:8100/echo');
  w = await wire();
  assert.ok(w.includes('manual=1') && !w.includes('theme='), `manual Cookie must beat the jar, got: ${w}`);
  passed++; console.log('PASS a manual Cookie header beats the jar');
  await setReq(req => { req.headers = []; });

  await sendTo('https://localhost:8101/set');
  await evaluate(`__verify.loadCookies()`);
  const sec = await rowNamed('secure_tok', 'localhost');
  assert.ok(sec && sec.chips.includes('Secure') && sec.chips.includes('none'), 'https stores Secure+SameSite=none');
  passed++; console.log('PASS https stores secure_tok with Secure and SameSite=none chips');
  await sendTo('https://localhost:8101/echo');
  w = await wire();
  assert.ok(w.includes('secure_tok=sec-99'), `https echo sends the Secure cookie, got: ${w}`);
  passed++; console.log('PASS https /echo sends secure_tok');
}

try {
  await reload();
  await check('isolated storage', `(await __TAURI__.path.appDataDir()).includes('app.postlite.tier1verify20261110')`);
  const appData = await evaluate(`__TAURI__.path.appDataDir()`);
  const wsId = await evaluate(`__verify.state.activeWorkspaceId`);
  const jarFile = join(appData, 'cookies', `${wsId}.bin`);

  if (phase === 'main') {
    await evaluate(`(() => { const v=__verify; v.state.settings=v.normalizeSettings(); v.state.settings.request.sslVerify=false; v.applyAppSettings(); return true; })()`);
    await evaluate(`__TAURI__.core.invoke('cookie_purge',{workspaceId:${lit(wsId)}})`);
    await evaluate(`(() => { __verify.addTab(__verify.newRequest({ method:'GET', url:'http://127.0.0.1:8100/set' })); return true; })()`);

    /* ------------------------------------------- drawer shell and geometry */
    await click('#cookies-link');
    await check('Cookies link opens the drawer', `document.querySelector('#drawer').open && !document.querySelector('[data-drawer-pane="cookies"]').hidden && document.querySelector('[data-drawer-pane="code"]').hidden`);
    await click('#code-link');
    await check('Code swaps in place, only one drawer', `document.querySelectorAll('#drawer').length===1 && !document.querySelector('[data-drawer-pane="code"]').hidden && document.querySelector('[data-drawer-pane="cookies"]').hidden && document.querySelector('#code-link').getAttribute('aria-expanded')==='true' && document.querySelector('#cookies-link').getAttribute('aria-expanded')==='false'`);
    await settle();
    await check('drawer is fixed 320px', `Math.round(document.querySelector('#drawer').getBoundingClientRect().width)===320`);
    await check('drawer does not overlap Send', `(() => { const d=document.querySelector('#drawer').getBoundingClientRect(), s=document.querySelector('#send').getBoundingClientRect(); return d.left>=s.right-1; })()`);
    await check('drawer does not clip the URL box', `(() => { const d=document.querySelector('#drawer').getBoundingClientRect(), u=document.querySelector('.urlbox').getBoundingClientRect(); return u.right<=d.left+1 && u.width>120; })()`);
    await escape();
    await check('Esc closes and returns focus to the link', `!document.querySelector('#drawer').open && document.activeElement.id==='code-link'`);

    /* ------------------------------------- masking on the editable surfaces */
    await setReq(req => {
      req.url = 'http://127.0.0.1:8100/api/echo';
      req.headers = [
        { key: 'Authorization', value: 'Bearer tok-123', enabled: true },
        { key: 'X-Api-Key', value: 'key-1', enabled: true },
        { key: 'X-Plain', value: 'visible', enabled: true },
      ];
    });
    await click('#req-tabs .tab[data-tab="headers"]');
    let h = await headerRow('Authorization');
    assert.ok(h && h.type === 'password' && h.value === 'Bearer tok-123' && h.eye && !h.eyeHidden, 'Authorization masked with eye');
    assert.equal((await headerRow('X-Plain')).type, 'text');
    passed++; console.log('PASS sensitive header masked, plain header visible');
    await evaluate(`(() => { const tr=[...document.querySelectorAll('#headers-table tbody tr')].find(r=>r.querySelector('[data-field="key"]').value==='Authorization'); const i=tr.querySelector('[data-field="value"]'); i.value='Bearer tok-456'; i.dispatchEvent(new Event('input',{bubbles:true})); return true; })()`);
    h = await headerRow('Authorization');
    assert.equal(h.type, 'password', 'editing a masked row must not unmask it');
    assert.equal(h.value, 'Bearer tok-456');
    passed++; console.log('PASS masked header row stays editable in place');
    await evaluate(`(() => { const tr=[...document.querySelectorAll('#headers-table tbody tr')].find(r=>r.querySelector('[data-field="key"]').value==='Authorization'); tr.querySelector('[data-act="reveal-row"]').click(); return true; })()`);
    await check('header eye reveals one row only', `(() => { const rows=[...document.querySelectorAll('#headers-table tbody tr')]; const ra=rows.find(r=>r.querySelector('[data-field="key"]').value==='Authorization'), rk=rows.find(r=>r.querySelector('[data-field="key"]').value==='X-Api-Key'); return ra.querySelector('[data-field="value"]').type==='text' && rk.querySelector('[data-field="value"]').type==='password'; })()`);
    await setReq(req => { req.auth = { type: 'bearer', bearer: { token: 'secret-tok' }, basic: { username: '', password: '' }, apikey: { key: '', value: '', addTo: 'header' } }; });
    await click('#req-tabs .tab[data-tab="auth"]');
    await check('Auth token masked with a per-field eye', `(() => { const i=document.querySelector('#req-auth-fields [data-k="token"]'); const e=document.querySelector('#req-auth-fields [data-act="reveal-auth"]'); return i.type==='password' && i.value==='secret-tok' && e && !e.hidden; })()`);
    await click('#req-auth-fields [data-act="reveal-auth"]');
    await check('Auth eye reveals just that field', `document.querySelector('#req-auth-fields [data-k="token"]').type==='text'`);
    await sendTo('http://127.0.0.1:8100/api/echo');
    await click('#res-tabs .tab[data-tab="res-headers"]');
    const copied = await evaluate(`__verify.responseCopyText()`);
    assert.ok(copied.includes('synthetic-auth') && !copied.includes('•'), `response copy must carry real values, got: ${copied}`);
    passed++; console.log('PASS response-header copy carries real values');
    await check('response table still displays masks', `document.querySelector('#res-headers').textContent.includes('••••')`);
    await setReq(req => { req.headers = []; req.auth = { type: 'none', bearer: { token: '' }, basic: { username: '', password: '' }, apikey: { key: '', value: '', addTo: 'header' } }; });

    /* ------------------------------------------------------- the cookie suite */
    await evaluate(`__TAURI__.core.invoke('cookie_purge',{workspaceId:${lit(wsId)}})`);
    await cookieCore();
    await shot('tier2b-verify-cookies.png');

    /* --------------------------------------- persist off writes nothing */
    await evaluate(`(() => { __verify.state.settings.request.persistCookies=false; __verify.applyAppSettings(); return true; })()`);
    await wait(200);
    await sendTo('http://127.0.0.1:8100/set');
    await evaluate(`__verify.loadCookies()`);
    await check('persist off keeps the session jar', `(await __verify.loadCookies(), document.querySelectorAll('.cookie-row').length>0)`);
    await check('persist off says so in the drawer', `!document.querySelector('#cookie-note').hidden && document.querySelector('#cookie-note').textContent==='Persist cookies is off: these live only until the app closes.'`);
    await evaluate(`__verify.flushWorkspace().then(()=>true)`);
    assert.equal(existsSync(jarFile), false, 'persist off must leave no jar file');
    await sendTo('http://127.0.0.1:8100/set');
    await evaluate(`__verify.flushWorkspace().then(()=>true)`);
    assert.equal(existsSync(jarFile), false, 'further receives must not write while off');
    passed++; console.log('PASS persist off writes nothing, drawer explains');
    await evaluate(`(() => { __verify.state.settings.request.persistCookies=true; __verify.applyAppSettings(); return true; })()`);
    await wait(200);
    await sendTo('http://127.0.0.1:8100/set');
    await evaluate(`__verify.flushWorkspace().then(()=>true)`);
    assert.equal(existsSync(jarFile), true, 'persist on must write the jar again');
    passed++; console.log('PASS persist on writes the jar again');

    /* ------------------------- manual proxy routes, bypass goes direct */
    const proxy = createServer((req, res) => {
      const target = new URL(req.url);
      const chunks = [];
      req.on('data', c => chunks.push(c));
      req.on('end', () => {
        const headers = { ...req.headers, 'x-proxied-by': 'postlite-verify' };
        const fwd = httpRequest({ hostname: target.hostname, port: target.port, path: target.pathname + target.search, method: req.method, headers }, (up) => {
          res.writeHead(up.statusCode, up.headers);
          up.pipe(res);
        });
        fwd.on('error', (e) => { res.writeHead(502); res.end(String(e)); });
        fwd.end(Buffer.concat(chunks));
      });
    });
    await new Promise(r => proxy.listen(8890, '127.0.0.1', r));
    await evaluate(`(() => { __verify.state.settings.request.proxy={ mode:'manual', url:'http://127.0.0.1:8890', bypass:'localhost' }; __verify.applyAppSettings(); return true; })()`);
    await sendTo('http://127.0.0.1:8100/echo');
    assert.equal((await lastBody()).headers?.['x-proxied-by'], 'postlite-verify', 'manual proxy must route the request');
    passed++; console.log('PASS manual proxy routes through the local proxy');
    await sendTo('http://localhost:8100/echo');
    assert.equal((await lastBody()).headers?.['x-proxied-by'], undefined, 'bypass host must go direct');
    passed++; console.log('PASS bypass list goes direct');
    await evaluate(`(() => { __verify.state.settings.request.proxy={ mode:'direct', url:'', bypass:'' }; __verify.applyAppSettings(); return true; })()`);
    proxy.close();

    /* ----------------------------- leave a seeded jar for the restart phase */
    await sendTo('http://127.0.0.1:8100/set');
    await evaluate(`__verify.flushWorkspace().then(()=>true)`);
    assert.equal(existsSync(jarFile), true, 'seed for restart must be on disk');
    await evaluate(`__verify.persist()`);
    console.log(`\n${passed} checks passed (part 1)`);
    console.log('RESTART the isolated app, then: node .verify/tier2b-verify.mjs --after-restart');
  }

  if (phase === 'restart') {
    await openCookies();
    await evaluate(`__verify.loadCookies()`);
    assert.equal(await rowNamed('sid', '127.0.0.1'), undefined, 'session cookie must not survive a restart');
    const theme = await rowNamed('theme', '127.0.0.1');
    assert.ok(theme && theme.value === 'dark', 'persistent cookie must survive a restart');
    passed++; console.log('PASS restart drops the session cookie and restores the persistent one');
    await sendTo('http://127.0.0.1:8100/echo');
    const w = await wire();
    assert.ok(w.includes('theme=dark') && !w.includes('sid='), `restarted jar replays only persistent cookies, got: ${w}`);
    passed++; console.log('PASS restarted jar replays persistent only on the wire');
    await evaluate(`(() => { const row=[...document.querySelectorAll('.cookie-row')].find(r=>r.dataset.name==='theme'&&r.dataset.domain==='127.0.0.1'); row.querySelector('.cookie-del').click(); return true; })()`);
    await wait(350);
    await sendTo('http://127.0.0.1:8100/echo');
    assert.ok(!(await wire()).includes('theme='), 'delete after restart must reach the wire');
    passed++; console.log('PASS delete after restart is reflected in /echo');
    await sendTo('http://127.0.0.1:8100/set');
    await evaluate(`__TAURI__.core.invoke('cookie_purge',{workspaceId:${lit(wsId)}})`);
    await sendTo('http://127.0.0.1:8100/echo');
    assert.equal(await wire(), '', 'clear-all must empty the wire');
    passed++; console.log('PASS clear-all after restart empties the jar');
    await evaluate(`(() => { const v=__verify; v.state.settings=v.normalizeSettings(); v.applyAppSettings(); v.persist(); return true; })()`);
    console.log(`\n${passed} checks passed (part 2, after restart)`);
  }

  if (phase === 'nosecret') {
    await evaluate(`(() => { const v=__verify; v.state.settings=v.normalizeSettings(); v.state.settings.request.sslVerify=false; v.applyAppSettings(); return true; })()`);
    await evaluate(`__TAURI__.core.invoke('cookie_purge',{workspaceId:${lit(wsId)}})`);
    await evaluate(`(() => { __verify.addTab(__verify.newRequest({ method:'GET', url:'http://127.0.0.1:8100/set' })); return true; })()`);
    await cookieCore();
    const snap = await evaluate(`__TAURI__.core.invoke('cookie_list',{workspaceId:${lit(wsId)}})`);
    // `persisting` is the user's switch; the note carries why the jar cannot actually save.
    assert.ok(snap.note && snap.note.includes('no secret store'), `note must explain, got: ${snap.note}`);
    passed++; console.log('PASS no-secret-store snapshot explains why nothing is saved');
    await evaluate(`__verify.loadCookies()`);
    await check('drawer shows the no-secret-store reason', `!document.querySelector('#cookie-note').hidden && document.querySelector('#cookie-note').textContent.includes('no secret store')`);
    await evaluate(`__verify.flushWorkspace().then(()=>true)`);
    assert.equal(existsSync(jarFile), false, 'no secret store must never write a jar file');
    passed++; console.log('PASS nothing written without a secret store');
    await shot('tier2b-verify-no-secret.png');
    console.log(`\n${passed} checks passed (no-secret-store run)`);
  }
} finally {
  close();
}