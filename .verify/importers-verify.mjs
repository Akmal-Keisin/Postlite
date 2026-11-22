import assert from 'node:assert/strict';
import { evaluate, close, reload } from './tier1-cdp.mjs';

const results = [];
let executed = 0;
// Optional third arg: run only the first N checks (task-staged verification).
const maxChecks = Number(process.argv[3]) || Infinity;
const check = async (name, expression) => {
  if (executed >= maxChecks) return;
  // A function runs harness-side (multi-step flows); a string evaluates in the page.
  const value = typeof expression === "function" ? await expression() : await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  executed++;
  results.push(name);
  console.log(`PASS ${name}`);
};

// Insomnia v4 export fixture covering auth variants, body variants, a dangling
// parent, and a nested-data environment.
const insomniaDoc = () => ({
  _type: "export", __export_format: 4, __export_date: "2026-11-22T00:00:00.000Z", __export_source: "app",
  resources: [
    { _id: "wrk_1", _type: "workspace", name: "Insomnia workspace" },
    { _id: "fld_1", _type: "folder", parentId: "wrk_1", name: "Folder A" },
    { _id: "req_basic", _type: "request", parentId: "fld_1", name: "Basic req", method: "post",
      url: "http://127.0.0.1:8100/api/echo",
      headers: [{ name: "X-Test", value: "1" }, { name: "X-Off", value: "0", disabled: true }],
      authentication: { type: "basic", username: "u", password: "p" },
      body: { mimeType: "application/json", text: '{"a":1}' } },
    { _id: "req_form", _type: "request", parentId: "wrk_1", name: "Form req", method: "POST",
      url: "http://127.0.0.1:8100/api/echo",
      body: { mimeType: "application/x-www-form-urlencoded", params: [{ name: "a", value: "1" }, { name: "b", value: "2", disabled: true }] } },
    { _id: "req_oauth", _type: "request", parentId: "wrk_1", name: "OAuth req", method: "GET",
      url: "http://127.0.0.1:8100/api/echo", authentication: { type: "oauth2", accessToken: "tok-123" } },
    { _id: "req_digest", _type: "request", parentId: "wrk_missing", name: "Digest req", method: "GET",
      url: "http://127.0.0.1:8100/api/echo", authentication: { type: "digest", username: "u" } },
    { _id: "req_file", _type: "request", parentId: "wrk_1", name: "Multipart req", method: "POST",
      url: "http://127.0.0.1:8100/api/echo",
      body: { mimeType: "multipart/form-data", params: [{ name: "f", value: "C:/x/a.png", type: "file" }, { name: "t", value: "hi" }] } },
    { _id: "req_query", _type: "request", parentId: "wrk_1", name: "Query req", method: "GET",
      url: "http://127.0.0.1:8100/api/echo?z=9", parameters: [{ name: "q", value: "1" }] },
    { _id: "env_1", _type: "environment", parentId: "wrk_1", name: "Dev",
      data: { base_url: "http://127.0.0.1:8100", nested: { deep: "v" } } },
  ],
});
const postmanDoc = () => ({ info: { name: "Postman collection" }, item: [{ name: "P", request: { method: "GET", url: "http://x/" } }] });

