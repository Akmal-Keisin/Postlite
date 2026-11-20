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

  console.log(`\n${results.length} checks passed`);
} finally {
  await close();
}
