import assert from 'node:assert/strict';
import { evaluate, close, reload } from './tier1-cdp.mjs';

const results = [];
let executed = 0;
// Optional third arg: run only the first N checks (task-staged verification).
const maxChecks = Number(process.argv[3]) || Infinity;
const check = async (name, expression) => {
  if (executed >= maxChecks) return;
  const value = await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  executed++;
  results.push(name);
  console.log(`PASS ${name}`);
};
const settle = () => evaluate('__verify.flushWorkspace().then(()=>true)');
// Seed one collection with a saved request and one unsaved scratch tab.
const seed = () => evaluate(`(() => {
  const v = __verify;
  v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
  v.state.history = []; v.state.settings = v.normalizeSettings(); v.applyAppSettings();
  const c = v.newCollection('History suite', { items: [
    v.newRequest({ name: 'Hello', url: 'http://127.0.0.1:8100/api/echo?n=1', method: 'GET' }),
  ]});
  v.state.collections.push(c);
  window.__scratch = v.newRequest({ name: 'Scratch', url: 'http://127.0.0.1:8100/api/echo?scratch=1', method: 'GET' });
  v.addTab(window.__scratch);
  v.persist(); v.render();
  return true;
})()`);
const reseedHistory = () => evaluate(`(() => {
  const v = __verify;
  v.state.history = [];
  v.recordHistory({ id: 'h3', method: 'GET', url: 'http://127.0.0.1:9/x', name: 'Dead' }, { error: 'connection refused', elapsed_ms: 0 });
  v.recordHistory({ id: 'h2', method: 'POST', url: 'http://127.0.0.1:8100/api/echo', name: '', params: [{ key: 'q', value: 'snapsecret', enabled: true }] }, { status: 404, status_text: 'Not Found', elapsed_ms: 7 });
  v.recordHistory({ id: 'h1', method: 'GET', url: 'http://127.0.0.1:8100/api/echo?n=1&token=supersecret', name: 'Hello' }, { status: 200, status_text: 'OK', elapsed_ms: 5 });
  v.recordHistory({ id: 'h1', method: 'GET', url: 'http://127.0.0.1:8100/api/echo?n=1&token=supersecret', name: 'Hello' }, { status: 200, status_text: 'OK', elapsed_ms: 5 });
  v.render();
  return true;
})()`);