try {
  await reload();
  await evaluate(`(() => {
    window.__runtimeErrors = []; window.addEventListener('error', e => __runtimeErrors.push(e.message));
  })()`);
  await evaluate(`(() => {
    const v = __verify;
    v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
    v.state.history = []; v.state.settings = v.normalizeSettings(); v.applyAppSettings();
    v.persist(); v.render();
    return true;
  })()`);

  // --- Task 1: Insomnia import -------------------------------------------------
  await check('looksLikeInsomnia detects the format-4 export, not Postman', `(() => {
    const r = {
      insomnia: __verify.looksLikeInsomnia((${JSON.stringify(insomniaDoc())})),
      postman: __verify.looksLikeInsomnia((${JSON.stringify(postmanDoc())})),
    };
    return r.insomnia === true && r.postman === false;
  })()`);
  await check('dropping the fixture imports the collection with a notify', async () => {
    return await evaluate(`(async () => {
      const doc = ${JSON.stringify(insomniaDoc())};
      const file = new File([JSON.stringify(doc)], "insomnia.json", { type: "application/json" });
      await __verify.importDroppedFiles([file]);
      const names = __verify.state.collections.map(c => c.name);
      const note = document.querySelector("#tool-note").textContent;
      // One digest auth is unmappable and must surface in the report line.
      return names.includes("Insomnia workspace") && note.includes("Imported") && note.includes("1 request(s) skipped");
    })()`);
  });
  await check('request maps method, url, headers with disabled flags', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Insomnia workspace");
    const folder = c.items.find(i => i.kind === "folder");
    const req = folder.items.find(i => i.name === "Basic req");
    return req.method === "POST" && req.url === "http://127.0.0.1:8100/api/echo"
      && req.headers[0].key === "X-Test" && req.headers[0].enabled === true
      && req.headers[1].key === "X-Off" && req.headers[1].enabled === false;
  })()`);
  await check('json body lands as raw with application/json', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Insomnia workspace");
    const req = c.items.find(i => i.kind === "folder").items.find(i => i.name === "Basic req");
    return req.body.mode === "raw" && req.body.raw === '{"a":1}' && req.body.contentType === "application/json";
  })()`);
  await check('urlencoded params become form rows, multipart file param becomes a file row', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Insomnia workspace");
    const form = c.items.find(i => i.name === "Form req");
    const multi = c.items.find(i => i.name === "Multipart req");
    return form.body.mode === "form-data" && form.body.form[0].key === "a" && form.body.form[0].enabled === true
      && form.body.form[1].key === "b" && form.body.form[1].enabled === false
      && multi.body.form[0].type === "file" && multi.body.form[0].value === "C:/x/a.png"
      && multi.body.form[1].type === "text";
  })()`);
  await check('auth maps basic/bearer-from-oauth2, digest skips and is counted', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Insomnia workspace");
    const basic = c.items.find(i => i.kind === "folder").items.find(i => i.name === "Basic req");
    const oauth = c.items.find(i => i.name === "OAuth req");
    const digest = c.items.find(i => i.name === "Digest req");
    return basic.auth.type === "basic" && basic.auth.basic.username === "u"
      && oauth.auth.type === "bearer" && oauth.auth.bearer.token === "tok-123"
      && digest.auth.type === "inherit" && v.state.collections.length === 1;
  })()`);
  await check('environment imports flattened and binds to the collection', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Insomnia workspace");
    const env = v.state.environments.find(e => e.name === "Dev");
    const rows = Object.fromEntries(env.variables.map(r => [r.key, r.value]));
    return Boolean(c.environmentId) && c.environmentId === env.id
      && rows.base_url === "http://127.0.0.1:8100" && rows["nested.deep"] === "v";
  })()`);
  await check('a dangling-parent request lands at the workspace root and the query table wins', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Insomnia workspace");
    const digest = c.items.find(i => i.name === "Digest req");
    const query = c.items.find(i => i.name === "Query req");
    return Boolean(digest) && digest.url === "http://127.0.0.1:8100/api/echo"
      && query.url === "http://127.0.0.1:8100/api/echo"
      && query.params.map(p => p.key).join(",") === "q";
  })()`);
  await check('non-v4 export fails with a message naming the format', async () => {
    await evaluate(`(() => {
      const doc = { ...${JSON.stringify(insomniaDoc())}, __export_format: 3 };
      return __verify.importDroppedFiles([new File([JSON.stringify(doc)], "old.json", { type: "application/json" })]);
    })()`);
    return await evaluate(`document.querySelector("#error").textContent.includes("4")`);
  });

  console.log(`\n${results.length} checks passed`);
} catch (err) {
  console.error(`FAIL ${err.message}`);
  process.exitCode = 1;
} finally {
  await close();
}
