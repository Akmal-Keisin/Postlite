import assert from 'node:assert/strict';
import { evaluate, close, screenshot, reload } from './tier1-cdp.mjs';

const results = [];
const check = async (name, expression) => {
  const value = await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  results.push(name);
  console.log(`PASS ${name}`);
};
const click = selector => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
const settle = () => evaluate('__verify.flushWorkspace().then(()=>true)');
// Seed one collection with three requests against the echo fixture.
const seed = () => evaluate(`(() => {
  const v = __verify;
  v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
  v.state.settings = v.normalizeSettings(); v.applyAppSettings();
  const r = (name, url, scripts) => v.newRequest({ name, url, method: 'GET', scripts: scripts ?? { pre: '', test: '' } });
  const c = v.newCollection('Runner suite', { items: [
    r('First', 'http://127.0.0.1:8100/api/echo?n=1'),
    r('Second', 'http://127.0.0.1:8100/api/echo?n=2'),
    r('Third', 'http://127.0.0.1:8100/api/echo?n=3'),
  ]});
  v.state.collections.push(c);
  v.addTab(c.items[0]);
  v.persist(); v.render();
  return true;
})()`);
const waitRunDone = () => evaluate(`new Promise(resolve => {
  const start = Date.now();
  const timer = setInterval(() => {
    if (!__verify.runnerState.active || Date.now() - start > 15000) { clearInterval(timer); resolve(true); }
  }, 30);
})`);

