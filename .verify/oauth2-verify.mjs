import assert from 'node:assert/strict';
import { connect } from 'node:net';
import { evaluate, close, reload } from './tier1-cdp.mjs';

const results = [];
let executed = 0;
const maxChecks = Number(process.argv[3]) || Infinity;
const check = async (name, expression) => {
  if (executed >= maxChecks) return;
  const value = typeof expression === 'function' ? await expression() : await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  executed++;
  results.push(name);
  console.log(`PASS ${name}`);
};
const seed = () => evaluate(`(() => {
  const v = __verify;
  v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
  v.state.history = []; v.state.settings = v.normalizeSettings(); v.applyAppSettings();
  v.oauth2Cache.clear(); v.oauth2Pending.clear(); v.oauth2Flows.clear(); v.oauth2UiState.clear(); v.oauth2Secrets.clear();
  const c = v.newCollection('OAuth2 suite', { items: [
    v.newRequest({ name: 'Authed', url: 'http://127.0.0.1:8100/api/echo', method: 'GET' }),
    v.newRequest({ name: 'Second', url: 'http://127.0.0.1:8100/api/echo', method: 'GET' }),
  ]});
  v.state.collections.push(c);
  v.addTab(c.items[0]);
  v.render();
  return true;
})()`);
const configureOAuth2 = (grant, fields, index = 0) => evaluate(`(async () => {
  const v = __verify;
  v.state.tabs = []; v.render();
  v.addTab(v.state.collections[0].items[${index}]);
  const select = document.querySelector('#req-auth-type');
  select.value = 'oauth2';
  select.dispatchEvent(new Event('change', { bubbles: true }));
  const host = document.querySelector('#req-auth-fields');
  const set = (key, value) => {
    const el = host.querySelector(\`[data-k="\${key}"]\`);
    if (!el) return false;
    el.value = value;
    el.dispatchEvent(new Event(el.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
    return true;
  };
  set('grantType', ${JSON.stringify(grant)});
  for (const [key, value] of Object.entries(${JSON.stringify(fields)})) set(key, value);
  return Boolean(host.querySelector('[data-oauth-get]'));
})()`);
const setSecret = (value) => evaluate(`(async () => {
  const input = document.querySelector('#req-auth-fields [data-oauth-secret]');
  input.value = ${JSON.stringify(value)};
  input.dispatchEvent(new Event('input', { bubbles: true }));
  await __verify.flushOAuth2Secret(__verify.state.tabs[0].req.id);
  return true;
})()`);
const clickGetToken = () => evaluate(`(() => {
  document.querySelector('#req-auth-fields [data-oauth-get]').click();
  return true;
})()`);
const waitStatus = (needle, timeoutMs = 15000) => evaluate(`(async () => {
  const start = Date.now();
  while (Date.now() - start < ${JSON.stringify(timeoutMs)}) {
    const text = document.querySelector('#req-auth-fields .oauth2-status')?.textContent ?? '';
    if (text.includes(${JSON.stringify(needle)})) return text;
    await new Promise(r => setTimeout(r, 60));
  }
  return document.querySelector('#req-auth-fields .oauth2-status')?.textContent || '(timeout)';
})()`);
const getJson = async (path) => (await fetch(`http://127.0.0.1:8100${path}`)).json();
const oauthStats = async () => ({ ...(await getJson('/api/oauth/hits')), echoHits: (await getJson('/api/echo/hits')).hits });
const followAuthorize = async (follow = true) => {
  await new Promise(resolve => setTimeout(resolve, 150));
  const url = await evaluate('window.__oauth2AuthorizeUrl');
  let response;
  try { response = await fetch(url, { redirect: 'manual' }); }
  catch (error) { throw new Error(`authorize fetch failed for ${url}: ${error.message}`); }
  const location = response.headers.get('location');
  if (follow && location) {
    try { await fetch(location); }
    catch (error) { throw new Error(`loopback fetch failed for ${location}: ${error.message}`); }
  }
  return { url, location };
};
const waitForNewAuthorizeUrl = (previous) => evaluate(`(async () => {
  const start = Date.now();
  while (Date.now() - start < 10000) {
    if (window.__oauth2AuthorizeUrl && window.__oauth2AuthorizeUrl !== ${JSON.stringify(previous)}) return true;
    await new Promise(r => setTimeout(r, 60));
  }
  return false;
})()`);
const clickAndWaitAuthorize = async () => {
  const previous = await evaluate('window.__oauth2AuthorizeUrl ?? null');
  await clickGetToken();
  return waitForNewAuthorizeUrl(previous);
};
const expireCachedToken = () => evaluate(`(() => {
  const id = __verify.state.tabs[0].req.id;
  for (const [key, entry] of __verify.oauth2Cache) if (key.includes(id)) entry.expiresAt = 0;
  return true;
})()`);

