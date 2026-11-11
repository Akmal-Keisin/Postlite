/* Tier 2A: cURL + code generation + full backup, driven through the real app.
   P10 rewrite: the Code menu/dialog moved into the right-side drawer (P6). */
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { cmd, evaluate, reload, screenshot, close } from './tier1-cdp.mjs';

const dir = mkdtempSync(join(tmpdir(), 'postlite-tier2a-files-'));
const winPath = (name) => pathToFileURL(join(dir, name)).pathname.replace(/^\//, '').replaceAll('/', sep);
// Files are handed to the input node directly: clicking the button would open
// the native picker and block the run.
const upload = async (selector, file) => {
  const { root } = await cmd('DOM.getDocument');
  const { nodeId } = await cmd('DOM.querySelector', { nodeId: root.nodeId, selector });
  await cmd('DOM.setFileInputFiles', { nodeId, files: [file] });
};

// lit(value) embeds any string into a page expression without quote accidents.
const lit = (value) => JSON.stringify(value);

let passed = 0;
const check = async (name, expression) => {
  const value = await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  passed++;
  console.log(`PASS ${name}`);
};
const click = (selector) => evaluate(`document.querySelector(${lit(selector)}).click()`);
const wait = async (condition, message = 'condition timeout') => {
  await evaluate(`(async () => { const start = Date.now(); await new Promise((resolve, reject) => { const t = setInterval(() => { if (${condition}) { clearInterval(t); resolve(); } else if (Date.now() - start > 4000) { clearInterval(t); reject(new Error(${lit(message)})); } }, 30); }); })()`);
};
const press = async (key, code, windowsVirtualKeyCode) => {
  const base = { key, code, ...(windowsVirtualKeyCode ? { windowsVirtualKeyCode } : {}) };
  await cmd('Input.dispatchKeyEvent', { type: 'keyDown', ...base });
  await cmd('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
};
const escape = () => press('Escape', 'Escape', 27);
const setUrl = (url) => evaluate(`(() => { const i=document.querySelector('#url'); i.value=${lit(url)}; i.dispatchEvent(new Event('input',{bubbles:true})); return true; })()`);
const selectTarget = (id) => evaluate(`(() => { const s=document.querySelector('#code-target'); s.value=${lit(id)}; s.dispatchEvent(new Event('change',{bubbles:true})); return true; })()`);
// The drawer is non-modal and its links toggle, so re-opening goes through openDrawer
// instead of clicking a link that might already be active.
const openCode = () => evaluate(`(() => { if (!document.querySelector('#drawer').open || document.querySelector('[data-drawer-pane="code"]').hidden) __verify.openDrawer('code'); return true; })()`);
// Dialogs and the drawer fade in, so a capture taken immediately shows a half-transparent pane.
const shot = async (name) => {
  await evaluate(`new Promise((r) => setTimeout(r, 320))`);
  await screenshot(name);
};
const settle = () => evaluate(`new Promise((r) => setTimeout(r, 300))`);

const ECHO = 'http://127.0.0.1:8100/api/echo';
const CURL_FULL = `curl -u 'rina:o"b"x' -X POST ${ECHO} -H 'Content-Type: application/json' --data-raw '{"name":"rina"}'`;
const CURL_PROXY = `curl -x http://proxy:8080 --compressed -b 'sid=7' https://api.example.com/v1/items?limit=2`;

try {
  await reload();
  await check('tier 2a runs against isolated storage', `(await __TAURI__.path.appDataDir()).includes('app.postlite.tier1verify20261110')`);
  await evaluate(`(() => {
    const v = __verify;
    v.state.settings = v.normalizeSettings();
    v.applyAppSettings();
    const a = v.newWorkspace({ name: 'WS A' });
    const b = v.newWorkspace({ name: 'WS B' });
    v.state.workspaces = [a, b];
    v.enterWorkspace(a);
    v.state.collections.push(v.newCollection('Alpha', {
      variables: [{ key: 'token', value: 'secret-123', enabled: true, secret: true }],
      items: [{ kind: 'request', name: 'Greet', method: 'POST', url: 'http://127.0.0.1:8100/api/echo',
        headers: [{ key: 'X-Token', value: '{{token}}', enabled: true }],
        body: { mode: 'raw', raw: '{"name":"rina"}', contentType: 'application/json' } }],
    }));
    v.enterWorkspace(b);
    v.state.collections.push(v.newCollection('Beta'));
    v.enterWorkspace(a);
    v.addTab(structuredClone(v.state.collections[0].items[0]));
    v.persist();
    window.__downloads = [];
    HTMLAnchorElement.prototype.click = function () {
      const name = this.download;
      window.__downloads.push(fetch(this.href).then(r => r.json()).then(data => ({ name, data })));
    };
    window.__clipboard = '';
    navigator.clipboard.writeText = async (text) => { window.__clipboard = text; };
    window.__runtimeErrors = [];
    window.addEventListener('error', (e) => window.__runtimeErrors.push(e.message));
    return true;
  })()`);

  /* ---------------------------------------------------------------- drawer */

  await click('#code-link');
  await check('Code link opens the drawer with the three footer actions',
    `document.querySelector('#drawer').open && document.querySelector('#code-link').getAttribute('aria-expanded')==='true' && ['code-copy','code-import','code-export'].every(id=>!!document.querySelector('#'+id))`);
  await settle();
  await check('drawer sits inside the window at a fixed 320px',
    `(() => { const m=document.querySelector('#drawer').getBoundingClientRect(); return m.left>=0 && m.top>=0 && m.right<=innerWidth && m.bottom<=innerHeight && Math.round(m.width)===320; })()`);
  await shot('tier2a-drawer.png');
  await click('#code-link');
  await check('clicking the open link closes the drawer',
    `!document.querySelector('#drawer').open && document.querySelector('#code-link').getAttribute('aria-expanded')==='false'`);
  await click('#code-link');
  await escape();
  await check('Escape closes the drawer and returns focus to the link',
    `!document.querySelector('#drawer').open && document.activeElement.id==='code-link'`);

  /* ------------------------------------------------------------- copy cURL */

  await openCode();
  await click('#code-copy');
  await check('Copy as cURL resolves variables and secrets into the command',
    `(() => { const c=window.__clipboard; return c.includes('--request POST') && c.includes(${lit(ECHO)}) && c.includes('X-Token: secret-123') && c.includes('Content-Type: application/json') && c.includes(${lit(`--data-raw '{"name":"rina"}'`)}) && !c.includes('{{token}}'); })()`);
  await wait(`document.querySelector('#tool-note').textContent.includes('Snippet copied')`, 'copy never reported');
  await check('the note names the resolved values and the real command length',
    `(() => { const n=document.querySelector('#tool-note').textContent; const m=n.match(/\\((\\d+) characters/); return n.includes('variables resolved') && m && Number(m[1])===window.__clipboard.length; })()`);

  const clipboardBefore = await evaluate('window.__clipboard');
  await setUrl('');
  await click('#code-copy');
  await check('a request without a URL explains itself instead of copying',
    `window.__clipboard===${lit(clipboardBefore)} && document.querySelector('#code-copy').disabled && document.querySelector('#code-note').textContent.includes('URL')`);
  await setUrl(ECHO);

  /* -------------------------------------------------------- generate code */

  await escape();
  await openCode();
  await check('Code drawer opens focused on the target picker',
    `document.querySelector('#drawer').open && document.activeElement.id==='code-target'`);
  await check('the picker offers every verified target',
    `(() => { const ids=[...document.querySelectorAll('#code-target option')].map(o=>o.value); return ids.join(',')===window.postliteText.CODE_TARGETS.map(t=>t.id).join(','); })()`);
  await check('the first snippet is the shell command with resolved values',
    `(() => { const o=document.querySelector('#code-output').textContent; return o.includes('--request POST') && o.includes('X-Token: secret-123') && !o.includes('{{token}}'); })()`);
  await check('the note states no package is needed for a shell command',
    `document.querySelector('#code-note').textContent.startsWith('No third-party package needed.')`);
  await press('Tab', 'Tab', 9);
  await check('Tab from the picker reaches the snippet pane',
    `document.activeElement.id==='code-output'`);
  await selectTarget('js-fetch');
  await check('switching target re-renders the snippet',
    `(() => { const o=document.querySelector('#code-output').textContent; return o.includes('await fetch(') && o.includes(${lit('method: "POST"')}) && o.includes('X-Token'); })()`);
  await selectTarget('js-axios');
  await check('the dependency note comes from the snippet itself',
    `document.querySelector('#code-note').textContent.startsWith('Needs npm install axios.')`);
  await selectTarget('go-http');
  await check('Go snippet carries the body and the resolved header',
    `(() => { const o=document.querySelector('#code-output').textContent; return o.includes('body := bytes.NewBufferString(') && o.includes('Header.Set(') && o.includes('secret-123'); })()`);
  await shot('tier2a-code-dark.png');
  await evaluate(`(() => { const v=__verify; v.state.settings.general.theme='light'; v.applyAppSettings(); return true; })()`);
  await check('the snippet pane uses light tokens in the light theme',
    `document.documentElement.dataset.theme==='light' && getComputedStyle(document.querySelector('#code-output')).backgroundColor==='rgb(255, 255, 255)'`);
  await shot('tier2a-code-light.png');
  await evaluate(`(() => { const v=__verify; v.state.settings.general.theme='dark'; v.applyAppSettings(); return true; })()`);
  await selectTarget('py-requests');
  await evaluate(`window.__clipboard=''; document.querySelector('#code-copy').click()`);
  await wait(`window.__clipboard.length>0`, 'snippet copy never reached the clipboard');
  await check('Copy snippet puts exactly the shown text on the clipboard',
    `window.__clipboard===document.querySelector('#code-output').textContent && document.querySelector('#tool-note').textContent.includes('Snippet copied')`);
  await escape();
  await check('Escape closes the drawer', `!document.querySelector('#drawer').open`);

  await setUrl('');
  await openCode();
  await check('an empty request shows the reason in the drawer and disables copy',
    `(() => { const o=document.querySelector('#code-output').textContent; return o.includes('Enter a URL before exporting') && document.querySelector('#code-copy').disabled && document.querySelector('#code-target').disabled && document.querySelector('#code-note').textContent.length>0; })()`);
  await shot('tier2a-code-empty.png');
  await escape();
  await setUrl(ECHO);

  /* --------------------------------------------------------- import cURL */

  const tabsBefore = await evaluate(`__verify.state.tabs.length`);
  await openCode();
  await click('#code-import');
  await check('Import cURL opens with an empty command box focused',
    `document.querySelector('#curl-dialog').open && document.activeElement.id==='curl-input' && document.querySelector('#curl-input').value==='' && document.querySelector('#curl-status').textContent===''`);
  await press('Tab', 'Tab', 9);
  await check('Tab from the command box reaches a dialog action',
    `['curl-cancel', 'curl-import'].includes(document.activeElement.id)`);
  await evaluate(`document.querySelector('#curl-input').focus()`);
  // keyDown with text exercises the app's keydown handler AND inserts the char;
  // a handler that preventDefaults typing would leave the box empty.
  await cmd('Input.dispatchKeyEvent', { type: 'keyDown', key: 'x', code: 'KeyX', windowsVirtualKeyCode: 88, text: 'x' });
  await cmd('Input.dispatchKeyEvent', { type: 'keyUp', key: 'x', code: 'KeyX', windowsVirtualKeyCode: 88 });
  await check('typing reaches the command box while the modal is open',
    `document.querySelector('#curl-input').value==='x'`);
  await evaluate(`document.querySelector('#curl-input').value=''`);
  await shot('tier2a-curl.png');
  await click('#curl-import');
  await check('an empty command is refused with a hint',
    `document.querySelector('#curl-status').textContent==='Paste a cURL command first.' && __verify.state.tabs.length===${tabsBefore} && document.querySelector('#curl-dialog').open`);
  await evaluate(`document.querySelector('#curl-input').value=${lit(CURL_FULL)}`);
  await click('#curl-import');
  await wait(`__verify.state.tabs.length>${tabsBefore}`, 'import created no tab');
  await check('imported command becomes a new tab with every part mapped',
    `(() => { const req=__verify.state.tabs.at(-1).req; return req.method==='POST' && req.url===${lit(ECHO)} && req.headers.some(h=>h.key==='Content-Type'&&h.value==='application/json') && req.body.mode==='raw' && JSON.parse(req.body.raw).name==='rina' && req.auth.type==='basic' && req.auth.basic.username==='rina' && req.auth.basic.password===${lit('o"b"x')}; })()`);
  await check('the request bar shows the imported method, URL, and body',
    `(() => { const body=document.querySelector('#body'); return document.querySelector('#method').value==='POST' && document.querySelector('#url').value===${lit(ECHO)} && body.value.includes('rina') && [...document.querySelectorAll('#headers-table tbody tr')].length>0; })()`);
  await check('a command with nothing dropped says so plainly',
    `document.querySelector('#tool-note').textContent.startsWith('cURL imported into a new tab.')`);

  const tabsProxy = await evaluate(`__verify.state.tabs.length`);
  await click('#code-import');
  await evaluate(`document.querySelector('#curl-input').value=${lit(CURL_PROXY)}`);
  await click('#curl-import');
  await wait(`__verify.state.tabs.length>${tabsProxy}`, 'proxy command imported no tab');
  await check('cookie becomes a header and the query moves to params',
    `(() => { const req=__verify.state.tabs.at(-1).req; return req.url==='https://api.example.com/v1/items' && req.params.some(p=>p.key==='limit'&&p.value==='2') && req.headers.some(h=>h.key.toLowerCase()==='cookie'&&h.value==='sid=7') && !req.headers.some(h=>h.key.toLowerCase().includes('proxy')); })()`);
  await wait(`document.querySelector('#tool-note').textContent.includes('Not carried over')`, 'dropped flags not reported');
  await check('flags postlite cannot express are named, not guessed',
    `(() => { const n=document.querySelector('#tool-note').textContent; return n.includes('-x') && n.includes('--compressed'); })()`);

  const tabsInvalid = await evaluate(`__verify.state.tabs.length`);
  await click('#code-import');
  await evaluate(`document.querySelector('#curl-input').value='wget https://api.example.com'`);
  await click('#curl-import');
  await check('a command postlite cannot read is refused in place',
    `document.querySelector('#curl-status').textContent.toLowerCase().includes('curl') && document.querySelector('#curl-dialog').open && __verify.state.tabs.length===${tabsInvalid}`);
  await click('#curl-cancel');
  await check('Cancel closes the import dialog', `!document.querySelector('#curl-dialog').open`);

  /* ------------------------------------------------------------ full backup */

  await evaluate(`(() => { const v=__verify; v.state.settings.response.jsonIndent='tab'; v.applyAppSettings(); v.persist(); return true; })()`);
  await click('#open-app-settings');
  await click('#app-settings-tabs [data-tab="pref-transfer"]');
  await check('the Import / Export pane ships a full backup pair',
    `!!document.querySelector('#settings-export-backup') && !!document.querySelector('#backup-file') && document.querySelector('#settings-backup-help').textContent.includes('open tabs') && document.querySelector('#settings-export-warning').textContent.includes('backup')`);
  await shot('tier2a-transfer.png');
  await click('#settings-export-backup');
  await wait(`document.querySelector('#confirm-dialog').open`, 'backup confirm never opened');
  await check('backup export warns about secrets before writing',
    `document.querySelector('#confirm-title').textContent==='Export full backup' && document.querySelector('#confirm-message').textContent.includes('secret variables') && document.querySelector('#confirm-ok').textContent==='Export backup'`);
  await evaluate(`document.querySelector('#confirm-dialog').close('cancel')`);
  await check('cancelling the backup writes no file',
    `(async()=>{await new Promise(r=>setTimeout(r,250)); return (await Promise.all(__downloads)).length===0;})()`);
  await click('#settings-export-backup');
  await wait(`document.querySelector('#confirm-dialog').open`, 'second confirm never opened');
  await click('#confirm-ok');
  await wait(`__downloads.length>0`, 'no backup downloaded');
  await check('backup file holds every workspace, its data, and preferences',
    `(async () => { const file=(await Promise.all(__downloads)).at(-1); return file.name.startsWith('postlite.backup.') && file.name.endsWith('.json') && file.data.kind==='postlite.backup' && file.data.version===1 && file.data.exportedAt && file.data.payload.workspaces.length===2 && file.data.payload.workspaces.map(w=>w.name).join()==='WS A,WS B' && file.data.payload.workspaces[0].collections[0].name==='Alpha' && file.data.payload.workspaces[1].collections[0].name==='Beta' && file.data.payload.workspaces[0].tabs.length>0 && file.data.payload.settings.response.jsonIndent==='tab'; })()`);
  const backup = await evaluate(`(async()=>JSON.stringify((await Promise.all(__downloads)).at(-1).data))()`);
  writeFileSync(join(dir, 'backup.json'), backup);
  const backupIds = JSON.parse(backup).payload.workspaces.map((w) => w.id).join();

  await upload('#backup-file', winPath('backup.json'));
  await wait(`document.querySelector('#restore-dialog').open`, 'restore dialog never opened');
  await check('importing a backup describes both sides and defaults to merge',
    `(() => { const s=document.querySelector('#restore-summary').textContent; return document.querySelector('#restore-dialog').open && s.includes('2 workspace(s)') && s.includes('exported') && document.querySelector('#restore-existing').textContent.includes('Currently open') && document.querySelector('input[name="restore-mode"]:checked').value==='merge'; })()`);
  await shot('tier2a-restore.png');
  await evaluate(`(() => { const v=__verify; v.state.collections.push(v.newCollection('Local only')); v.persist(); return true; })()`);
  const firstId = await evaluate(`__verify.state.workspaces[0].id`);
  await click('#restore-apply');
  await wait(`document.querySelector('#tool-note').textContent.includes('Backup merged')`, 'merge did not report');
  await check('merge adds the file beside current data and keeps local work',
    `(() => { const v=__verify; const names=v.state.workspaces.map(w=>w.name); return v.state.workspaces[0].id===${lit(firstId)} && v.state.collections.some(c=>c.name==='Local only') && names.filter(n=>n==='WS A').length===2 && new Set(v.state.workspaces.map(w=>w.id)).size===4 && !document.querySelector('#restore-dialog').open && document.querySelector('#tool-note').textContent.includes('Backup merged'); })()`);

  await upload('#backup-file', winPath('backup.json'));
  await wait(`document.querySelector('#restore-dialog').open`, 'restore dialog did not reopen');
  await evaluate(`(() => { const c=document.querySelector('input[name="restore-mode"][value="replace"]'); c.checked=true; c.dispatchEvent(new Event('change',{bubbles:true})); return true; })()`);
  await evaluate(`(() => { const v=__verify; v.state.settings.response.jsonIndent='2'; v.applyAppSettings(); return true; })()`);
  await click('#restore-apply');
  await wait(`document.querySelector('#confirm-dialog').open`, 'replace did not ask again');
  await check('replace is confirmed separately',
    `document.querySelector('#confirm-title').textContent==='Replace everything' && document.querySelector('#confirm-message').textContent.includes('cannot undo')`);
  await evaluate(`document.querySelector('#confirm-dialog').close('cancel')`);
  await check('cancelling replace keeps workspaces and preferences',
    `(async()=>{await new Promise(r=>setTimeout(r,250)); return __verify.state.workspaces.length===4 && __verify.state.settings.response.jsonIndent==='2' && document.querySelector('#restore-dialog').open;})()`);
  await click('#restore-apply');
  await wait(`document.querySelector('#confirm-dialog').open`, 'replace did not ask a second time');
  await click('#confirm-ok');
  await wait(`document.querySelector('#tool-note').textContent.includes('Backup restored')`, 'replace did not report');
  await check('replace swaps in the backup and its preferences',
    `(() => { const v=__verify; return v.state.workspaces.map(w=>w.id).join()===${lit(backupIds)} && v.state.collections.length===1 && v.state.collections[0].name==='Alpha' && v.state.settings.response.jsonIndent==='tab' && !document.querySelector('#restore-dialog').open && document.querySelector('#tool-note').textContent.includes('Backup restored'); })()`);
  await check('the restored workspace arrives with its own tab strip',
    `(() => { const v=__verify; return v.state.tabs.length>0 && document.querySelectorAll('#tabbar .tab-item').length===v.state.tabs.length && v.state.activeTabId===v.state.tabs.at(-1).id; })()`);
  await click('#app-settings-close');

  writeFileSync(join(dir, 'broken.json'), '{"kind":"not.a.backup","version":1,"payload":{}}');
  writeFileSync(join(dir, 'unreadable.json'), 'this is not json');
  const guardedIds = await evaluate(`__verify.state.workspaces.map(w=>w.id).join()`);
  await upload('#backup-file', winPath('broken.json'));
  await wait(`document.querySelector('#tool-note').textContent.startsWith('Restore failed')`, 'foreign file not rejected');
  await check('a foreign file is rejected without touching data',
    `!document.querySelector('#restore-dialog').open && __verify.state.workspaces.map(w=>w.id).join()===${lit(guardedIds)}`);
  await upload('#backup-file', winPath('unreadable.json'));
  await wait(`document.querySelector('#tool-note').textContent.includes('not valid JSON')`, 'unreadable file not rejected');
  await check('an unreadable file is rejected the same way',
    `!document.querySelector('#restore-dialog').open && __verify.state.workspaces.map(w=>w.id).join()===${lit(guardedIds)}`);

  /* -------------------------------------------------- shared load migration */

  await check('a legacy single-workspace file loads through the restore path',
    `(() => { const v=__verify; v.applySavedPayload({ collections:[{ name:'Legacy', items:[{ kind:'request', name:'Saved', url:'http://127.0.0.1:8100/saved' }] }], environments:[], tabs:[{ kind:'request', name:'Old', url:'http://127.0.0.1:8100/legacy' }] }, 'load'); v.enterWorkspace(v.state.workspaces[0]); v.render(); return v.state.workspaces.length===1 && v.state.activeWorkspaceId===v.state.workspaces[0].id && v.state.collections[0].name==='Legacy' && v.state.tabs.length===1 && v.state.tabs[0].req.url.endsWith('/legacy'); })()`);
  await check('a payload with no workspace data is refused',
    `(() => { let message=''; try { __verify.applySavedPayload({ version:2, settings:{} }, 'load'); } catch (err) { message=err.message; } return message.includes('no workspace data'); })()`);

  /* ---------------------------------------------------------------- narrow */

  await cmd('Emulation.setDeviceMetricsOverride', { width: 1080, height: 700, deviceScaleFactor: 1, mobile: false });
  await openCode();
  await settle();
  await check('the request bar keeps Send and both drawer links usable at the 1080px minimum',
    `(() => { const d=document.querySelector('#drawer').getBoundingClientRect(); const s=document.querySelector('#send').getBoundingClientRect(); const u=document.querySelector('.urlbox').getBoundingClientRect(); const c=document.querySelector('#code-link').getBoundingClientRect(); const k=document.querySelector('#cookies-link').getBoundingClientRect(); return s.right<=d.left+1 && u.right<=d.left+1 && u.width>120 && c.width>=28 && k.width>=28 && c.right<=document.documentElement.clientWidth && getComputedStyle(document.querySelector('#code-link')).display!=='none'; })()`);
  await shot('tier2a-narrow.png');
  await evaluate(`__verify.closeDrawer()`);
  await cmd('Emulation.clearDeviceMetricsOverride');

  await check('the whole tier 2a pass produced no runtime errors', `window.__runtimeErrors.length===0`);
  await wait(`document.querySelector('#save-state').textContent.startsWith('Saved')`, 'workspace never reported saved');
  const appData = await evaluate(`__TAURI__.path.appDataDir()`);
  await evaluate(`__verify.flushWorkspace().then(()=>true)`);
  const onDisk = JSON.parse(readFileSync(join(appData, 'workspace.json'), 'utf8'));
  assert.deepEqual(onDisk.workspaces.map((w) => w.name), ['My Workspace']);
  assert.equal(onDisk.workspaces[0].collections[0].name, 'Legacy');
  assert.equal(onDisk.workspaces[0].tabs.length, 1);
  assert.equal(onDisk.activeWorkspaceId, onDisk.workspaces[0].id);
  console.log('PASS the last restore and the legacy load are written to workspace.json');
  passed++;

  console.log(`PASS ${passed} tier 2a checks (new baseline after the drawer move)`);
} finally {
  close();
}
