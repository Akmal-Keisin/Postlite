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
  v.recordHistory({ id: 'h1', method: 'GET', url: 'http://127.0.0.1:8100/api/echo?n=1', name: 'Hello' }, { status: 200, status_text: 'OK', elapsed_ms: 5 });
  v.recordHistory({ id: 'h2', method: 'POST', url: 'http://127.0.0.1:8100/api/echo', name: '' }, { status: 404, status_text: 'Not Found', elapsed_ms: 7 });
  v.recordHistory({ id: 'h3', method: 'GET', url: 'http://127.0.0.1:9/x', name: 'Dead' }, { error: 'connection refused', elapsed_ms: 0 });
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

  console.log(`\n${results.length} checks passed`);
} catch (err) {
  console.error(`FAIL ${err.message}`);
  process.exitCode = 1;
} finally {
  await close();
}