try {
  await reload();
  await evaluate(`(() => {
    window.__runtimeErrors = []; window.addEventListener('error', e => __runtimeErrors.push(e.message));
  })()`);
  await seed();

  // --- Task 1: model + workspace plumbing -----------------------------------
  await check('normalizeHistory rejects junk and fills defaults', `(() => {
    const junk = __verify.normalizeHistory('x');
    const filled = __verify.normalizeHistory([{ method: 7 }]);
    return Array.isArray(junk) && junk.length === 0
      && filled.length === 1 && filled[0].method === 'GET' && filled[0].ok === false && filled[0].runs === 1
      && typeof filled[0].id === 'string' && typeof filled[0].at === 'string';
  })()`);
  await check('normalizeHistory caps at 100', `__verify.normalizeHistory(Array.from({length:150},(_,i)=>({id:'e'+i}))).length === 100`);
  await check('recordHistory prepends and caps', `(async () => {
    const v = __verify;
    v.state.history = [];
    for (let i = 0; i < 120; i++) v.recordHistory({ id: 'r'+i, method: 'GET', url: 'http://x/'+i, name: 'n'+i }, { status: 200, status_text: 'OK', elapsed_ms: 5 });
    return v.state.history.length === 100 && v.state.history[0].url === 'http://x/119' && !v.state.history.some(e => e.url === 'http://x/0');
  })()`);
  await check('consecutive identical sends collapse', `(() => {
    const v = __verify;
    v.state.history = [];
    const req = { id: 'r1', method: 'GET', url: 'http://x/same', name: 'Same' };
    v.recordHistory(req, { status: 200, status_text: 'OK', elapsed_ms: 5 });
    v.recordHistory(req, { status: 404, status_text: 'Not Found', elapsed_ms: 9 });
    const collapsed = v.state.history.length === 1 && v.state.history[0].runs === 2
      && v.state.history[0].status === 404 && v.state.history[0].elapsedMs === 9;
    v.recordHistory({ id: 'r1', method: 'GET', url: 'http://x/other', name: 'Same' }, { status: 200, status_text: 'OK', elapsed_ms: 5 });
    return collapsed && v.state.history.length === 2 && v.state.history[0].url === 'http://x/other';
  })()`);
  await check('workspacePayload writes history', `(() => {
    const payload = __verify.workspacePayload();
    const ws = payload.workspaces[0];
    return Array.isArray(ws.history) && ws.history.length === __verify.state.history.length
      && ws.history[0].id === __verify.state.history[0].id;
  })()`);
  await check('history survives reload with restore off', `(async () => {
    const v = __verify;
    v.state.settings.general.restoreLastSession = false;
    v.recordHistory(window.__scratch, { status: 200, status_text: 'OK', elapsed_ms: 5 });
    return v.state.history.length >= 1;
  })()`);

  if (executed >= 6) {
    await settle();
    await reload();
    await check('history entries survive the reload intact', `(() => {
      const v = __verify;
      const entry = v.state.history.find(e => e.url === 'http://127.0.0.1:8100/api/echo?scratch=1');
      return v.state.history.length >= 1 && entry
        && entry.method === 'GET' && entry.name === 'Scratch' && entry.ok === true && entry.status === 200;
    })()`);
  }

  // --- Task 2: recording sends in executeRequest ------------------------------
  await check('manual send records a saved-request entry', `(async () => {
    const v = __verify;
    v.state.history = [];
    const req = v.state.collections[0].items[0];
    const { response } = await v.executeRequest(req);
    const top = v.state.history[0];
    return !response.error && top && top.ref === req.id && top.ok === true && top.status === 200
      && top.url === 'http://127.0.0.1:8100/api/echo?n=1' && top.name === 'Hello';
  })()`);
  await check('unsaved tab records a snapshot without examples', `(async () => {
    const v = __verify;
    v.state.history = [];
    const req = v.newRequest({ name: 'Snap', url: 'http://127.0.0.1:8100/api/echo?snap=1', method: 'GET',
      examples: [{ name: 'old', body: 'x' }] });
    await v.executeRequest(req);
    const top = v.state.history[0];
    return top.ref === null && top.snapshot && top.snapshot.url === 'http://127.0.0.1:8100/api/echo?snap=1'
      && Array.isArray(top.snapshot.examples) && top.snapshot.examples.length === 0 && top.snapshot.response === undefined;
  })()`);
  await check('transport error records ok:false', `(async () => {
    const v = __verify;
    v.state.history = [];
    const req = v.newRequest({ name: 'Dead', url: 'http://127.0.0.1:9/x', method: 'GET' });
    const { response } = await v.executeRequest(req);
    const top = v.state.history[0];
    return Boolean(response.error) && top && top.ok === false && top.status === 0 && top.ref === null && Boolean(top.snapshot);
  })()`);
  await check('authored url is stored unresolved', `(async () => {
    const v = __verify;
    v.state.history = [];
    const c = v.newCollection('Var suite', { items: [
      v.newRequest({ name: 'Templated', url: '{{base}}/api/echo?tpl=1', method: 'GET' }),
    ], variables: [{ key: 'base', value: 'http://127.0.0.1:8100', enabled: true }] });
    v.state.collections.push(c);
    const { response } = await v.executeRequest(c.items[0]);
    const top = v.state.history[0];
    return !response.error && top && top.url === '{{base}}/api/echo?tpl=1' && top.ref === c.items[0].id;
  })()`);
  await check('runner records one entry per request', `(async () => {
    const v = __verify;
    v.state.history = [];
    const c = v.newCollection('Run suite', { items: ['a', 'b', 'c'].map(n =>
      v.newRequest({ name: 'Run ' + n, url: 'http://127.0.0.1:8100/api/echo?run=' + n, method: 'GET' })) });
    v.state.collections.push(c);
    v.openRunnerDialog(c.id);
    document.querySelector('#runner-run').click();
    const done = await new Promise(resolve => {
      const start = Date.now();
      const timer = setInterval(() => {
        if (!v.runnerState.active && v.runnerState.phase === 'done') { clearInterval(timer); resolve(true); }
        else if (Date.now() - start > 20000) { clearInterval(timer); resolve(false); }
      }, 30);
    });
    document.querySelector('#runner-close')?.click();
    const top3 = v.state.history.slice(0, 3).map(e => e.name).sort();
    return done && top3.join(',') === 'Run a,Run b,Run c'
      && v.state.history.slice(0, 3).every(e => e.ref && e.ok && e.status === 200);
  })()`);

  // --- Task 3: history sidebar panel ------------------------------------------
  await check('rail switches to history view', `(() => {
    document.querySelector('#rail-history').click();
    const v = __verify;
    return document.querySelector('#sidebar-title').textContent === 'History'
      && !document.querySelector('#history').hidden
      && document.querySelector('#collections').hidden
      && v.state.sidebarView === 'history';
  })()`);
  await check('empty state copy', `(() => {
    const v = __verify;
    v.state.history = [];
    v.renderSidebar();
    return document.querySelector('#history').textContent.includes('No history yet')
      && document.querySelector('#history').textContent.includes('Send a request');
  })()`);
  await reseedHistory();
  await check('rows render method, name, status, time', `(() => {
    const v = __verify;
    v.renderSidebar();
    const rows = [...document.querySelectorAll('#history .history-row')];
    const hello = rows.find(r => r.textContent.includes('Hello'));
    const post = rows.find(r => r.querySelector('.history-status')?.textContent === '404');
    const dead = rows.find(r => r.querySelector('.history-status')?.textContent === 'ERR');
    return rows.length === 3 && hello && post && dead
      && hello.querySelector('.history-method').textContent === 'GET'
      && hello.querySelector('.history-status').textContent === '200'
      && hello.querySelector('.history-time').textContent.trim().length > 0
      && hello.querySelector('.history-runs')?.textContent === '2'
      && hello.querySelector('.history-url')?.textContent.includes('/api/echo?n=1')
      && !post.textContent.includes('snapsecret')
      && post.querySelector('.history-name').textContent === 'http://127.0.0.1:8100/api/echo'
      && !document.querySelector('#history-clear').hidden;
  })()`);
  await check('filter narrows history rows', `(() => {
    const v = __verify;
    v.state.filter = 'echo';
    v.renderSidebar();
    const narrowed = document.querySelectorAll('#history .history-row').length;
    v.state.filter = 'zzzz-nothing';
    v.renderSidebar();
    const noMatch = document.querySelector('#history').textContent.includes('No matches');
    v.state.filter = '';
    v.renderSidebar();
    return narrowed === 2 && noMatch && document.querySelectorAll('#history .history-row').length === 3;
  })()`);
  await check('clear-all empties and persists', `(async () => {
    document.querySelector('#history-clear').click();
    const dialog = document.querySelector('#confirm-dialog');
    if (!dialog.open) return false;
    document.querySelector('#confirm-ok').click();
    await new Promise(r => setTimeout(r, 50));
    const payload = __verify.workspacePayload();
    return __verify.state.history.length === 0
      && Array.isArray(payload.workspaces[0].history)
      && payload.workspaces[0].history.length === 0;
  })()`);

  console.log(`\n${results.length} checks passed`);
} catch (err) {
  console.error(`FAIL ${err.message}`);
  process.exitCode = 1;
} finally {
  await close();
}
