import assert from 'node:assert/strict';
import { evaluate, close, reload } from './tier1-cdp.mjs';

const results = [];
let executed = 0;
// Optional third arg: run only the first N checks (task-staged verification).
const maxChecks = Number(process.argv[3]) || Infinity;
const check = async (name, expression) => {
  if (executed >= maxChecks) return;
  const value = typeof expression === "function" ? await expression() : await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  executed++;
  results.push(name);
  console.log(`PASS ${name}`);
};
const seed = () => evaluate(`(() => {
  const v = __verify;
  v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
  v.state.history = []; v.state.settings = v.normalizeSettings(); v.applyAppSettings();
  const c = v.newCollection('OAuth2 suite', { items: [
    v.newRequest({ name: 'Authed', url: 'http://127.0.0.1:8100/api/echo', method: 'GET' }),
  ]});
  v.state.collections.push(c);
  v.addTab(c.items[0]);
  v.render();
  return true;
})()`);
// Select OAuth 2.0 in the request auth tab, switch grant, fill the fields.
const configureOAuth2 = (grant, fields) => evaluate(`(async () => {
  const v = __verify;
  v.state.tabs = []; v.render();
  v.addTab(v.state.collections[0].items[0]);
  const select = document.querySelector('#req-auth-type');
  select.value = 'oauth2';
  select.dispatchEvent(new Event('change', { bubbles: true }));
  const host = document.querySelector('#req-auth-fields');
  const set = (key, value) => {
    const el = host.querySelector(\`[data-k="\${key}"]\`);
    el.value = value;
    el.dispatchEvent(new Event('input', { bubbles: true }));
  };
  set('grantType', ${JSON.stringify(grant)});
  for (const [key, value] of Object.entries(${JSON.stringify(fields)})) set(key, value);
  return Boolean(host.querySelector('[data-oauth-get]'));
})()`);
const clickGetToken = () => evaluate(`(() => {
  document.querySelector('#req-auth-fields [data-oauth-get]').click();
  return true;
})()`);
const waitStatus = (needle, timeoutMs = 15000) => evaluate(`(async () => {
  const start = Date.now();
  while (Date.now() - start < ${JSON.stringify(timeoutMs)}) {
    const text = document.querySelector('#req-auth-fields .oauth2-status').textContent;
    if (text.includes(${JSON.stringify(needle)})) return text;
    await new Promise(r => setTimeout(r, 60));
  }
  return document.querySelector('#req-auth-fields .oauth2-status').textContent || '(timeout)';
})()`);
const hits = async () => (await evaluate(`(async () => (await (await fetch('http://127.0.0.1:8100/api/oauth/hits')).json()))()`)).hits;

try {
  await reload();
  await evaluate(`(() => {
    window.__runtimeErrors = []; window.addEventListener('error', e => __runtimeErrors.push(e.message));
  })()`);
  await seed();

  // --- Task 2: client credentials + cache + send integration --------------------
  await check('selecting OAuth 2.0 renders CC fields and persists without the secret', async () => {
    await configureOAuth2("client_credentials", { tokenUrl: "http://127.0.0.1:8100/api/oauth/token", clientId: "cid" });
    return await evaluate(`(() => {
      const auth = __verify.state.collections[0].items[0].auth;
      const persisted = JSON.stringify(__verify.workspacePayload());
      return auth.type === 'oauth2' && auth.oauth2.grantType === 'client_credentials'
        && auth.oauth2.tokenUrl === 'http://127.0.0.1:8100/api/oauth/token'
        && !Object.hasOwn(auth.oauth2, 'clientSecret')
        && !persisted.includes('"clientSecret"');
    })()`);
  });
  await check('setting the secret stores it in the keyring and not in the page state', async () => {
    await evaluate(`(() => {
      const input = document.querySelector('#req-auth-fields [data-oauth-secret]');
      input.value = 'sekrit';
      input.dispatchEvent(new Event('input', { bubbles: true }));
      return true;
    })()`);
    const stored = await evaluate(`(async () => {
      const secret = await __TAURI__.core.invoke('oauth2_secret_get', { nodeId: __verify.state.collections[0].items[0].id });
      return secret === 'sekrit';
    })()`);
    const clean = await evaluate(`!JSON.stringify(__verify.workspacePayload()).includes('sekrit')`);
    return stored === true && clean === true;
  });
  await check('Get token acquires and caches against the fixture endpoint', async () => {
    const before = await hits();
    await clickGetToken();
    const status = await waitStatus("Token acquired");
    const after = await hits();
    return status.includes("Token acquired") && after === before + 1;
  });
  await check('sending an oauth2 request carries the bearer token', async () => {
    return await evaluate(`(async () => {
      const v = __verify;
      const { response } = await v.executeRequest(v.state.tabs[0].req);
      const headers = JSON.parse(response.body ?? '{}').headers ?? {};
      return response.status === 200 && (headers.authorization ?? '').toLowerCase() === 'bearer runner-token-123';
    })()`);
  });
  await check('unchanged config reuses the cached token without a new hit', async () => {
    const before = await hits();
    return await evaluate(`(async () => {
      const v = __verify;
      await v.executeRequest(v.state.tabs[0].req);
      return true;
    })()`) && (await hits()) === before;
  });
  await check('a wrong secret fails the send with oauth2: and never hits the wire', async () => {
    await evaluate(`(() => {
      const input = document.querySelector('#req-auth-fields [data-oauth-secret]');
      input.value = 'wrong';
      input.dispatchEvent(new Event('input', { bubbles: true }));
      return true;
    })()`);
    const before = await hits();
    return await evaluate(`(async () => {
      const v = __verify;
      // A fresh token URL makes the config identity miss the warm cache, so the
      // wrong secret is actually read server-side.
      v.state.tabs[0].req.auth.oauth2.tokenUrl = 'http://127.0.0.1:8100/api/oauth/token?wrong=1';
      const { response } = await v.executeRequest(v.state.tabs[0].req);
      return response.error.includes('oauth2:') && response.error.includes('401')
        && v.state.history[0].ok === false && v.state.history[0].status === 0;
    })()`) && (await hits()) === before + 1;
  });

  console.log(`\n${results.length} checks passed`);
} catch (err) {
  console.error(`FAIL ${err.message}`);
  process.exitCode = 1;
} finally {
  await close();
}
