import assert from 'node:assert/strict';
import { evaluate, close, reload } from './tier1-cdp.mjs';

const results = [];
let executed = 0;
// Optional third arg: run only the first N checks (task-staged verification).
const maxChecks = Number(process.argv[3]) || Infinity;
const check = async (name, expression) => {
  if (executed >= maxChecks) return;
  // A function runs harness-side (multi-step flows with evaluates); a string
  // is evaluated in the page in one shot.
  const value = typeof expression === "function" ? await expression() : await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  executed++;
  results.push(name);
  console.log(`PASS ${name}`);
};
const settle = () => evaluate('__verify.flushWorkspace().then(()=>true)');
// Seed one collection with a request against the echo fixture plus variables,
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

  // --- Task 2: {{ autocomplete -------------------------------------------------
  const typeInUrl = (value) => evaluate(`(() => {
    const url = document.querySelector('#url');
    url.value = ${JSON.stringify(value)};
    url.focus();
    url.setSelectionRange(url.value.length, url.value.length);
    url.dispatchEvent(new Event('input', { bubbles: true }));
    return true;
  })()`);
  await check('typing {{ opens the dropdown listing scope variables', `(async () => {
    document.querySelector('#var-ac').hidden = true;
    const url = document.querySelector('#url');
    url.value = 'http://{{';
    url.focus();
    url.setSelectionRange(url.value.length, url.value.length);
    url.dispatchEvent(new Event('input', { bubbles: true }));
    const ac = document.querySelector('#var-ac');
    return !ac.hidden && ac.textContent.includes('base') && ac.textContent.includes('secretKey');
  })()`);
  await check('query text after {{ filters the list', `(async () => {
    const url = document.querySelector('#url');
    url.value = 'http://{{ba';
    url.dispatchEvent(new Event('input', { bubbles: true }));
    const ac = document.querySelector('#var-ac');
    return !ac.hidden && ac.textContent.includes('base') && !ac.textContent.includes('secretKey');
  })()`);
  await check('secret variables are masked in the list', `(async () => {
    const url = document.querySelector('#url');
    url.value = 'http://{{';
    url.dispatchEvent(new Event('input', { bubbles: true }));
    const ac = document.querySelector('#var-ac');
    const row = ac.querySelector('[data-name="secretKey"]');
    return row && row.classList.contains('is-secret') && !ac.textContent.includes('s3cr3t-value');
  })()`);
  await check('Enter inserts the highlighted variable', `(async () => {
    const url = document.querySelector('#url');
    url.value = 'http://{{ba';
    url.dispatchEvent(new Event('input', { bubbles: true }));
    url.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
    return url.value === 'http://{{base}}' && document.querySelector('#var-ac').hidden;
  })()`);
  await check('dynamic tokens form a trailing group and insert on select', `(async () => {
    const url = document.querySelector('#url');
    url.value = 'http://{{';
    url.dispatchEvent(new Event('input', { bubbles: true }));
    const ac = document.querySelector('#var-ac');
    const dynamicRow = ac.querySelector('[data-name="$timestamp"]');
    if (!dynamicRow || !dynamicRow.closest('.var-ac-group')?.classList.contains('var-ac-dynamic')) return false;
    // Selection happens on mousedown (keeps editor focus); .click() alone would never fire it.
    dynamicRow.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
    return url.value === 'http://{{$timestamp}}';
  })()`);
  await check('Escape closes the dropdown', `(async () => {
    const url = document.querySelector('#url');
    url.value = 'http://{{';
    url.dispatchEvent(new Event('input', { bubbles: true }));
    url.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    return document.querySelector('#var-ac').hidden;
  })()`);

  // --- Task 3: duplicate request / folder / collection --------------------------
  // Seed a folder with a child request and an example on the top-level request.
  const seedDupes = () => evaluate(`(() => {
    const v = __verify;
    const c = v.state.collections[0];
    c.items.push(v.newFolder({ name: 'Folder A', items: [
      v.newRequest({ name: 'Inner', url: 'http://127.0.0.1:8100/api/echo' }),
    ]}));
    c.items[0].examples = v.normExamples([{ name: 'ex1', body: 'x' }]);
    v.persist(); v.render();
    return true;
  })()`);
  await seedDupes();
  const dupMenuFlow = (kind) => `
    const row = document.querySelector('#collections .node[data-kind="${kind}"]');
    if (!row) return false;
    row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 20, clientY: 20 }));
    // Scope to the menu that actually opened — hidden menus keep their
    // listeners attached, so an unscoped querySelector would click the wrong one.
    const item = document.querySelector('.ctx-menu:not([hidden]) [data-menu="duplicate"]');
    if (!item) return false;
    item.click();
    return true;
  `;
  await check('duplicate request via context menu inserts a fresh-id copy', `(async () => {
    const v = __verify;
    const c = v.state.collections[0];
    const tabsBefore = v.state.tabs.length;
    const source = c.items[0];
    if (!await (async () => { ${dupMenuFlow('request')} })()) return false;
    const dup = c.items[1];
    return c.items.length === 3 && c.items[2].kind === 'folder'
      && dup.id !== source.id && dup.name === 'Hello copy' && dup.kind === 'request'
      && v.state.tabs.length === tabsBefore;
  })()`);
  await check('duplicate folder gives every descendant a fresh id', `(async () => {
    const v = __verify;
    const c = v.state.collections[0];
    const source = c.items.find(n => n.kind === 'folder');
    if (!await (async () => { ${dupMenuFlow('folder')} })()) return false;
    const dup = c.items.find(n => n.name === 'Folder A copy');
    const ids = (n) => [n.id, ...(n.items ?? []).flatMap(ids)];
    const sourceIds = ids(source);
    const dupIds = ids(dup);
    return dupIds.every(id => !sourceIds.includes(id)) && c.items.some(n => n.id === dup.id);
  })()`);
  await check('duplicate collection lands right after the source', `(async () => {
    const v = __verify;
    if (!await (async () => { ${dupMenuFlow('collection')} })()) return false;
    const copy = v.state.collections[1];
    return v.state.collections.length === 2
      && copy.id !== v.state.collections[0].id
      && copy.name === 'Quick wins suite copy';
  })()`);
  await check('duplicate favorite collection stays directly after its source', `(() => {
    const v = __verify;
    const original = v.state.collections;
    const source = v.newCollection('Fav source', { favorite: true });
    const other = v.newCollection('Fav other', { favorite: true });
    const normal = v.newCollection('Normal', { favorite: false });
    v.state.collections = [other, source, normal];
    v.render();
    document.querySelector('#collections .node[data-id="' + source.id + '"]')
      .dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 20, clientY: 20 }));
    document.querySelector('.ctx-menu:not([hidden]) [data-menu="duplicate"]').click();
    const rendered = [...document.querySelectorAll('#collections .node[data-kind="collection"]')]
      .map(row => row.querySelector('.node-name').textContent);
    const copy = v.state.collections[2];
    const ok = copy.name === 'Fav source copy' && copy.favorite === true
      && v.state.collections.map(c => c.name).join(',') === 'Fav other,Fav source,Fav source copy,Normal'
      && rendered.join(',') === 'Fav other,Fav source,Fav source copy,Normal';
    v.state.collections = original;
    v.persist(); v.render();
    return ok;
  })()`);
  await check('duplicated request example ids are fresh', `(() => {
    const v = __verify;
    const c = v.state.collections[0];
    const dup = c.items[1];
    const source = c.items[0];
    return dup.examples?.[0] && dup.examples[0].id !== source.examples[0].id;
  })()`);
  await check('duplication never touches open tabs', `(() => {
    const v = __verify;
    return v.state.tabs.length === 1 && v.state.tabs[0].req.name === 'Hello';
  })()`);

  // --- Task 4: search in the response body -------------------------------------
  // Seed a real response against /api/large-json (120 "row-" names).
  const sendLarge = () => evaluate(`(async () => {
    const v = __verify;
    v.state.collections[0].items[0].url = 'http://127.0.0.1:8100/api/large-json';
    await v.send();
    return true;
  })()`);
  const openFindOn = (tab) => evaluate(`(() => {
    document.querySelector('#res-tabs [data-tab="${tab}"]').click();
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true }));
    return !document.querySelector('#res-find').hidden;
  })()`);
  const findInput = (value) => evaluate(`(() => {
    const input = document.querySelector('#res-find-input');
    input.value = ${JSON.stringify(value)};
    input.dispatchEvent(new Event('input', { bubbles: true }));
    return true;
  })()`);
  await sendLarge();
  await check('Ctrl+F on raw view opens the find bar and marks matches', async () => {
    if (!await openFindOn('raw')) return false;
    return await evaluate(`(() => {
      const input = document.querySelector('#res-find-input');
      input.value = 'row-1';
      input.dispatchEvent(new Event('input', { bubbles: true }));
      const marks = document.querySelectorAll('#raw mark.res-find-mark');
      return marks.length > 0 && document.querySelector('#res-find-count').textContent === '1 of ' + marks.length;
    })()`);
  });
  await check('next and prev cycle the current match', async () => {
    const total = await evaluate(`document.querySelectorAll('#raw mark.res-find-mark').length`);
    await evaluate(`document.querySelector('#res-find-next').click()`);
    const second = await evaluate(`document.querySelector('#res-find-count').textContent`);
    await evaluate(`document.querySelector('#res-find-prev').click()`);
    const back = await evaluate(`document.querySelector('#res-find-count').textContent`);
    return second === '2 of ' + total && back === '1 of ' + total;
  });
  await check('pretty view search expands folds and finds matches', async () => {
    if (!await openFindOn('pretty')) return false;
    await findInput('row-42');
    return await evaluate(`(() => {
      const marks = document.querySelectorAll('#pretty mark.res-find-mark');
      const current = document.querySelector('#pretty mark.res-find-mark.is-current');
      return marks.length === 1 && Boolean(current);
    })()`);
  });
  await check('Escape closes the find bar and unwraps marks', async () => {
    await evaluate(`document.querySelector('#res-find-input')
      .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }))`);
    return await evaluate(`document.querySelector('#res-find').hidden
      && document.querySelectorAll('#pretty mark.res-find-mark').length === 0`);
  });
  await check('a new send clears the find bar', async () => {
    if (!await openFindOn('raw')) return false;
    await findInput('row-1');
    await sendLarge();
    return await evaluate(`document.querySelector('#res-find').hidden
      && document.querySelectorAll('#raw mark.res-find-mark').length === 0`);
  });
  await check('settings dialog owns Ctrl+F when open', async () => {
    await evaluate(`__verify.openAppSettings()`);
    // The dialog handler listens on the dialog element itself, so dispatch there.
    await evaluate(`document.querySelector('#app-settings-dialog')
      .dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true }))`);
    const ok = await evaluate(`document.activeElement?.id === 'pref-search' && document.querySelector('#res-find').hidden`);
    await evaluate(`document.querySelector('#app-settings-dialog').close()`);
    return ok;
  });

  // --- Task 5: save response to file -------------------------------------------
  // The fetch-IPC seam captures save_export_file calls without native dialogs
  // (same pattern tier1-verify uses for Tauri's non-writable invoke).
  await evaluate(`(() => {
    if (window.__qwSeam) return true;
    const originalFetch = window.fetch.bind(window);
    window.fetch = (input, init) => {
      const url = typeof input === 'string' ? input : input.url;
      if (url.endsWith('/save_export_file')) {
        const args = JSON.parse(init.body);
        window.__qwSaves = window.__qwSaves || [];
        window.__qwSaves.push(args);
        return Promise.resolve(new Response(JSON.stringify('C:/exports/' + args.defaultName), { status: 200, headers: { 'content-type': 'application/json', 'Tauri-Response': 'ok' } }));
      }
      return originalFetch(input, init);
    };
    window.__qwSeam = true;
    return true;
  })()`);
  const sendTo = (url) => evaluate(`(async () => {
    __verify.state.collections[0].items[0].url = ${JSON.stringify('http://127.0.0.1:8100')} + ${JSON.stringify(url)};
    await __verify.send();
    return true;
  })()`);
  const clickSave = () => evaluate(`(() => { window.__qwSaves = []; document.querySelector('#save-res').click(); return true })()`);
  await check('text response saves its exact body with a derived name', async () => {
    await sendTo('/api/echo?n=1');
    await clickSave();
    return await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 60));
      const args = window.__qwSaves.at(-1);
      const body = __verify.state.tabs[0].response.body;
      return Boolean(args) && args.contents === body && args.contentsB64 === null && args.defaultName === 'echo.json';
    })()`);
  });
  await check('binary response saves byte-faithful via base64', async () => {
    await sendTo('/api/bin');
    await clickSave();
    return await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 60));
      const args = window.__qwSaves.at(-1);
      if (!args || !args.contentsB64) return false;
      const bytes = Uint8Array.from(atob(args.contentsB64), c => c.charCodeAt(0));
      const expected = [80, 75, 3, 4, 255, 0, 254, 128, 1];
      // Binary saves carry the body once: contents stays empty, base64 holds the bytes.
      return args.defaultName === 'bin.bin' && args.contents === ''
        && bytes.length === 9 && expected.every((b, i) => bytes[i] === b);
    })()`);
  });
  await check('truncated saves carry a truncation warning', async () => {
    await evaluate(`(() => { __verify.state.settings.response.maxResponseSizeKB = 1; __verify.applyAppSettings(); return true })()`);
    await sendTo('/api/large');
    const truncated = await evaluate(`__verify.state.tabs[0].response.truncated === true`);
    await clickSave();
    const note = await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 60));
      return document.querySelector('#tool-note').textContent;
    })()`);
    await evaluate(`(() => { __verify.state.settings.response.maxResponseSizeKB = 2000; __verify.applyAppSettings(); return true })()`);
    return truncated && note.includes('truncated');
  });
  await check('unknown content types fall back to .txt names', async () => {
    await sendTo('/api/large');
    await clickSave();
    return await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 60));
      return window.__qwSaves.at(-1).defaultName === 'large.txt';
    })()`);
  });
  await check('switching response views closes the find bar', async () => {
    if (!await openFindOn('raw')) return false;
    await findInput('row-1');
    await evaluate(`document.querySelector('#res-tabs [data-tab="pretty"]').click()`);
    return await evaluate(`document.querySelector('#res-find').hidden
      && document.querySelectorAll('#pretty mark.res-find-mark').length === 0`);
  });

  console.log(`\n${results.length} checks passed`);
} catch (err) {
  console.error(`FAIL ${err.message}`);
  process.exitCode = 1;
} finally {
  await close();
}
