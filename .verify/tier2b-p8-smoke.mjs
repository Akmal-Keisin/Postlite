/* P8 smoke: Settings Cookies/Proxy groups, Persist cookies wiring, workspace lifecycle purge,
   restore/cURL flag reporting. Gate = settings round-trip survives restart and the destructive
   confirm names the workspace. Run against the isolated debug build with tier1-server.mjs on 8100. */
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
const wait = (ms) => evaluate(`new Promise(r=>setTimeout(r,${ms}))`);
const shot = async (name) => { await wait(300); await screenshot(name); };
const scrollTo = (sel) => evaluate(`document.querySelector(${lit(sel)}).closest('fieldset').scrollIntoView({ block: 'start' })`);
const setSetting = (path, value) => evaluate(`(() => {
  const c = document.querySelector(${lit(`[data-setting="${path}"]`)});
  if (c.type === 'checkbox') c.checked = ${typeof value === 'boolean' ? value : lit(value)};
  else c.value = ${lit(String(value))};
  c.dispatchEvent(new Event('change', { bubbles: true }));
  return true;
})()`);
const openRequestSettings = async () => {
  await evaluate(`__verify.openAppSettings()`);
  await click('#app-settings-tabs .tab[data-tab="pref-request"]');
};
const sendTo = async (url) => {
  await evaluate(`(() => { if (!__verify.state.tabs.length) __verify.addTab(__verify.newRequest({ url: 'http://127.0.0.1:8100/echo' })); const t = __verify.state.tabs.find(t => t.id === __verify.state.activeTabId); t.req.url = ${lit(url)}; t.req.params = []; __verify.render(); return true; })()`);
  await evaluate(`__verify.send()`);
  await wait(600);
  return evaluate(`__verify.state.tabs.find(t => t.id === __verify.state.activeTabId).response`);
};
const cookieList = (id) => evaluate(`__TAURI__.core.invoke('cookie_list', { workspaceId: ${lit(id)} })`);

