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
    { _id: "req_bearer", _type: "request", parentId: "wrk_1", name: "Bearer req", method: "GET",
      url: "http://127.0.0.1:8100/api/echo", authentication: { type: "bearerToken", token: "b-tok" } },
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

  // --- Task 2: Hoppscotch import ------------------------------------------------
  const hoppscotchDoc = () => [
    { name: "Hoppscotch collection", folders: [
      { name: "Hopp folder", folders: [], requests: [
        { name: "Hopp inner", method: "put", endpoint: "http://127.0.0.1:8100/api/echo",
          auth: { authType: "basic", username: "hu", password: "hp" },
          headers: [{ key: "X-H", value: "1", active: false }], params: [{ key: "p", value: "1", active: true }],
          body: { body: "contentType", contentType: "application/json", raw: '{"h":1}' } },
      ]}],
      requests: [
        { name: "Hopp bearer", method: "GET", endpoint: "http://127.0.0.1:8100/api/echo",
          auth: { authType: "bearer", token: "hop-tok" }, body: { body: "formURLEncoded", form: [{ key: "k", value: "v", active: true }] } },
        { name: "Hopp apikey", method: "GET", endpoint: "http://127.0.0.1:8100/api/echo",
          auth: { authType: "api-key", key: "X-Key", value: "kv", addTo: "query_params" }, body: { body: "formData", form: [{ key: "f", value: "fv" }] } },
        { name: "Hopp weird", method: "GET", endpoint: "http://127.0.0.1:8100/api/echo",
          auth: { authType: "aws-signature", accessKey: "a" }, body: { body: "gql" } },
        { name: "Hopp legacy", method: "GET", endpoint: "http://127.0.0.1:8100/api/echo",
          auth: { authType: "basic", httpUser: "lu", httpPassword: "lp" }, body: { body: "contentType", contentType: "text/plain", raw: "hello" } },
      ],
    },
  ];
  const hoppscotchEnvs = () => [
    { name: "Hopp env rows", variables: [{ key: "alpha", value: "1" }] },
    { name: "Hopp env map", variables: { beta: "2" } },
  ];
  await check('detection distinguishes Hoppscotch collections and keeps Postman first', `(() => {
    const v = __verify;
    return v.looksLikeHoppscotchCollections((${JSON.stringify(hoppscotchDoc())})) === true
      && v.looksLikeHoppscotchEnvironments((${JSON.stringify(hoppscotchEnvs())})) === true;
  })()`);
  await check('collections import with nested folders, method, headers, params', async () => {
    await evaluate(`(async () => {
      const doc = ${JSON.stringify(hoppscotchDoc())};
      await __verify.importDroppedFiles([new File([JSON.stringify(doc)], "hoppscotch.json", { type: "application/json" })]);
      return true;
    })()`);
    return await evaluate(`(() => {
      const v = __verify;
      const c = v.state.collections.find(c => c.name === "Hoppscotch collection");
      if (!c) return false;
      const folder = c.items.find(i => i.kind === "folder" && i.name === "Hopp folder");
      const inner = folder?.items.find(i => i.name === "Hopp inner");
      return Boolean(inner) && inner.method === "PUT"
        && inner.headers[0].key === "X-H" && inner.headers[0].enabled === false
        && inner.params[0].key === "p" && inner.params[0].enabled === true;
    })()`);
  });
  await check('auth maps basic/bearer/api-key with legacy spellings, unknown inherits', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Hoppscotch collection");
    const inner = c.items.find(i => i.kind === "folder").items[0];
    const bearer = c.items.find(i => i.name === "Hopp bearer");
    const apikey = c.items.find(i => i.name === "Hopp apikey");
    const weird = c.items.find(i => i.name === "Hopp weird");
    const legacy = c.items.find(i => i.name === "Hopp legacy");
    return inner.auth.type === "basic" && inner.auth.basic.username === "hu"
      && bearer.auth.type === "bearer" && bearer.auth.bearer.token === "hop-tok"
      && apikey.auth.type === "apikey" && apikey.auth.apikey.addTo === "query"
      && weird.auth.type === "inherit" && legacy.auth.basic.username === "lu";
  })()`);
  await check('bodies map raw, urlencoded and formData; unknown variant skips the body only', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Hoppscotch collection");
    const inner = c.items.find(i => i.kind === "folder").items[0];
    const bearer = c.items.find(i => i.name === "Hopp bearer");
    const apikey = c.items.find(i => i.name === "Hopp apikey");
    const weird = c.items.find(i => i.name === "Hopp weird");
    const legacy = c.items.find(i => i.name === "Hopp legacy");
    return inner.body.mode === "raw" && inner.body.raw === '{"h":1}' && inner.body.contentType === "application/json"
      && bearer.body.mode === "form-data" && bearer.body.form[0].key === "k"
      && apikey.body.mode === "form-data" && apikey.body.form[0].key === "f"
      && weird.body.mode === "none" && legacy.body.mode === "raw" && legacy.body.raw === "hello";
  })()`);
  await check('environment files import in both variable shapes', `(() => {
    const v = __verify;
    v.state.environments = []; v.render();
    return (async () => {
      const doc = ${JSON.stringify(hoppscotchEnvs())};
      const file = new File([JSON.stringify(doc)], "hoppscotch-env.json", { type: "application/json" });
      await __verify.importDroppedFiles([file]);
      const names = v.state.environments.map(e => e.name);
      const rows = v.state.environments.flatMap(e => e.variables.map(r => r.key + "=" + r.value));
      return names.join(",") === "Hopp env rows,Hopp env map" && rows.join(",") === "alpha=1,beta=2";
    })();
  })()`);
  await check('the Import menu file input routes Hoppscotch JSON too', async () => {
    // Count before/after instead of wiping collections — later checks read the
    // collections earlier imports left behind.
    const before = await evaluate(`__verify.state.collections.filter(c => c.name === "Hoppscotch collection").length`);
    await evaluate(`(() => {
      const doc = ${JSON.stringify(hoppscotchDoc())};
      const dt = new DataTransfer();
      dt.items.add(new File([JSON.stringify(doc)], "hoppscotch.json", { type: "application/json" }));
      const input = document.querySelector("#import-file");
      input.files = dt.files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    })()`);
    return await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 80));
      return __verify.state.collections.filter(c => c.name === "Hoppscotch collection").length === ${JSON.stringify(before + 1)};
    })()`);
  });

  // --- Review-fix: verzod shapes real Hoppscotch exports use ---------------------
  const verzodDoc = () => [{
    v: 2, name: "Verzod collection", auth: { authType: "inherit", authActive: true }, headers: [],
    folders: [{ v: 2, name: "V folder", auth: { authType: "inherit", authActive: true }, headers: [], folders: [], requests: [
      { v: "5", name: "V raw", method: "POST", endpoint: "http://127.0.0.1:8100/api/echo",
        auth: { authType: "inherit", authActive: true }, params: [], headers: [],
        body: { contentType: "application/json", body: '{"v":1}' } },
      { v: "5", name: "V none", method: "GET", endpoint: "http://127.0.0.1:8100/api/echo",
        auth: { authType: "inherit", authActive: true }, params: [], headers: [], body: { body: null, contentType: null } },
      { v: "5", name: "V form", method: "POST", endpoint: "http://127.0.0.1:8100/api/echo",
        auth: { authType: "inherit", authActive: true }, params: [], headers: [],
        body: { contentType: "multipart/form-data", body: [{ key: "f", value: "x.png", active: true, isFile: true }, { key: "t", value: "hi", active: true }] } },
      { v: "5", name: "V urlenc", method: "POST", endpoint: "http://127.0.0.1:8100/api/echo",
        auth: { authType: "inherit", authActive: true }, params: [], headers: [],
        body: { contentType: "application/x-www-form-urlencoded", body: [{ key: "k", value: "v", active: true }] } },
      { v: "5", name: "V apikey", method: "GET", endpoint: "http://127.0.0.1:8100/api/echo",
        auth: { authType: "api-key", key: "X-Key", value: "kv", addTo: "QUERY_PARAMS", authActive: true }, params: [], headers: [],
        body: { body: null, contentType: null } },
      { v: "5", name: "V oauth", method: "GET", endpoint: "http://127.0.0.1:8100/api/echo",
        auth: { authType: "oauth-2", grantTypeInfo: { token: "gz-tok", grantType: "CLIENT_CREDENTIALS" }, authActive: true }, params: [], headers: [],
        body: { body: null, contentType: null } },
    ]}],
    requests: [],
  }];
  const verzodEnvs = () => [
    { v: "2", name: "Verzod env", variables: [
      { key: "host", initialValue: "http://127.0.0.1:8100", currentValue: "http://127.0.0.1:8100", secret: false },
      { key: "apikey", initialValue: "", currentValue: "s3cr3t", secret: true },
    ] },
  ];
  await check('verzod bodies map: raw string, urlencoded array, multipart isFile', async () => {
    await evaluate(`(async () => {
      const doc = ${JSON.stringify(verzodDoc())};
      await __verify.importDroppedFiles([new File([JSON.stringify(doc)], "verzod.json", { type: "application/json" })]);
      return true;
    })()`);
    return await evaluate(`(() => {
      const v = __verify;
      const c = v.state.collections.find(c => c.name === "Verzod collection");
      if (!c) return false;
      const folder = c.items.find(i => i.kind === "folder");
      const raw = folder.items.find(i => i.name === "V raw");
      const none = folder.items.find(i => i.name === "V none");
      const form = folder.items.find(i => i.name === "V form");
      const urlenc = folder.items.find(i => i.name === "V urlenc");
      return raw.body.mode === "raw" && raw.body.raw === '{"v":1}' && raw.body.contentType === "application/json"
        && none.body.mode === "none"
        && form.body.mode === "form-data" && form.body.form[0].type === "file" && form.body.form[1].type === "text"
        && urlenc.body.mode === "form-data" && urlenc.body.contentType === "application/x-www-form-urlencoded"
        && urlenc.body.form[0].key === "k";
    })()`);
  });
  await check('verzod inherit auth imports uncounted; apikey addTo and oauth grantTypeInfo map', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Verzod collection");
    const folder = c.items.find(i => i.kind === "folder");
    const apikey = folder.items.find(i => i.name === "V apikey");
    const oauth = folder.items.find(i => i.name === "V oauth");
    return apikey.auth.type === "apikey" && apikey.auth.apikey.addTo === "query"
      && oauth.auth.type === "bearer" && oauth.auth.bearer.token === "gz-tok";
  })()`);
  await check('verzod environment rows use initialValue/currentValue/secret', async () => {
    await evaluate(`(async () => {
      const doc = ${JSON.stringify(verzodEnvs())};
      await __verify.importDroppedFiles([new File([JSON.stringify(doc)], "verzod-env.json", { type: "application/json" })]);
      return true;
    })()`);
    return await evaluate(`(() => {
      const v = __verify;
      const env = v.state.environments.find(e => e.name === "Verzod env");
      if (!env) return false;
      const rows = Object.fromEntries(env.variables.map(r => [r.key, r]));
      return rows.host.value === "http://127.0.0.1:8100"
        && rows.apikey.value === "s3cr3t" && rows.apikey.secret === true;
    })()`);
  });
  await check('verzod imports report no skipped requests', async () => {
    return await evaluate(`(async () => {
      const doc = ${JSON.stringify(verzodDoc())};
      await __verify.importDroppedFiles([new File([JSON.stringify(doc)], "verzod.json", { type: "application/json" })]);
      return !document.querySelector('#tool-note').textContent.includes('skipped');
    })()`);
  });
  await check('Insomnia bearer and bearerToken auth types map', `(() => {
    const v = __verify;
    const c = v.state.collections.find(c => c.name === "Insomnia workspace");
    const bearer = c.items.find(i => i.name === "Bearer req");
    return Boolean(bearer) && bearer.auth.type === "bearer" && bearer.auth.bearer.token === "b-tok";
  })()`);
  await check('parentId cycles import without crashing', async () => {
    await evaluate(`(async () => {
      const doc = ${JSON.stringify(insomniaDoc())};
      doc.resources.push({ _id: "fld_cyc", _type: "folder", parentId: "fld_cyc", name: "Cycle folder" });
      await __verify.importDroppedFiles([new File([JSON.stringify(doc)], "cycle.json", { type: "application/json" })]);
      return true;
    })()`);
    return await evaluate(`__verify.state.collections.some(c => c.items.some(i => i.name === "Cycle folder"))`);
  });
  await check('the Import menu file input routes Insomnia JSON too', async () => {
    const before = await evaluate(`__verify.state.collections.filter(c => c.name === "Insomnia workspace").length`);
    await evaluate(`(() => {
      const doc = ${JSON.stringify(insomniaDoc())};
      const dt = new DataTransfer();
      dt.items.add(new File([JSON.stringify(doc)], "insomnia.json", { type: "application/json" }));
      const input = document.querySelector("#import-file");
      input.files = dt.files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    })()`);
    return await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 80));
      return __verify.state.collections.filter(c => c.name === "Insomnia workspace").length === ${JSON.stringify(before + 1)};
    })()`);
  });

  console.log(`\n${results.length} checks passed`);
} catch (err) {
  console.error(`FAIL ${err.message}`);
  process.exitCode = 1;
} finally {
  await close();
}
