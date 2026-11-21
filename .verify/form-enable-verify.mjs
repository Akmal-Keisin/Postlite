import assert from 'node:assert/strict';
import { writeFileSync, unlinkSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { evaluate, close, reload } from './tier1-cdp.mjs';

const results = [];
const check = async (name, expression) => {
  const value = await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  results.push(name);
  console.log(`PASS ${name}`);
};
const click = selector => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
const E = 'http://127.0.0.1:8100/api/echo';
const seed = () => evaluate(`(() => {
  const v = __verify;
  v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
  v.state.settings = v.normalizeSettings(); v.applyAppSettings();
  const req = v.newRequest({
    name: 'Rows', url: '${E}', method: 'POST',
    params: [ { key: 'on', value: '1', enabled: true }, { key: 'off', value: '2', enabled: true } ],
    body: { mode: 'form-data', form: [
      { key: 'keep', value: 'yes', enabled: true, type: 'text' },
      { key: 'drop', value: 'no', enabled: true, type: 'text' },
    ]},
  });
  const c = v.newCollection('Rows suite');
  c.items = [req]; // share the tab's object so UI edits reach the stored node
  v.state.collections.push(c);
  v.addTab(req);
  v.persist(); v.render();
  return true;
})()`);

const fixturePath = fileURLToPath(new URL('./kv-check-fixture.txt', import.meta.url));
writeFileSync(fixturePath, 'FILE-CONTENT-123');

try {
  await reload();
  await evaluate(`(() => { window.__runtimeErrors = []; window.addEventListener('error', e => __runtimeErrors.push(e.message)); })()`);
  await seed();

  await check('param rows render include checkboxes, checked by default',
    `(() => {
      const boxes = [...document.querySelectorAll('#params-table tbody input[data-field="enabled"]')];
      return boxes.length === 2 && boxes.every(b => b.checked);
    })()`);
  await check('form-data rows render include checkboxes, checked by default',
    `(() => {
      const boxes = [...document.querySelectorAll('#form-table tbody input[data-field="enabled"]')];
      return boxes.length === 2 && boxes.every(b => b.checked);
    })()`);

  // Uncheck the second param and the second form field through the real UI.
  await evaluate(`document.querySelectorAll('#params-table tbody input[data-field="enabled"]')[1].click()`);
  await evaluate(`document.querySelectorAll('#form-table tbody input[data-field="enabled"]')[1].click()`);
  await check('unchecking writes enabled=false into the request model',
    `(() => { const r = __verify.state.tabs[0].req; return r.params[1].enabled === false && r.body.form[1].enabled === false; })()`);
  await check('unchecked param is not sent',
    `(async () => { const { response } = await __verify.executeRequest(__verify.state.tabs[0].req); return JSON.parse(response.body).url === '/api/echo?on=1'; })()`);
  await check('unchecked form field is not sent',
    `(async () => { const { response } = await __verify.executeRequest(__verify.state.tabs[0].req); const b = JSON.parse(response.body).body; return b.includes('name="keep"') && !b.includes('name="drop"'); })()`);
  await check('re-checking a form field sends it again',
    `(async () => {
      document.querySelectorAll('#form-table tbody input[data-field="enabled"]')[1].click();
      const { response } = await __verify.executeRequest(__verify.state.tabs[0].req);
      return JSON.parse(response.body).body.includes('name="drop"');
    })()`);
  await check('unchecked file row is skipped',
    `(async () => {
      const r = __verify.state.tabs[0].req;
      r.body.form.push({ key: 'doc', value: 'C:/secret/report.pdf', type: 'file', enabled: false });
      const { response } = await __verify.executeRequest(r);
      r.body.form.pop();
      const b = JSON.parse(response.body).body;
      return !b.includes('name="doc"') && !b.includes('report.pdf');
    })()`);
  await check('new form rows default to checked',
    `(() => {
      document.querySelector('[data-add="form"]').click();
      const boxes = [...document.querySelectorAll('#form-table tbody input[data-field="enabled"]')];
      const added = boxes[boxes.length - 1];
      const ok = added && added.checked;
      document.querySelector('#form-table tbody tr:last-child [data-act="drop-row"]').click();
      return ok;
    })()`);
  // Uncheck "drop" again so the export/round-trip checks see a disabled row.
  await evaluate(`document.querySelectorAll('#form-table tbody input[data-field="enabled"]')[1].click()`);
  await check('code export resolves only checked rows',
    `(() => {
      const t = __verify.exportTarget(__verify.state.tabs[0].req);
      return t.url === '${E}?on=1' && t.form.some(f => f.key === 'keep') && !t.form.some(f => f.key === 'drop');
    })()`);
  await check('Postman round-trip preserves the disabled flags',
    `(() => {
      const v = __verify, c = v.state.collections[0];
      const pm = JSON.parse(JSON.stringify(v.toPostmanCollection(c)));
      const item = pm.item[0].request;
      const exported = item.url.query[1].disabled === true && item.body.formdata[1].disabled === true;
      const back = v.fromPostman(pm);
      const r = back.items[0];
      return exported && r.params[1].enabled === false && r.body.form[1].enabled === false && r.params[0].enabled === true;
    })()`);
  await check('unchecked state persists into the workspace payload',
    `(async () => {
      await __verify.flushWorkspace();
      const payload = JSON.parse(JSON.stringify(__verify.workspacePayload()));
      const ws = payload.workspaces.find(w => w.id === __verify.state.activeWorkspaceId);
      const r = ws.collections[0].items[0];
      return r.params[1].enabled === false;
    })()`);
  await check('form-data table header reserves the check column',
    `!!document.querySelector('#form-table thead th.c-check')`);
  await check('manual Send excludes unchecked rows too',
    `(async () => {
      document.querySelector('#send').click();
      await new Promise(r => { const t = setInterval(() => { if (__verify.state.tabs[0].response && !document.querySelector('#send').disabled) { clearInterval(t); r(); } }, 30); setTimeout(() => { clearInterval(t); r(); }, 8000); });
      const resp = __verify.state.tabs[0].response;
      const echo = JSON.parse(resp.body);
      return echo.url === '/api/echo?on=1' && echo.body.includes('name="keep"') && !echo.body.includes('name="drop"');
    })()`);
  await evaluate(`__verify.flushWorkspace().then(()=>true)`);
  await reload();
  await evaluate(`(() => { window.__runtimeErrors = []; window.addEventListener('error', e => __runtimeErrors.push(e.message)); })()`);
  await check('after reload the rows are still unchecked in model and UI',
    `(() => {
      const req = __verify.state.tabs[0].req;
      const pbox = document.querySelectorAll('#params-table tbody input[data-field="enabled"]')[1];
      const fbox = document.querySelectorAll('#form-table tbody input[data-field="enabled"]')[1];
      return req.params[1].enabled === false && req.body.form[1].enabled === false && pbox && !pbox.checked && fbox && !fbox.checked;
    })()`);
  await check('a checked file row uploads its bytes; unchecked does not',
    `(async () => {
      const v = __verify, req = v.state.tabs[0].req;
      req.body.form.push({ key: 'doc', value: ${JSON.stringify(fixturePath)}, type: 'file', enabled: false });
      const off = await v.executeRequest(req);
      req.body.form[req.body.form.length - 1].enabled = true;
      const on = await v.executeRequest(req);
      req.body.form.pop();
      return !JSON.parse(off.response.body).body.includes('FILE-CONTENT-123') && JSON.parse(on.response.body).body.includes('FILE-CONTENT-123');
    })()`);
  await check('no runtime exceptions', `__runtimeErrors.length === 0`);

  console.log(`\n${results.length} checks passed`);
} finally {
  await close();
  try { unlinkSync(fixturePath); } catch {}
}