try {
  await reload();
  await evaluate(`(() => {
    window.__runtimeErrors = []; window.addEventListener('error', e => __runtimeErrors.push(e.message));
  })()`);
  await seed();

  await check('selecting OAuth 2.0 renders client-credentials fields without persisting a secret', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid' });
    return await evaluate(`(() => {
      const auth = __verify.state.collections[0].items[0].auth;
      const persisted = JSON.stringify(__verify.workspacePayload());
      return auth.type === 'oauth2' && auth.oauth2.grantType === 'client_credentials'
        && auth.oauth2.tokenUrl === 'http://127.0.0.1:8100/api/oauth/token'
        && !Object.hasOwn(auth.oauth2, 'clientSecret') && !persisted.includes('"clientSecret"');
    })()`);
  });
  await check('the secret field writes to the keyring and never enters page state', async () => {
    await setSecret('sekrit');
    return await evaluate(`(async () => {
      const secret = await __TAURI__.core.invoke('oauth2_secret_get', { nodeId: __verify.state.tabs[0].req.id });
      return secret === 'sekrit' && !JSON.stringify(__verify.workspacePayload()).includes('sekrit');
    })()`);
  });
  await check('punctuation and case in node ids cannot collide in the secret store', () => evaluate(`(async () => {
    const invoke = window.__TAURI__.core.invoke;
    await invoke('oauth2_secret_set', { nodeId: 'request:a', secret: 'secret-a' });
    await invoke('oauth2_secret_set', { nodeId: 'requestA', secret: 'secret-b' });
    const a = await invoke('oauth2_secret_get', { nodeId: 'request:a' });
    const b = await invoke('oauth2_secret_get', { nodeId: 'requestA' });
    await invoke('oauth2_secret_clear', { nodeId: 'request:a' });
    await invoke('oauth2_secret_clear', { nodeId: 'requestA' });
    return a === 'secret-a' && b === 'secret-b';
  })()`));
  await check('Get token acquires and caches against the fixture endpoint', async () => {
    const before = await oauthStats();
    await clickGetToken();
    const status = await waitStatus('Token acquired');
    const after = await oauthStats();
    return status.includes('Token acquired') && after.hits === before.hits + 1;
  });
  await check('sending an oauth2 request carries the bearer token', () => evaluate(`(async () => {
    const { response } = await __verify.executeRequest(__verify.state.tabs[0].req);
    const headers = JSON.parse(response.body ?? '{}').headers ?? {};
    return response.status === 200 && (headers.authorization ?? '').toLowerCase() === 'bearer runner-token-123';
  })()`));
  await check('unchanged configuration reuses the cached token without a new hit', async () => {
    const before = await oauthStats();
    const ok = await evaluate(`(async () => {
      const { response } = await __verify.executeRequest(__verify.state.tabs[0].req);
      return response.status === 200;
    })()`);
    return ok && (await oauthStats()).hits === before.hits;
  });
  await check('a wrong secret fails before the request reaches the wire', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid' });
    await setSecret('wrong-secret-421');
    await evaluate(`__verify.state.tabs[0].req.auth.oauth2.tokenUrl = 'http://127.0.0.1:8100/api/oauth/token?wrong=1'`);
    const before = await oauthStats();
    const sanitized = await evaluate(`(async () => {
      const { response } = await __verify.executeRequest(__verify.state.tabs[0].req);
      const exported = JSON.stringify({ response, history: __verify.state.history, runner: __verify.runnerReport() });
      return !exported.includes('wrong-secret-421') && !exported.includes('rejected secret')
        && response.error.includes('oauth2:') && response.error.includes('401')
        && __verify.state.history[0].ok === false && __verify.state.history[0].status === 0;
    })()`);
    const after = await oauthStats();
    return sanitized && after.hits === before.hits + 1 && after.echoHits === before.echoHits;
  });
  await check('an oversized token response is rejected without leaking contents', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token?oversized=1', clientId: 'cid' });
    await setSecret('sekrit');
    return await evaluate(`(async () => {
      const { response } = await __verify.executeRequest(__verify.state.tabs[0].req);
      return response.error.includes('too large')
        && !JSON.stringify({ response, history: __verify.state.history }).includes('sekrit');
    })()`);
  });
  await check('identical configurations on different nodes do not share cached tokens', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid' });
    await setSecret('sekrit');
    await clickGetToken();
    await waitStatus('Token acquired');
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid' }, 1);
    return await evaluate(`(() => {
      const first = __verify.state.collections[0].items[0];
      const second = __verify.state.collections[0].items[1];
      const firstHeaders = __verify.exportTarget(first).headers;
      const secondHeaders = __verify.exportTarget(second).headers;
      return firstHeaders.some(h => h.key.toLowerCase() === 'authorization' && h.value === 'Bearer runner-token-123')
        && !secondHeaders.some(h => h.key.toLowerCase() === 'authorization');
    })()`);
  });
  await check('grant switching renders authorization-code fields and a fixed redirect URI', () => evaluate(`(() => {
    const select = document.querySelector('#req-auth-type');
    select.value = 'oauth2'; select.dispatchEvent(new Event('change', { bubbles: true }));
    const grant = document.querySelector('#req-auth-fields [data-k="grantType"]');
    grant.value = 'authorization_code'; grant.dispatchEvent(new Event('change', { bubbles: true }));
    const port = document.querySelector('#req-auth-fields [data-k="redirectPort"]');
    port.value = '8123'; port.dispatchEvent(new Event('input', { bubbles: true }));
    return Boolean(document.querySelector('#req-auth-fields [data-k="authorizeUrl"]'))
      && document.querySelector('#oauth2-redirect').textContent === 'http://127.0.0.1:8123/callback'
      && Boolean(document.querySelector('[data-oauth-paste]'));
  })()`));
  await check('Postman OAuth2 round-trips configuration but never the client secret', async () => {
    await configureOAuth2('authorization_code', {
      authorizeUrl: 'http://127.0.0.1:8100/api/oauth/authorize',
      tokenUrl: 'http://127.0.0.1:8100/api/oauth/token',
      clientId: 'cid', scope: 'read write', clientAuthIn: 'basic', redirectPort: '8123',
    });
    await setSecret('sekrit');
    return await evaluate(`(() => {
      const v = __verify;
      const exported = v.toPostmanCollection(v.state.collections[0]);
      const text = JSON.stringify(exported);
      const imported = v.fromPostman(exported);
      const o = imported.items[0].auth.oauth2;
      return !text.includes('sekrit') && !text.includes('clientSecret')
        && o.grantType === 'authorization_code' && o.tokenUrl === 'http://127.0.0.1:8100/api/oauth/token'
        && o.authorizeUrl === 'http://127.0.0.1:8100/api/oauth/authorize' && o.clientId === 'cid'
        && o.scope === 'read write' && o.clientAuthIn === 'basic' && o.redirectPort === '8123';
    })()`);
  });
  await check('authorization code completes the loopback exchange with a valid PKCE verifier', async () => {
    await configureOAuth2('authorization_code', {
      authorizeUrl: 'http://127.0.0.1:8100/api/oauth/authorize',
      tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid', clientAuthIn: 'body', redirectPort: '',
    });
    await setSecret('sekrit');
    const before = await oauthStats();
    await clickAndWaitAuthorize();
    await followAuthorize();
    const status = await waitStatus('Token acquired');
    return status.includes('Token acquired') && (await oauthStats()).hits === before.hits + 1;
  });
  await check('an expired authorization-code token refreshes without reopening the provider', async () => {
    await expireCachedToken();
    const before = await oauthStats();
    const ok = await evaluate(`(async () => {
      const { response } = await __verify.executeRequest(__verify.state.tabs[0].req);
      const headers = JSON.parse(response.body ?? '{}').headers ?? {};
      return response.status === 200 && headers.authorization === 'Bearer refreshed-tok';
    })()`);
    const after = await oauthStats();
    return ok && after.refreshHits === before.refreshHits + 1 && after.hits === before.hits + 1;
  });
  await check('a rejected refresh falls back to full authorization and then sends', async () => {
    await configureOAuth2('authorization_code', {
      authorizeUrl: 'http://127.0.0.1:8100/api/oauth/authorize',
      tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'refresh-fails', clientAuthIn: 'body', redirectPort: '',
    });
    await setSecret('sekrit');
    await clickAndWaitAuthorize();
    await followAuthorize();
    const initialStatus = await waitStatus('Token acquired');
    if (!initialStatus.includes('Token acquired')) throw new Error(initialStatus);
    await expireCachedToken();
    const before = await oauthStats();
    const previousUrl = await evaluate('window.__oauth2AuthorizeUrl');
    await evaluate(`(() => {
      window.__oauth2Send = __verify.executeRequest(__verify.state.tabs[0].req);
      return true;
    })()`);
    const reopened = await waitForNewAuthorizeUrl(previousUrl);
    await followAuthorize();
    const ok = await evaluate(`(async () => {
      const { response } = await window.__oauth2Send;
      const headers = JSON.parse(response.body ?? '{}').headers ?? {};
      return response.status === 200 && headers.authorization === 'Bearer runner-token-123';
    })()`);
    const after = await oauthStats();
    return reopened && ok && after.refreshHits === before.refreshHits + 1 && after.hits === before.hits + 2;
  });
  await check('the Rust loopback capture rejects a state mismatch', () => evaluate(`(async () => {
    const invoke = window.__TAURI__.core.invoke;
    const begin = await invoke('oauth2_authorize_begin', { port: 0 });
    const waiting = invoke('oauth2_authorize_await', { captureId: begin.captureId, expectedState: 'the-real-state' });
    await new Promise(resolve => setTimeout(resolve, 50));
    await fetch(begin.redirectUri + '?code=evil&state=rogue').catch(() => {});
    let rejected = false;
    try { await waiting; }
    catch (e) { rejected = String(e).includes('state'); }
    await invoke('oauth2_authorize_cancel', { captureId: begin.captureId });
    return rejected;
  })()`));
  await check('a loopback denial is reported immediately without leaking its description', async () => {
    const begin = await evaluate(`(async () => {
      const invoke = window.__TAURI__.core.invoke;
      const capture = await invoke('oauth2_authorize_begin', { port: 0 });
      window.__denialCapture = invoke('oauth2_authorize_await', { captureId: capture.captureId, expectedState: 'expected-state' })
        .then(value => ({ ok: true, value }), error => ({ ok: false, error: String(error) }));
      return capture;
    })()`);
    const started = Date.now();
    await fetch(`${begin.redirectUri}?error=access_denied&error_description=sekrit&state=expected-state`);
    const result = await evaluate('window.__denialCapture');
    return result.ok === false && result.error.includes('authorization was denied (access_denied)')
      && !result.error.includes('sekrit') && Date.now() - started < 3000;
  });
  await check('manual paste rejects a callback URL with the wrong origin or path', async () => {
    await configureOAuth2('authorization_code', {
      authorizeUrl: 'http://127.0.0.1:8100/api/oauth/challenge',
      tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid', clientAuthIn: 'body', redirectPort: '',
    });
    await setSecret('sekrit');
    const before = await oauthStats();
    await clickAndWaitAuthorize();
    const { url } = await followAuthorize(false);
    const authorize = new URL(url);
    const callback = `${authorize.searchParams.get('redirect_uri')}/other?code=abc123&state=${encodeURIComponent(authorize.searchParams.get('state'))}`;
    await evaluate(`(() => {
      const input = document.querySelector('#req-auth-fields [data-oauth-paste]');
      input.value = ${JSON.stringify(callback)};
      document.querySelector('#req-auth-fields [data-oauth-paste-get]').click();
      return true;
    })()`);
    const status = await waitStatus('does not match the loopback redirect');
    return status.includes('does not match the loopback redirect') && (await oauthStats()).hits === before.hits;
  });
  await check('manual paste rejects a callback URL with the wrong state', async () => {
    await configureOAuth2('authorization_code', {
      authorizeUrl: 'http://127.0.0.1:8100/api/oauth/challenge',
      tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid', clientAuthIn: 'body', redirectPort: '',
    });
    await setSecret('sekrit');
    const before = await oauthStats();
    await clickAndWaitAuthorize();
    const { url } = await followAuthorize(false);
    const redirectUri = new URL(url).searchParams.get('redirect_uri');
    await evaluate(`(() => {
      const input = document.querySelector('#req-auth-fields [data-oauth-paste]');
      input.value = ${JSON.stringify(`${redirectUri}?code=abc123&state=rogue`)};
      document.querySelector('#req-auth-fields [data-oauth-paste-get]').click();
      return true;
    })()`);
    const status = await waitStatus('state mismatch');
    return status.includes('state mismatch') && (await oauthStats()).hits === before.hits;
  });
  await check('manual paste exchanges a valid callback URL and sends with its token', async () => {
    const before = await oauthStats();
    const url = await evaluate('window.__oauth2AuthorizeUrl');
    const authorize = new URL(url);
    const callback = `${authorize.searchParams.get('redirect_uri')}?code=abc123&state=${encodeURIComponent(authorize.searchParams.get('state'))}`;
    await evaluate(`(() => {
      const input = document.querySelector('#req-auth-fields [data-oauth-paste]');
      input.value = ${JSON.stringify(callback)};
      document.querySelector('#req-auth-fields [data-oauth-paste-get]').click();
      return true;
    })()`);
    const status = await waitStatus('Token acquired');
    await new Promise(resolve => setTimeout(resolve, 200));
    const settledStatus = await evaluate(`document.querySelector('#req-auth-fields .oauth2-status')?.textContent ?? ''`);
    const sent = await evaluate(`(async () => {
      const { response } = await __verify.executeRequest(__verify.state.tabs[0].req);
      const headers = JSON.parse(response.body ?? '{}').headers ?? {};
      return response.status === 200 && headers.authorization === 'Bearer runner-token-123';
    })()`);
    return status.includes('Token acquired') && settledStatus.includes('Token acquired') && sent && (await oauthStats()).hits === before.hits + 1;
  });
  await check('concurrent manual exchange and Send use a single authorization-code request', async () => {
    await configureOAuth2('authorization_code', {
      authorizeUrl: 'http://127.0.0.1:8100/api/oauth/challenge',
      tokenUrl: 'http://127.0.0.1:8100/api/oauth/token?slow=1', clientId: 'slow-token', clientAuthIn: 'body', redirectPort: '',
    });
    await setSecret('sekrit');
    await evaluate('__verify.oauth2Cache.clear()');
    await clickAndWaitAuthorize();
    await followAuthorize(false);
    const url = await evaluate('window.__oauth2AuthorizeUrl');
    const authorize = new URL(url);
    const callback = `${authorize.searchParams.get('redirect_uri')}?code=abc123&state=${encodeURIComponent(authorize.searchParams.get('state'))}`;
    const before = await oauthStats();
    const ok = await evaluate(`(async () => {
      const v = __verify;
      const req = v.state.tabs[0].req;
      const flow = v.oauth2Flows.get(req.id);
      const manual = v.exchangeOAuth2Paste(req.id, req.auth, null, ${JSON.stringify(callback)});
      const sent = v.executeRequest(req);
      const [entry, outcome, flowEntry] = await Promise.all([manual, sent, flow]);
      const headers = JSON.parse(outcome.response.body ?? '{}').headers ?? {};
      return entry.token === 'runner-token-123' && flowEntry.token === entry.token
        && outcome.response.status === 200 && headers.authorization === 'Bearer runner-token-123';
    })()`);
    return ok && (await oauthStats()).hits === before.hits + 1;
  });
  await check('zero-second token expiry is never reused', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'zero-expiry' });
    await setSecret('sekrit');
    await clickGetToken();
    await waitStatus('Token acquired');
    const before = await oauthStats();
    const ok = await evaluate(`(async () => {
      const v = __verify;
      const entry = [...v.oauth2Cache.entries()].find(([, entry]) => entry.token === 'zero-token')?.[1];
      const { response } = await v.executeRequest(v.state.tabs[0].req);
      const headers = JSON.parse(response.body ?? '{}').headers ?? {};
      return entry?.token === 'zero-token' && entry.expiresAt <= Date.now()
        && response.status === 200 && headers.authorization === 'Bearer zero-token';
    })()`);
    return ok && (await oauthStats()).hits === before.hits + 1;
  });
  await check('an in-flight token is cached under its original configuration snapshot', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'slow-token' });
    await setSecret('sekrit');
    const keys = await evaluate(`(() => {
      const v = __verify;
      v.oauth2Cache.clear();
      const id = v.state.tabs[0].req.id;
      const auth = v.state.tabs[0].req.auth;
      const original = v.oauth2CacheKey(id, auth);
      window.__slowOAuth = v.acquireOAuth2Token(id, auth).then(entry => entry.token);
      auth.oauth2.clientId = 'cid';
      auth.oauth2.scope = 'mutated-while-fetching';
      return { original, mutated: v.oauth2CacheKey(id, auth) };
    })()`);
    return await evaluate(`(async () => {
      const v = __verify;
      const token = await window.__slowOAuth;
      const keys = ${JSON.stringify(keys)};
      return token === 'runner-token-123' && v.oauth2Cache.get(keys.original)?.token === token
        && !v.oauth2Cache.has(keys.mutated);
    })()`);
  });
  await check('a token response arriving after cancellation is never cached', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'slow-token' });
    await setSecret('sekrit');
    return await evaluate(`(async () => {
      const v = __verify;
      v.oauth2Cache.clear();
      const id = v.state.tabs[0].req.id;
      const auth = v.state.tabs[0].req.auth;
      const key = v.oauth2CacheKey(id, auth);
      const pending = v.acquireOAuth2Token(id, auth).then(
        () => null,
        error => error,
      );
      await new Promise(resolve => setTimeout(resolve, 80));
      v.bumpOAuth2Epoch(id);
      const error = await pending;
      return error?.oauth2Cancelled === true && !v.oauth2Cache.has(key);
    })()`);
  });
  await check('rerender restores authorization state and Cancel stops the loopback', async () => {
    await configureOAuth2('authorization_code', {
      authorizeUrl: 'http://127.0.0.1:8100/api/oauth/challenge',
      tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid', clientAuthIn: 'body', redirectPort: '',
    });
    await setSecret('sekrit');
    await clickAndWaitAuthorize();
    const restored = await evaluate(`(() => {
      __verify.render();
      const host = document.querySelector('#req-auth-fields');
      return host.querySelector('.oauth2-status')?.textContent.includes('Waiting for the redirect')
        && host.querySelector('[data-oauth-get]')?.disabled === true
        && host.querySelector('[data-oauth-cancel]')?.disabled === false;
    })()`);
    await evaluate(`document.querySelector('#req-auth-fields [data-oauth-cancel]').click()`);
    const status = await waitStatus('Authorization cancelled');
    return restored && status.includes('Authorization cancelled') && await evaluate(`(() => {
      const id = __verify.state.tabs[0].req.id;
      return !__verify.oauth2Pending.has(id) && !__verify.oauth2Flows.has(id);
    })()`);
  });
  await check('changing authorization configuration cancels the active flow', async () => {
    await configureOAuth2('authorization_code', {
      authorizeUrl: 'http://127.0.0.1:8100/api/oauth/challenge',
      tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid', clientAuthIn: 'body', redirectPort: '',
    });
    await setSecret('sekrit');
    await clickAndWaitAuthorize();
    await evaluate(`(() => {
      const input = document.querySelector('#req-auth-fields [data-k="authorizeUrl"]');
      input.value = 'http://127.0.0.1:8100/api/oauth/authorize';
      input.dispatchEvent(new Event('input', { bubbles: true }));
      return true;
    })()`);
    const status = await waitStatus('Authorization cancelled');
    return status.includes('Authorization cancelled') && await evaluate(`(() => {
      const id = __verify.state.tabs[0].req.id;
      return !__verify.oauth2Pending.has(id) && !__verify.oauth2Flows.has(id);
    })()`);
  });
  await check('auth controls and async secret loads stay bound to their original owner', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid' });
    await setSecret('sekrit');
    const firstId = await evaluate('__verify.state.tabs[0].req.id');
    await evaluate(`(() => {
      __verify.addTab(__verify.state.collections[0].items[1]);
      return true;
    })()`);
    const secondId = await evaluate('__verify.state.tabs.find(tab => tab.id === __verify.state.activeTabId).req.id');
    await evaluate(`(() => {
      const host = document.querySelector('#req-auth-fields');
      host.dataset.oauthOwner = ${JSON.stringify(firstId)};
      host.innerHTML = '<input data-oauth-secret value="sekrit"><button data-oauth-get>Get token</button><span class="oauth2-status"></span>';
      host.querySelector('[data-oauth-secret]').dispatchEvent(new Event('input', { bubbles: true }));
      window.__ownerProbe = { owner: host.dataset.oauthOwner, active: __verify.state.tabs[0].req.id };
      host.querySelector('[data-oauth-get]').click();
      return true;
    })()`);
    const acquired = await evaluate(`(async () => {
      const start = Date.now();
      while (Date.now() - start < 5000) {
        if (__verify.oauth2UiState.get(${JSON.stringify(firstId)})?.phase === 'acquired') return true;
        await new Promise(resolve => setTimeout(resolve, 40));
      }
      return false;
    })()`);
    const observed = await evaluate(`(async () => {
      const v = __verify;
      const invoke = window.__TAURI__.core.invoke;
      await v.flushOAuth2Secret(${JSON.stringify(firstId)});
      const firstSecret = await invoke('oauth2_secret_get', { nodeId: ${JSON.stringify(firstId)} });
      const secondSecret = await invoke('oauth2_secret_get', { nodeId: ${JSON.stringify(secondId)} });
      const firstCached = [...v.oauth2Cache.keys()].some(key => JSON.parse(key)[0] === ${JSON.stringify(firstId)});
      const secondCached = [...v.oauth2Cache.keys()].some(key => JSON.parse(key)[0] === ${JSON.stringify(secondId)});
      const host = document.querySelector('#req-auth-fields');
      host.dataset.oauthOwner = ${JSON.stringify(secondId)};
      host.querySelector('[data-oauth-secret]').value = '';
      await v.loadOAuth2SecretInto(host, ${JSON.stringify(firstId)});
      return {
        firstId: ${JSON.stringify(firstId)}, secondId: ${JSON.stringify(secondId)},
        probe: window.__ownerProbe, firstSecret, secondSecret, firstCached, secondCached,
        field: host.querySelector('[data-oauth-secret]').value,
        owner: host.dataset.oauthOwner,
      };
    })()`);
    const passed = acquired && observed.firstSecret === 'sekrit' && observed.secondSecret === null
      && observed.firstCached && !observed.secondCached && observed.field === '';
    return passed ? true : { acquired, ...observed };
  });
  await check('an idle first connection cannot block a later valid loopback callback', async () => {
    const begin = await evaluate(`(async () => {
      const invoke = window.__TAURI__.core.invoke;
      const capture = await invoke('oauth2_authorize_begin', { port: 0 });
      window.__idleCapture = invoke('oauth2_authorize_await', { captureId: capture.captureId, expectedState: 'idle-state' })
        .then(value => ({ ok: true, value }), error => ({ ok: false, error: String(error) }));
      return capture;
    })()`);
    const port = new URL(begin.redirectUri).port;
    const idle = connect(Number(port), '127.0.0.1');
    await new Promise((resolve, reject) => {
      idle.once('connect', resolve);
      idle.once('error', reject);
    });
    idle.write('GET /callback?code=partial&state=idle-state HTTP/1.1\r\n');
    await new Promise(resolve => setTimeout(resolve, 50));
    await fetch(`${begin.redirectUri}?code=valid&state=idle-state`);
    const result = await evaluate('window.__idleCapture');
    idle.destroy();
    return result.ok && result.value[0] === 'valid' && result.value[1] === 'idle-state';
  });
  await check('concurrent loopback captures complete independently by capture id', async () => {
    const slots = await evaluate(`(async () => {
      const invoke = window.__TAURI__.core.invoke;
      const a = await invoke('oauth2_authorize_begin', { port: 0 });
      const b = await invoke('oauth2_authorize_begin', { port: 0 });
      window.__captureA = invoke('oauth2_authorize_await', { captureId: a.captureId, expectedState: 'state-a' });
      window.__captureB = invoke('oauth2_authorize_await', { captureId: b.captureId, expectedState: 'state-b' });
      return { a, b };
    })()`);
    await new Promise(resolve => setTimeout(resolve, 80));
    await fetch(`${slots.b.redirectUri}?code=code-b&state=state-b`);
    const second = await evaluate('window.__captureB');
    await fetch(`${slots.a.redirectUri}?code=code-a&state=state-a`);
    const first = await evaluate('window.__captureA');
    return slots.a.captureId !== slots.b.captureId
      && second[0] === 'code-b' && second[1] === 'state-b'
      && first[0] === 'code-a' && first[1] === 'state-a';
  });
  await check('cancelling one capture leaves another capture usable', async () => {
    const slots = await evaluate(`(async () => {
      const invoke = window.__TAURI__.core.invoke;
      const a = await invoke('oauth2_authorize_begin', { port: 0 });
      const b = await invoke('oauth2_authorize_begin', { port: 0 });
      window.__captureC = invoke('oauth2_authorize_await', { captureId: a.captureId, expectedState: 'state-c' })
        .then(value => ({ ok: true, value }), error => ({ ok: false, error: String(error) }));
      window.__captureD = invoke('oauth2_authorize_await', { captureId: b.captureId, expectedState: 'state-d' })
        .then(value => ({ ok: true, value }), error => ({ ok: false, error: String(error) }));
      return { a, b };
    })()`);
    await new Promise(resolve => setTimeout(resolve, 80));
    await evaluate(`window.__TAURI__.core.invoke('oauth2_authorize_cancel', { captureId: ${JSON.stringify(slots.a.captureId)} })`);
    await fetch(`${slots.b.redirectUri}?code=code-d&state=state-d`);
    const results = await evaluate('Promise.all([window.__captureC, window.__captureD])');
    return results[0].ok === false && results[0].error.toLowerCase().includes('cancel')
      && results[1].ok === true && results[1].value[0] === 'code-d';
  });
  await check('deleting a request clears its cached token and encrypted secret', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid' });
    await setSecret('sekrit');
    await clickGetToken();
    await waitStatus('Token acquired');
    const id = await evaluate(`__verify.state.tabs[0].req.id`);
    await evaluate(`(() => {
      const v = __verify;
      v.state.settings.general.confirmBeforeDelete = false;
      v.render();
      document.querySelector('.node[data-kind="request"][data-id="${id}"] [data-act="delete"]').click();
      return true;
    })()`);
    return await evaluate(`(async () => {
      const v = __verify;
      const id = ${JSON.stringify(id)};
      const start = Date.now();
      while (Date.now() - start < 4000) {
        const exists = v.state.collections[0].items.some(item => item.id === id);
        const secret = await window.__TAURI__.core.invoke('oauth2_secret_get', { nodeId: id });
        const cached = [...v.oauth2Cache.keys()].some(key => JSON.parse(key)[0] === id);
        if (!exists && secret === null && !v.oauth2Secrets.has(id) && !cached && !v.oauth2UiState.has(id)) return true;
        await new Promise(resolve => setTimeout(resolve, 40));
      }
      return false;
    })()`);
  });
  await check('merging a backup cannot reuse an existing node secret or token', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid' });
    await setSecret('sekrit');
    await clickGetToken();
    await waitStatus('Token acquired');
    const oldId = await evaluate('__verify.state.tabs[0].req.id');
    return await evaluate(`(async () => {
      const v = __verify;
      const invoke = window.__TAURI__.core.invoke;
      const current = v.state.workspaces.find(ws => ws.id === v.state.activeWorkspaceId) ?? v.state.workspaces[0];
      const currentId = current.id;
      current.collections = v.state.collections;
      const imported = v.newWorkspace({ id: currentId, name: 'Imported workspace' });
      const collection = v.newCollection('Imported collection');
      collection.items.push(v.newRequest({
        id: ${JSON.stringify(oldId)}, name: 'Imported collision', url: 'http://127.0.0.1:8100/api/echo',
        auth: { type: 'oauth2', oauth2: {
          grantType: 'client_credentials', tokenUrl: 'http://127.0.0.1:9999/stolen', clientId: 'cid',
        } },
      }));
      imported.collections.push(collection);
      const [merged] = v.applySavedPayload({ workspaces: [imported], activeWorkspaceId: imported.id }, 'merge');
      const mergedId = merged.collections[0].items[0].id;
      const oldSecret = await invoke('oauth2_secret_get', { nodeId: ${JSON.stringify(oldId)} });
      const importedSecret = await invoke('oauth2_secret_get', { nodeId: mergedId });
      const oldCached = [...v.oauth2Cache.keys()].some(key => JSON.parse(key)[0] === ${JSON.stringify(oldId)});
      const importedCached = [...v.oauth2Cache.keys()].some(key => JSON.parse(key)[0] === mergedId);
      return merged.id !== currentId && mergedId !== ${JSON.stringify(oldId)} && oldSecret === 'sekrit' && importedSecret === null
        && oldCached && !importedCached && current.collections[0].items[0].id === ${JSON.stringify(oldId)};
    })()`);
  });
  await check('replacing a backup clears secrets and tokens owned by discarded nodes', async () => {
    await configureOAuth2('client_credentials', { tokenUrl: 'http://127.0.0.1:8100/api/oauth/token', clientId: 'cid' });
    await setSecret('sekrit');
    await clickGetToken();
    await waitStatus('Token acquired');
    const oldId = await evaluate('__verify.state.tabs[0].req.id');
    await evaluate(`(() => {
      const v = __verify;
      const workspace = v.newWorkspace({ name: 'Restored workspace' });
      const collection = v.newCollection('Restored collection');
      collection.items.push(v.newRequest({ name: 'Restored request', url: 'http://127.0.0.1:8100/api/echo' }));
      workspace.collections.push(collection);
      const payload = { ...v.workspacePayload(), workspaces: [workspace], activeWorkspaceId: workspace.id };
      v.openRestoreDialog(JSON.stringify({ kind: 'postlite.backup', version: 1, exportedAt: new Date().toISOString(), payload }));
      document.querySelector('#restore-dialog input[value="replace"]').checked = true;
      window.__backupRestore = v.confirmRestore();
      return true;
    })()`);
    await new Promise(resolve => setTimeout(resolve, 50));
    await evaluate(`document.querySelector('#confirm-ok').click()`);
    await evaluate('window.__backupRestore');
    return await evaluate(`(async () => {
      const v = __verify;
      const invoke = window.__TAURI__.core.invoke;
      const secret = await invoke('oauth2_secret_get', { nodeId: ${JSON.stringify(oldId)} });
      const cached = [...v.oauth2Cache.keys()].some(key => JSON.parse(key)[0] === ${JSON.stringify(oldId)});
      const exists = v.state.workspaces.some(ws => ws.collections.some(collection => collection.items.some(item => item.id === ${JSON.stringify(oldId)})));
      return secret === null && !cached && !v.oauth2Secrets.has(${JSON.stringify(oldId)}) && !exists;
    })()`);
  });
  await check('the OAuth2 flow leaves no runtime errors', () => evaluate('window.__runtimeErrors.length === 0'));

  console.log(`\n${results.length} checks passed`);
} catch (err) {
  console.error(`FAIL ${err.message}`);
  process.exitCode = 1;
} finally {
  await close();
}