try {
  await reload();
  await check('isolated storage', `(await __TAURI__.path.appDataDir()).includes('app.postlite.tier1verify20261110')`);
  await evaluate(`(() => { if (!__verify.state.tabs.length) __verify.addTab(__verify.newRequest({ url: 'http://127.0.0.1:8100/echo' })); return true; })()`);
  await check('shipped default persists cookies, proxy direct', `__verify.DEFAULT_SETTINGS.request.persistCookies === true && __verify.DEFAULT_SETTINGS.request.proxy.mode === 'direct'`);
  // Prior runs persist settings; reset the two groups so this run starts from a known state.
  await evaluate(`(() => { __verify.state.settings.request.persistCookies = true; __verify.state.settings.request.proxy = { mode: 'direct', url: '', bypass: '' }; __verify.applyAppSettings(); return true; })()`);
  const wsId = await evaluate(`__verify.state.activeWorkspaceId`);
  const wsName = await evaluate(`__verify.state.workspaces.find(w => w.id === __verify.state.activeWorkspaceId).name`);

  // --- Cookies + Proxy groups exist with sane defaults ---
  await openRequestSettings();
  await check('Persist cookies defaults on', `document.querySelector('#pref-persistCookies').checked === true`);
  await check('proxy defaults to direct with fields disabled', `(() => {
    const mode = document.querySelector('#pref-proxy-mode');
    const off = ['#pref-proxy-url', '#pref-proxy-user', '#pref-proxy-pass', '#pref-proxy-bypass'].every(s => document.querySelector(s).disabled);
    return mode.value === 'direct' && off && document.querySelector('#pref-proxy-help').textContent.includes('directly');
  })()`);
  await scrollTo('#pref-persistCookies');
  await shot('tier2p8-settings-request.png');

  // --- Mode drives enablement + help ---
  await setSetting('request.proxy.mode', 'manual');
  await check('manual enables url/credentials/bypass', `['#pref-proxy-url', '#pref-proxy-user', '#pref-proxy-pass', '#pref-proxy-bypass'].every(s => !document.querySelector(s).disabled)`);
  await setSetting('request.proxy.mode', 'environment');
  await check('environment disables fields and names the variables', `(() => {
    const off = ['#pref-proxy-url', '#pref-proxy-user', '#pref-proxy-pass', '#pref-proxy-bypass'].every(s => document.querySelector(s).disabled);
    const help = document.querySelector('#pref-proxy-help').textContent;
    return off && help.includes('HTTPS_PROXY') && help.includes('NO_PROXY');
  })()`);
  await setSetting('request.proxy.mode', 'manual');
  await setSetting('request.proxy.url', 'http://127.0.0.1:8899');
  await setSetting('request.proxy.bypass', 'localhost,.internal');
  await check('proxy url/bypass round-trip into settings', `__verify.state.settings.request.proxy.url === 'http://127.0.0.1:8899' && __verify.state.settings.request.proxy.bypass === 'localhost,.internal'`);

  // --- Credentials go to the secret store, never into a save ---
  await evaluate(`(() => { const u = document.querySelector('#pref-proxy-user'); u.value = 'proxyuser'; u.dispatchEvent(new Event('change', { bubbles: true })); const p = document.querySelector('#pref-proxy-pass'); p.value = 'proxypass'; p.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
  await wait(300);
  await check('credentials stored in the OS secret store', `(await __TAURI__.core.invoke('proxy_credentials_get')).username === 'proxyuser'`);
  await check('credentials absent from the workspace payload', `!JSON.stringify(__verify.workspacePayload()).includes('proxypass')`);
  await scrollTo('#pref-proxy-mode');
  await shot('tier2p8-settings-proxy-manual.png');

  // --- Proxy is actually applied per request (validate + routing) ---
  await setSetting('request.proxy.url', '');
  let res = await sendTo('http://127.0.0.1:8100/echo');
  assert.ok(res.error && res.error.includes('proxy url is required'), `empty manual url must error, got ${JSON.stringify(res.error)}`);
  passed++; console.log('PASS manual mode with empty url is a send error');
  await setSetting('request.proxy.url', 'http://127.0.0.1:8899');
  res = await sendTo('http://127.0.0.1:8100/echo');
  assert.ok(res.error, 'a dead manual proxy must fail the send (proves it was used)');
  passed++; console.log('PASS manual proxy is used (dead proxy fails the send)');
  await setSetting('request.proxy.mode', 'direct');
  res = await sendTo('http://127.0.0.1:8100/echo');
  assert.equal(res.status, 200, 'direct must succeed');
  passed++; console.log('PASS direct mode connects without a proxy');

  // --- Clear-all confirm names the workspace, then purges ---
  await sendTo('http://127.0.0.1:8100/set');
  await openRequestSettings();
  await click('#settings-clear-cookies');
  await check('clear-all confirm names the workspace', `document.querySelector('#confirm-dialog').open && document.querySelector('#confirm-message').textContent.includes(${lit(wsName)})`);
  await click('#confirm-dialog button[value="cancel"]');
  await check('cancel keeps the cookies', `(await __TAURI__.core.invoke('cookie_list', { workspaceId: ${lit(wsId)} })).rows.length > 0`);
  await click('#settings-clear-cookies');
  await click('#confirm-ok');
  await wait(300);
  await check('confirm purges the workspace jar', `(await __TAURI__.core.invoke('cookie_list', { workspaceId: ${lit(wsId)} })).rows.length === 0`);

  // --- Persist cookies off: session works, nothing persisted, drawer says so ---
  await setSetting('request.persistCookies', false);
  await wait(200);
  await check('persist off reaches the jar', `(await __TAURI__.core.invoke('cookie_list', { workspaceId: ${lit(wsId)} })).persisting === false`);
  await evaluate(`__verify.openDrawer('cookies')`);
  await wait(300);
  await check('drawer explains persist off', `document.querySelector('#cookie-note').textContent.includes('Persist cookies is off')`);
  await sendTo('http://127.0.0.1:8100/set');
  const offList = await cookieList(wsId);
  assert.equal(offList.persisting, false);
  assert.ok(offList.rows.length > 0, 'the jar still works for the session');
  passed++; console.log('PASS persist off keeps the session jar but does not persist');
  await evaluate(`__verify.closeDrawer()`);

  // --- Settings round-trip survives a restart (reload re-reads workspace.json) ---
  await setSetting('request.proxy.mode', 'manual');
  await evaluate(`__verify.flushWorkspace()`);
  await reload();
  await check('settings round-trip survives restart', `__verify.state.settings.request.persistCookies === false && __verify.state.settings.request.proxy.mode === 'manual' && __verify.state.settings.request.proxy.url === 'http://127.0.0.1:8899'`);
  await check('persist switch re-applied at boot', `(await __TAURI__.core.invoke('cookie_list', { workspaceId: __verify.state.activeWorkspaceId })).persisting === false`);
  // The round-trip restored the dead manual proxy; go direct again so later sends can reach the server.
  await evaluate(`(() => { __verify.state.settings.request.proxy.mode = 'direct'; __verify.applyAppSettings(); return true; })()`);

  // --- Deleting a workspace purges its jar ---
  const ws2 = await evaluate(`(() => { const ws = __verify.newWorkspace(); __verify.state.workspaces.push(ws); __verify.switchWorkspace(ws.id); return ws.id; })()`);
  await wait(200);
  await sendTo('http://127.0.0.1:8100/set');
  assert.ok((await cookieList(ws2)).rows.length > 0, 'second workspace seeded');
  // Fire-and-forget: deleteWorkspace awaits the confirm dialog, so the click must not be gated on it.
  await evaluate(`(() => { void __verify.deleteWorkspace(${lit(ws2)}); return true; })()`);
  await click('#confirm-ok');
  await wait(300);
  await check('workspace delete purges its jar', `(await __TAURI__.core.invoke('cookie_list', { workspaceId: ${lit(ws2)} })).rows.length === 0`);

  // --- cURL flags reported, never applied ---
  await check('-x/-c reported and -b string becomes a manual Cookie header', `(() => {
    const p = postliteText.parseCurl('curl -x http://p:1 -c jar.txt -b "sid=1" http://example.com/');
    return p.notes.includes('proxy flags are not applied') && p.notes.includes('cookie-jar flags are not applied')
      && p.headers.some(h => h.key === 'Cookie' && h.value === 'sid=1');
  })()`);
  await check('-b <file> reported, not imported', `(() => {
    const p = postliteText.parseCurl('curl -b cookies.txt http://example.com/');
    return p.notes.includes('cookie files are not imported') && !p.headers.some(h => h.key === 'Cookie');
  })()`);
  const modeBefore = await evaluate(`__verify.state.settings.request.proxy.mode`);
  await evaluate(`(() => { __verify.openCurlDialog(); document.querySelector('#curl-input').value = 'curl -x http://p:1 -b "sid=1" http://127.0.0.1:8100/echo'; return true; })()`);
  await evaluate(`__verify.importCurlCommand()`);
  await wait(200);
  await check('cURL import does not mutate the global proxy', `__verify.state.settings.request.proxy.mode === ${lit(modeBefore)}`);

  // --- Light theme sanity for the new groups ---
  await evaluate(`(() => { document.documentElement.dataset.theme = 'light'; return true; })()`);
  await openRequestSettings();
  await scrollTo('#pref-persistCookies');
  await shot('tier2p8-settings-light.png');
  await evaluate(`(() => { document.documentElement.dataset.theme = 'dark'; return true; })()`);

  console.log(`\n${passed} checks passed`);
} finally {
  close();
}