try {
  await reload();
  await evaluate(`(() => {
    window.__runtimeErrors = []; window.addEventListener('error', e => __runtimeErrors.push(e.message));
  })()`);
  await seed();

  await check('executeRequest is exposed', `typeof __verify.executeRequest === 'function'`);
  await check('executeRequest returns response and results for a live request',
    `(async () => { const { response, results } = await __verify.executeRequest(__verify.state.collections[0].items[0]); return response.status === 200 && Array.isArray(results) && !response.error; })()`);
  await check('executeRequest has no tab, button or pane side effects',
    `(async () => {
      const v = __verify, tab = v.state.tabs[0], button = document.querySelector('#send');
      const before = JSON.stringify({ resp: tab.response, active: v.state.activeTabId, disabled: button.disabled, text: button.textContent });
      await v.executeRequest(v.state.collections[0].items[1]);
      const after = JSON.stringify({ resp: tab.response, active: v.state.activeTabId, disabled: button.disabled, text: button.textContent });
      return before === after;
    })()`);
  await check('a failing pre-request script still sends and records the error',
    `(async () => {
      const v = __verify;
      const req = v.newRequest({ name: 'Bad pre', url: 'http://127.0.0.1:8100/api/echo', scripts: { pre: 'throw new Error("boom")', test: 'pm.test("ok", () => pm.expect(pm.response.code).to.equal(200));' } });
      const { response, results } = await v.executeRequest(req);
      return response.status === 200 && results.some(r => !r.passed && /boom/.test(r.message)) && results.some(r => r.passed && r.name === 'ok');
    })()`);
  await check('empty URL returns an error result without throwing',
    `(async () => { const { response, results } = await __verify.executeRequest(__verify.newRequest({ name: 'x', url: '  ' })); return response.error === 'URL is required' && results.length === 0; })()`);
  await check('manual Send still renders the response and restores the button',
    `(async () => {
      const v = __verify, tab = v.state.tabs[0];
      document.querySelector('#url').value = 'http://127.0.0.1:8100/api/echo?manual=1';
      document.querySelector('#url').dispatchEvent(new Event('input', { bubbles: true }));
      document.querySelector('#send').click();
      await new Promise(r => { const t = setInterval(() => { if (tab.response && !document.querySelector('#send').disabled) { clearInterval(t); r(); } }, 30); setTimeout(() => { clearInterval(t); r(); }, 8000); });
      const b = document.querySelector('#send');
      return tab.response?.status === 200 && tab.response.body.includes('manual=1') && !b.disabled && b.textContent === 'Send' && Array.isArray(tab.response.results);
    })()`);
  await check('manual Send with empty URL still notifies and focuses #url',
    `(() => {
      const v = __verify, tab = v.state.tabs[0];
      document.querySelector('#url').value = '';
      document.querySelector('#url').dispatchEvent(new Event('input', { bubbles: true }));
      const before = tab.response;
      document.querySelector('#send').click();
      return tab.response === before && document.activeElement === document.querySelector('#url');
    })()`);

  await check('right-click on a folder opens a folder menu with Run collection',
    `(() => {
      const v = __verify;
      const folder = v.newFolder({ name: 'Sub', items: [v.state.collections[0].items[2]] });
      v.state.collections[0].items.splice(2, 1, folder);
      v.render();
      const el = document.querySelector('.node[data-kind="folder"]');
      el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 120, clientY: 160 }));
      const menu = document.querySelector('#folder-menu');
      return !menu.hidden && !!menu.querySelector('[data-menu="run"]');
    })()`);
  await check('Escape closes the folder menu',
    `(() => {
      const menu = document.querySelector('#folder-menu');
      menu.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      return menu.hidden;
    })()`);
  await check('Shift+F10 on a folder opens the folder menu',
    `(() => {
      const el = document.querySelector('.node[data-kind="folder"]');
      el.focus();
      el.dispatchEvent(new KeyboardEvent('keydown', { key: 'F10', shiftKey: true, bubbles: true }));
      const open = !document.querySelector('#folder-menu').hidden;
      document.querySelector('#folder-menu').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      return open;
    })()`);
  await check('collection context menu offers Run collection',
    `(() => {
      const el = document.querySelector('.node[data-kind="collection"]');
      el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 120, clientY: 120 }));
      const menu = document.querySelector('#coll-menu');
      return !menu.hidden && !!menu.querySelector('[data-menu="run"]');
    })()`);
  await check('Run collection is disabled while a run is active',
    `(() => {
      __verify.runnerState.active = true;
      const el = document.querySelector('.node[data-kind="collection"]');
      el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 120, clientY: 120 }));
      const disabled = document.querySelector('#coll-menu [data-menu="run"]').disabled;
      __verify.runnerState.active = false;
      document.querySelector('#coll-menu').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      return disabled;
    })()`);

  // Task 3 — selection dialog. Give the Sub folder a second request so the
  // indeterminate state is reachable, then open the dialog on the collection.
  await evaluate(`(() => {
    const v = __verify;
    const sub = v.state.collections[0].items[2];
    sub.items.push(v.newRequest({ name: 'Fourth', url: 'http://127.0.0.1:8100/api/echo?n=4' }));
    v.persist(); v.render();
    v.openRunnerDialog(v.state.collections[0].id);
    return true;
  })()`);
  await check('runner dialog opens modal with the node name in the title',
    `document.querySelector('#runner-dialog').open && document.querySelector('#runner-title').textContent.includes('Runner suite')`);
  await check('all requests checked by default, counter shows m of m',
    `(() => {
      const boxes = [...document.querySelectorAll('#runner-tree input[data-run-id]')];
      return boxes.length === 4 && boxes.every(b => b.checked) && document.querySelector('#runner-count').textContent === '4 of 4 selected';
    })()`);
  await check('selection tree is in tree order including subfolders',
    `(() => {
      const v = __verify, ids = [...document.querySelectorAll('#runner-tree input[data-run-id]')].map(b => b.dataset.runId);
      const c = v.state.collections[0];
      return ids[0] === c.items[0].id && ids[1] === c.items[1].id && ids[2] === c.items[2].items[0].id && ids[3] === c.items[2].items[1].id;
    })()`);
  await check('folder checkbox toggles all descendants and shows indeterminate',
    `(() => {
      const v = __verify, folder = v.state.collections[0].items[2];
      const q = sel => document.querySelector(sel);
      const fid = '#runner-tree input[data-run-folder="' + folder.id + '"]';
      const cid = '#runner-tree input[data-run-id="' + folder.items[0].id + '"]';
      q(fid).click();
      const unchecked = !q(cid).checked && !q('#runner-tree input[data-run-id="' + folder.items[1].id + '"]').checked && q('#runner-count').textContent === '2 of 4 selected';
      q(fid).click();
      const rechecked = q(cid).checked && q('#runner-count').textContent === '4 of 4 selected';
      q(cid).click();
      const mixed = q(fid).indeterminate && q('#runner-count').textContent === '3 of 4 selected';
      q(cid).click();
      return unchecked && rechecked && mixed && q('#runner-count').textContent === '4 of 4 selected';
    })()`);
  await check('Run is disabled at zero selected',
    `(() => {
      const boxes = () => [...document.querySelectorAll('#runner-tree input[data-run-id]')];
      for (let i = 0; i < 4; i++) boxes().find(b => b.checked)?.click();
      const disabled = document.querySelector('#runner-run').disabled && document.querySelector('#runner-count').textContent === '0 of 4 selected';
      for (let i = 0; i < 4; i++) boxes().find(b => !b.checked)?.click();
      return disabled && !document.querySelector('#runner-run').disabled;
    })()`);
  await check('Cancel closes the dialog without running',
    `(() => {
      document.querySelector('#runner-cancel').click();
      return !document.querySelector('#runner-dialog').open && !__verify.runnerState.active;
    })()`);

  // Task 4 — run loop. Helper: seed a fresh collection from request specs,
  // optionally bind an environment, open the dialog.
  const runSpecs = (specs, { bindEnv = false } = {}) => evaluate(`(() => {
    const v = __verify;
    v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
    const specs = ${JSON.stringify(specs)};
    const items = specs.map(s => v.newRequest({ name: s.name, url: s.url, method: s.method ?? 'GET',
      headers: (s.headers ?? []).map(h => ({ key: h.key, value: h.value, enabled: true })),
      scripts: { pre: s.pre ?? '', test: s.test ?? '' } }));
    const c = v.newCollection('Run target', { items });
    if (${bindEnv ? 'true' : 'false'}) {
      const env = v.newEnvironment({ name: 'Run env' });
      v.state.environments.push(env);
      c.environmentId = env.id;
    }
    v.state.collections.push(c);
    v.addTab(items[0]);
    v.persist(); v.render();
    v.openRunnerDialog(c.id);
    return true;
  })()`);
  const startAndWait = async () => { await click('#runner-run'); await waitRunDone(); };
  const E = 'http://127.0.0.1:8100/api/echo';

  await runSpecs([
    { name: 'Login', url: 'http://127.0.0.1:8100/api/token', method: 'POST', test: 'pm.environment.set("token", pm.response.json().token);' },
    { name: 'Authorized', url: E, headers: [{ key: 'Authorization', value: 'Bearer {{token}}' }] },
    { name: 'After', url: E + '?after=1' },
  ], { bindEnv: true });
  await startAndWait();
  await check('environment chaining: login token reaches the next request and persists',
    `(() => {
      const s = __verify.runnerState;
      const echo = JSON.parse(s.entries[1].response.body);
      const env = __verify.state.environments[0];
      return s.entries.every(e => e.status === 'passed') && echo.headers.authorization === 'Bearer runner-token-123' && env.variables.some(r => r.key === 'token' && r.value === 'runner-token-123');
    })()`);
  await check('run result equals manual send result for the same request',
    `(async () => {
      const v = __verify, s = v.runnerState;
      const manual = await v.executeRequest(s.entries[2].req);
      const ran = s.entries[2];
      return manual.response.status === ran.response.status && manual.response.body === ran.response.body && JSON.stringify(manual.results.map(r => [r.name, r.passed])) === JSON.stringify(ran.results.map(r => [r.name, r.passed]));
    })()`);
  await check('requests run sequentially in tree order',
    `(() => {
      const s = __verify.runnerState;
      const names = s.entries.map(e => e.req.name);
      const u1 = JSON.parse(s.entries[1].response.body).url;
      const u2 = JSON.parse(s.entries[2].response.body).url;
      return names.join(',') === 'Login,Authorized,After' && u1 === '/api/echo' && u2 === '/api/echo?after=1';
    })()`);
  await check('no tab, active-tab or #send mutation during a run',
    `(() => {
      const v = __verify, b = document.querySelector('#send');
      return v.state.tabs.length === 1 && v.state.tabs[0].response == null && !b.disabled && b.textContent === 'Send';
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  await runSpecs([
    { name: 'Pass', url: E },
    { name: 'Fail test', url: E, test: 'pm.test("must be 201", () => pm.expect(pm.response.code).to.equal(201));' },
    { name: 'Never', url: E },
  ]);
  await startAndWait();
  await check('a failed pm.test stops the run and marks the rest not run',
    `(() => {
      const s = __verify.runnerState;
      return s.entries[0].status === 'passed' && s.entries[1].status === 'failed' && s.entries[2].status === 'skipped' && s.endReason === 'failed' && !s.active && document.querySelector('#runner-dialog').open;
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  await runSpecs([
    { name: 'Pass', url: E },
    { name: 'Transport', url: 'http://127.0.0.1:9/unreachable' },
    { name: 'Never', url: E },
  ]);
  await startAndWait();
  await check('a transport error stops the run',
    `(() => {
      const s = __verify.runnerState;
      return s.entries[1].status === 'failed' && !!s.entries[1].response.error && s.entries[2].status === 'skipped' && s.endReason === 'failed';
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  await runSpecs([
    { name: 'Server error', url: 'http://127.0.0.1:8100/api/server-error' },
    { name: 'Never', url: E },
  ]);
  await startAndWait();
  await check('an HTTP 500 without tests stops the run and marks the row failed',
    `(() => {
      const s = __verify.runnerState;
      return s.entries[0].status === 'failed' && s.entries[0].response.status === 500 && s.entries[0].results.length === 0 && s.entries[1].status === 'skipped' && s.endReason === 'failed';
    })()`);
  await check('HTTP 500 rows use failure styling for the mark and status metadata',
    `(() => {
      const row = document.querySelector('#runner-results .runner-row');
      return row.querySelector('.runner-mark').classList.contains('fail')
        && row.querySelector('.runner-meta').classList.contains('fail')
        && row.querySelector('.runner-meta').textContent.includes('500');
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  await runSpecs([
    { name: 'Negative', url: 'http://127.0.0.1:8100/api/not-found', test: 'pm.test("is 404", () => pm.expect(pm.response.code).to.equal(404));' },
    { name: 'Continues', url: E },
  ]);
  await startAndWait();
  await check('a passing 404 negative test does not stop the run',
    `(() => {
      const s = __verify.runnerState;
      return s.entries[0].status === 'passed' && s.entries[0].response.status === 404 && s.entries[1].status === 'passed' && s.endReason === 'done';
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  await runSpecs([
    { name: 'Empty', url: '   ' },
    { name: 'Never', url: E },
  ]);
  await startAndWait();
  await check('a checked request with empty URL fails with "URL is required" and stops',
    `(() => {
      const s = __verify.runnerState;
      return s.entries[0].status === 'failed' && s.entries[0].response.error === 'URL is required' && s.entries[1].status === 'skipped';
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  await runSpecs([
    { name: 'Bad pre', url: E, pre: 'throw new Error("pre boom")' },
    { name: 'Never', url: E },
  ]);
  await startAndWait();
  await check('pre-script failure completes the pipeline, then stops the run',
    `(() => {
      const s = __verify.runnerState;
      return s.entries[0].response.status === 200 && s.entries[0].results.some(r => !r.passed && /pre boom/.test(r.message)) && s.entries[0].status === 'failed' && s.entries[1].status === 'skipped';
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  await runSpecs([
    { name: 'Slow', url: 'http://127.0.0.1:8100/api/slow' },
    { name: 'Never', url: E },
  ]);
  await click('#runner-run');
  await evaluate(`new Promise(r => setTimeout(r, 60))`);
  await click('#runner-stop');
  await waitRunDone();
  await check('Stop finishes the in-flight request, records it, marks the rest not run',
    `(() => {
      const s = __verify.runnerState;
      return s.entries[0].status === 'passed' && s.entries[0].response.body === 'slow' && s.entries[1].status === 'skipped' && s.endReason === 'stopped' && document.querySelector('#runner-dialog').open;
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  await runSpecs([
    { name: 'Slow', url: 'http://127.0.0.1:8100/api/slow' },
    { name: 'Never', url: E },
  ]);
  await click('#runner-run');
  await evaluate(`new Promise(r => setTimeout(r, 60))`);
  await evaluate(`document.querySelector('#runner-dialog').dispatchEvent(new Event('cancel', { cancelable: true }))`);
  await waitRunDone();
  await check('Escape during a run acts as Stop and keeps the dialog open',
    `(() => {
      const s = __verify.runnerState;
      return document.querySelector('#runner-dialog').open && s.endReason === 'stopped' && s.entries[1].status === 'skipped';
    })()`);
  await check('after the run only Close is offered',
    `!document.querySelector('#runner-close').hidden && document.querySelector('#runner-stop').hidden && document.querySelector('#runner-run').hidden && document.querySelector('#runner-cancel').hidden`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  // Task 5 — result row expansion and final state.
  await runSpecs([
    { name: 'Pass', url: E, test: 'pm.test("has body", () => pm.expect(pm.response.text()).to.include("method"));' },
    { name: 'Fail test', url: E, test: 'pm.test("must be 201", () => pm.expect(pm.response.code).to.equal(201));' },
    { name: 'Never', url: E },
  ]);
  await startAndWait();
  await check('failed row shows status, elapsed and tests passed/total',
    `(() => {
      const row = document.querySelectorAll('#runner-results .runner-row')[1];
      const meta = row.querySelector('.runner-meta').textContent;
      return /200/.test(meta) && /ms/.test(meta) && /0\\/1 tests/.test(meta);
    })()`);
  await evaluate(`document.querySelectorAll('#runner-results .runner-row-head')[1].click()`);
  await check('expanding a row shows test names, failure message and body preview',
    `(() => {
      const row = document.querySelectorAll('#runner-results .runner-row')[1];
      const detail = row.querySelector('.runner-detail');
      return !detail.hidden && row.querySelector('.runner-row-head').getAttribute('aria-expanded') === 'true'
        && detail.textContent.includes('must be 201') && detail.querySelector('.runner-body-preview').textContent.includes('"method"');
    })()`);
  await evaluate(`document.querySelectorAll('#runner-results .runner-row-head')[1].click()`);
  await check('clicking again collapses the row',
    `document.querySelectorAll('#runner-results .runner-row')[1].querySelector('.runner-detail').hidden`);
  await check('not-run rows are not expandable',
    `(() => {
      const row = document.querySelectorAll('#runner-results .runner-row')[2];
      return row.querySelector('.runner-meta').textContent === 'Not run' && !row.querySelector('.runner-row-head').hasAttribute('data-expandable');
    })()`);
  await check('final header shows the failed end state',
    `document.querySelector('#runner-count .runner-final.failed')?.textContent === 'Failed'`);
  await check('body preview is plain text, not injected HTML',
    `(() => {
      const row = document.querySelectorAll('#runner-results .runner-row')[0];
      row.querySelector('.runner-row-head').click();
      const pre = row.querySelector('.runner-body-preview');
      return pre.childElementCount === 0 && pre.textContent.length > 0;
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);
  await check('closing after a run resets to selection phase for the next open',
    `__verify.runnerState.phase === 'select' && !document.querySelector('#runner-dialog').open`);

  // pm.collectionVariables — set/get/has/unset from scripts, persisted to the
  // collection's own Variables table and never to the bound environment.
  const VAR_SPECS = [
    { name: 'Setter', url: E, headers: [{ key: 'X-Cv', value: '{{cv_token}}' }],
      pre: 'pm.collectionVariables.set("cv_token", "abc"); pm.test("get", () => pm.expect(pm.collectionVariables.get("cv_token")).to.equal("abc"));',
      test: 'pm.collectionVariables.set("cv_test", "t"); pm.test("has", () => pm.expect(pm.collectionVariables.has("cv_token")).to.equal(true));' },
    { name: 'Reader', url: E, headers: [{ key: 'X-Cv', value: '{{cv_token}}' }] },
    { name: 'Unsetter', url: E,
      pre: 'pm.collectionVariables.unset("cv_token");',
      test: 'pm.test("gone", () => pm.expect(pm.collectionVariables.has("cv_token")).to.equal(false));' },
  ];
  const seedVars = (specs) => evaluate(`(() => {
    const v = __verify;
    v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
    const env = v.newEnvironment({ name: 'Vars env' });
    v.state.environments.push(env);
    const items = ${JSON.stringify(specs)}.map(s => v.newRequest({ name: s.name, url: s.url, method: 'GET',
      headers: (s.headers ?? []).map(h => ({ key: h.key, value: h.value, enabled: true })),
      scripts: { pre: s.pre ?? '', test: s.test ?? '' } }));
    const c = v.newCollection('Vars suite', { items, variables: [] });
    c.environmentId = env.id;
    v.state.collections.push(c);
    v.addTab(items[0]);
    v.persist(); v.render();
    return true;
  })()`);
  await seedVars(VAR_SPECS);
  await check('pm.collectionVariables set/get/has work in a pre-script and the same request sees the value',
    `(async () => {
      const { response, results } = await __verify.executeRequest(__verify.state.collections[0].items[0]);
      return results.every(r => r.passed) && results.some(r => r.name === 'get') && JSON.parse(response.body).headers['x-cv'] === 'abc';
    })()`);
  await check('test-script set writes to the collection Variables table, not the bound environment',
    `(() => {
      const c = __verify.state.collections[0], env = __verify.state.environments[0];
      return c.variables.some(r => r.key === 'cv_token' && r.value === 'abc' && r.enabled !== false)
        && c.variables.some(r => r.key === 'cv_test' && r.value === 't')
        && !env.variables.some(r => r.key.startsWith('cv_'));
    })()`);
  await check('the next request reads the collection variable through {{ }} interpolation',
    `(async () => {
      const { response } = await __verify.executeRequest(__verify.state.collections[0].items[1]);
      return JSON.parse(response.body).headers['x-cv'] === 'abc';
    })()`);
  await check('unset removes the collection variable and has() returns false',
    `(async () => {
      const v = __verify, c = v.state.collections[0];
      const { results } = await v.executeRequest(c.items[2]);
      return results.every(r => r.passed) && !c.variables.some(r => r.key === 'cv_token') && c.variables.some(r => r.key === 'cv_test');
    })()`);
  await seedVars(VAR_SPECS.slice(0, 2));
  await evaluate(`__verify.openRunnerDialog(__verify.state.collections[0].id)`);
  await startAndWait();
  await check('runner passes a collection variable from one request to the next',
    `(() => {
      const s = __verify.runnerState;
      return s.entries.every(e => e.status === 'passed') && JSON.parse(s.entries[1].response.body).headers['x-cv'] === 'abc';
    })()`);
  await evaluate(`document.querySelector('#runner-close').click()`);

  // Task 6 — screenshots (local artifacts, gitignored) and runtime-error trap.
  await runSpecs([
    { name: 'Login', url: 'http://127.0.0.1:8100/api/token', method: 'POST', test: 'pm.environment.set("token", pm.response.json().token);' },
    { name: 'Authorized', url: E, headers: [{ key: 'Authorization', value: 'Bearer {{token}}' }] },
  ], { bindEnv: true });
  await evaluate(`(() => { document.querySelector('#runner-tree input[data-run-id]').focus(); return true; })()`);
  await screenshot('runner-selection-dark.png');
  await startAndWait();
  await evaluate(`document.querySelectorAll('#runner-results .runner-row-head[data-expandable]')[0].click()`);
  await screenshot('runner-results-dark.png');
  await evaluate(`(() => { document.documentElement.dataset.theme = 'light'; return true; })()`);
  await screenshot('runner-results-light.png');
  await evaluate(`(() => { document.documentElement.dataset.theme = 'dark'; document.querySelector('#runner-close').click(); return true; })()`);
  await check('no runtime exceptions during the whole suite', `__runtimeErrors.length === 0`);

  console.log(`\n${results.length} checks passed`);
} finally {
  await close();
}
