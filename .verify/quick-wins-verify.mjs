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
// Seed one collection with a request against the echo fixture plus a variable,
// and open its tab — the shared baseline for every suite below.
const seed = () => evaluate(`(() => {
  const v = __verify;
  v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
  v.state.history = []; v.state.settings = v.normalizeSettings(); v.applyAppSettings();
  const c = v.newCollection('Quick wins suite', { items: [
    v.newRequest({ name: 'Hello', url: 'http://127.0.0.1:8100/api/echo?n=1', method: 'GET' }),
  ], variables: [{ key: 'base', value: 'http://127.0.0.1:8100', enabled: true },
    { key: 'secretKey', value: 's3cr3t-value', enabled: true, secret: true }] });
  v.state.collections.push(c);
  v.addTab(c.items[0]);
  v.persist(); v.render();
  return true;
})()`);

try {
  await reload();
  await evaluate(`(() => {
    window.__runtimeErrors = []; window.addEventListener('error', e => __runtimeErrors.push(e.message));
  })()`);
  await seed();

  // --- Task 1: dynamic variables ----------------------------------------------
  await check('dynamic tokens resolve with the right shapes', `(() => {
    const v = __verify;
    const vars = new Map();
    vars.onWire = true;
    const ts = v.interpolate('{{\\$timestamp}}', vars);
    const iso = v.interpolate('{{\\$isoTimestamp}}', vars);
    const int = v.interpolate('{{\\$randomInt}}', vars);
    const guid = v.interpolate('{{\\$guid}}', vars);
    const uuidRe = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
    return /^\\d{10}$/.test(ts) && !Number.isNaN(Date.parse(iso))
      && Number.isInteger(Number(int)) && Number(int) >= 0 && Number(int) <= 1000
      && uuidRe.test(guid);
  })()`);
  await check('unknown tokens stay literal', `__verify.interpolate('{{nope}}', new Map()) === '{{nope}}'`);
  await check('vars not marked on-wire keep tokens literal', `__verify.interpolate('{{\\$timestamp}}', new Map()) === '{{\\$timestamp}}'`);
  await check('exportTarget keeps dynamic tokens literal', `(async () => {
    const v = __verify;
    const c = v.state.collections[0];
    c.items[0].url = '{{base}}/api/echo?ts={{\\$timestamp}}';
    const target = v.exportTarget(c.items[0]);
    return target.url === 'http://127.0.0.1:8100/api/echo?ts={{\\$timestamp}}';
  })()`);
  await check('peek overlay shows dynamic scope', `(() => {
    const v = __verify;
    v.state.collections[0].items[0].url = '{{\\$timestamp}}';
    v.render();
    const req = v.state.tabs[0].req;
    const overlay = document.querySelector('#var-overlay');
    overlay.hidden = false;
    v.renderVarOverlay(['\\$timestamp'], req.url);
    const row = overlay.querySelector('.var-row');
    return row && row.textContent.includes('dynamic') && !row.classList.contains('missing');
  })()`);

  console.log(`\n${results.length} checks passed`);
} catch (err) {
  console.error(`FAIL ${err.message}`);
  process.exitCode = 1;
} finally {
  await close();
}
