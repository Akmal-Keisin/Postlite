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
const settle = () => evaluate('__verify.flushWorkspace().then(()=>true)');
// Seed a collection with three requests against the echo fixture.
const seed = () => evaluate(`(() => {
  const v = __verify;
  v.state.collections = []; v.state.environments = []; v.state.tabs = []; v.state.activeTabId = null;
  v.state.history = []; v.state.settings = v.normalizeSettings(); v.applyAppSettings();
  const r = (name, url) => v.newRequest({ name, url, method: 'GET' });
  const c = v.newCollection('Automation suite', { items: [
    r('First', 'http://127.0.0.1:8100/api/echo?n=1'),
    r('Second', 'http://127.0.0.1:8100/api/echo?n=2'),
    r('Third', 'http://127.0.0.1:8100/api/echo?n=3'),
  ]});
  v.state.collections.push(c);
  v.persist(); v.render();
  return true;
})()`);
// Open the runner over the whole collection and attach a data file + delay.
const openRunner = (dataRows, delayMs) => evaluate(`(async () => {
  const v = __verify;
  v.openRunnerDialog(v.state.collections[0].id);
  if (${JSON.stringify(dataRows)}) {
    const file = new File([JSON.stringify(${JSON.stringify(dataRows)})], "data.json", { type: "application/json" });
    const dt = new DataTransfer();
    dt.items.add(file);
    const input = document.querySelector("#runner-data-file");
    input.files = dt.files;
    input.dispatchEvent(new Event("change", { bubbles: true }));
    // The change handler parses asynchronously; let it land before running.
    await new Promise(r => setTimeout(r, 150));
  }
  document.querySelector("#runner-delay").value = ${JSON.stringify(delayMs ?? 0)};
  document.querySelector("#runner-delay").dispatchEvent(new Event("input", { bubbles: true }));
  return true;
})()`);
const runAndFinish = async () => {
  await evaluate(`document.querySelector('#runner-run').click()`);
  return await evaluate(`new Promise(resolve => {
    const start = Date.now();
    const timer = setInterval(() => {
      if (!__verify.runnerState.active && __verify.runnerState.phase === 'done') { clearInterval(timer); resolve({ ms: Date.now() - start }); }
      else if (Date.now() - start > 30000) { clearInterval(timer); resolve({ ms: Date.now() - start, timeout: true }); }
    }, 30);
  })`);
};

try {
  await reload();
  await evaluate(`(() => {
    window.__runtimeErrors = []; window.addEventListener('error', e => __runtimeErrors.push(e.message));
  })()`);
  await seed();

  // --- Task 1: data scope + data-driven iterations -----------------------------
  await check('parseDataFile handles JSON, CSV quotes, CRLF and BOM; rejects empties', `(() => {
    const v = __verify;
    const json = v.parseDataFile('[{"a":"1","b":2}]');
    const csv = v.parseDataFile('\\ufeffa,b\\r\\n"x,1","y""z"\\r\\n2,3\\r\\n');
    let emptyErr = "";
    try { v.parseDataFile('[]'); } catch (e) { emptyErr = e.message; }
    return json.length === 1 && json[0].a === "1" && json[0].b === "2"
      && csv.length === 2 && csv[0].a === "x,1" && csv[0].b === 'y"z' && csv[1].a === "2"
      && emptyErr.length > 0;
  })()`);
  await check('data rows fill the lowest-precedence scope on the wire', `(async () => {
    const v = __verify;
    const req = v.state.collections[0].items[0];
    req.url = 'http://127.0.0.1:8100/api/echo?token={{token}}';
    const { response } = await v.executeRequest(req, { token: 't1' });
    return response.status === 200 && response.body.includes('token=t1');
  })()`);
  await check('collection variables win over data rows', `(async () => {
    const v = __verify;
    const c = v.state.collections[0];
    c.variables = [{ key: 'token', value: 'cv', enabled: true }];
    const req = c.items[0];
    req.url = 'http://127.0.0.1:8100/api/echo?token={{token}}';
    const { response } = await v.executeRequest(req, { token: 't1' });
    return response.body.includes('token=cv');
  })()`);
  await check('pm.data.get exposes the row inside test scripts', `(async () => {
    const v = __verify;
    const c = v.state.collections[0];
    const req = c.items[0];
    req.scripts = { pre: '', test: 'pm.test("data", () => pm.expect(pm.data.get("token")).to.equal("t1")); pm.test("absent", () => pm.expect(pm.data.get("missing")).to.equal(undefined));' };
    const { results } = await v.executeRequest(req, { token: 't1' });
    return results.every(r => r.passed) && results.some(r => r.name === 'data');
  })()`);
  await check('data-driven runs flatten iterations in major order with grouping', async () => {
    await openRunner([{ n: "1" }, { n: "2" }], 0);
    await runAndFinish();
    return await evaluate(`(() => {
      const s = __verify.runnerState;
      if (s.entries.length !== 6) return false;
      const order = s.entries.map(e => e.iteration + ':' + e.req.name).join(',');
      const iters = [...document.querySelectorAll('#runner-results .runner-iter')].map(d => d.textContent);
      return order === '1:First,1:Second,1:Third,2:First,2:Second,2:Third'
        && iters.join(',') === 'Iteration 1,Iteration 2' && s.endReason === 'done';
    })()`);
  });
  await check('failures continue with data; the no-data contract still stops', async () => {
    await evaluate(`(() => {
      const c = __verify.state.collections[0];
      // Earlier checks left a data-asserting script on First — clear everything
      // so only Second's boom failure drives the stop semantics.
      for (const item of c.items) item.scripts = { pre: '', test: '' };
      c.items[1].scripts = { pre: '', test: 'pm.test("boom", () => pm.expect(1).to.equal(2));' };
      return true;
    })()`);
    await openRunner([{ n: "1" }, { n: "2" }], 0);
    await runAndFinish();
    const withData = await evaluate(`(() => {
      const s = __verify.runnerState;
      return s.entries.length === 6 && s.entries.every(e => e.status !== 'skipped');
    })()`);
    await openRunner(null, 0);
    await runAndFinish();
    const withoutData = await evaluate(`(() => {
      const s = __verify.runnerState;
      return s.entries.map(e => e.status).join(',') === 'passed,failed,skipped' && s.endReason === 'failed';
    })()`);
    await evaluate(`(() => {
      __verify.state.collections[0].items[1].scripts = { pre: '', test: '' };
      return true;
    })()`);
    return withData && withoutData;
  });

  // --- Task 2: delay + coalesced history persistence ----------------------------
  await check('a delay stretches the run wall time', async () => {
    await openRunner(null, 300);
    const { ms } = await runAndFinish();
    return ms >= 600; // 3 entries with 300 ms gaps between them
  });
  await check('data controls stay locked during a run and unlock afterward', async () => {
    await openRunner([{ n: "1" }, { n: "2" }], 200);
    return await evaluate(`(async () => {
      document.querySelector('#runner-run').click();
      const waitFor = async (predicate, timeout = 10000) => {
        const start = Date.now();
        while (Date.now() - start < timeout) {
          if (predicate()) return true;
          await new Promise(r => setTimeout(r, 20));
        }
        return false;
      };
      const started = await waitFor(() => __verify.runnerState.active);
      const locked = document.querySelector('#runner-data-clear').disabled
        && document.querySelector('#runner-data-pick').disabled
        && document.querySelector('#runner-delay').disabled;
      const done = await waitFor(() => !__verify.runnerState.active && __verify.runnerState.phase === 'done');
      return started && locked && done && !document.querySelector('#runner-data-clear').disabled
        && __verify.runnerState.dataRows.length === 2;
    })()`);
  });
  await check('stop lands promptly during a delay', async () => {
    await openRunner(null, 3000);
    await evaluate(`document.querySelector('#runner-run').click()`);
    await evaluate(`(() => {
      // Stop as soon as the first entry finishes.
      const timer = setInterval(() => {
        if (__verify.runnerState.entries[0]?.status === 'passed') {
          clearInterval(timer);
          document.querySelector('#runner-stop').click();
        }
      }, 20);
      return true;
    })()`);
    const finish = await evaluate(`new Promise(resolve => {
      const start = Date.now();
      const timer = setInterval(() => {
        if (__verify.runnerState.phase === 'done') { clearInterval(timer); resolve(Date.now() - start); }
        else if (Date.now() - start > 20000) { clearInterval(timer); resolve(99999); }
      }, 30);
    })`);
    return finish < 5000;
  });
  await check('data-driven runs coalesce history into few end-of-run flushes', async () => {
    // Count real disk-save requests through the fetch-IPC seam: coalescing must
    // keep 9 sends well under the per-send flush count.
    await evaluate(`(() => {
      window.__saveCalls = 0;
      if (!window.__saveSeam) {
        const originalFetch = window.fetch.bind(window);
        window.fetch = (input, init) => {
          const url = typeof input === 'string' ? input : input.url;
          if (url.endsWith('/save_workspace')) { window.__saveCalls += 1; }
          return originalFetch(input, init);
        };
        window.__saveSeam = true;
      }
      return true;
    })()`);
    const before = await evaluate(`window.__saveCalls`);
    await openRunner([{ n: "1" }, { n: "2" }, { n: "3" }], 0);
    await runAndFinish();
    await evaluate(`(async () => { await new Promise(r => setTimeout(r, 250)); return true })()`);
    const saves = await evaluate(`window.__saveCalls - ${JSON.stringify(before)}`);
    const history = await evaluate(`__verify.state.history.filter(h => h.name === 'First' || h.name === 'Second' || h.name === 'Third').length`);
    // 9 sends must not produce 9 saves: coalescing batches them.
    return history >= 9 && saves < 6;
  });

  // --- Task 3: report export (JSON + JUnit XML) ---------------------------------
  // The fetch-IPC seam captures save_export_file calls (no native dialogs).
  await evaluate(`(() => {
    window.__reportSaves = [];
    if (!window.__reportSeam) {
      const originalFetch = window.fetch.bind(window);
      window.fetch = (input, init) => {
        const url = typeof input === 'string' ? input : input.url;
        if (url.endsWith('/save_export_file')) {
          const args = JSON.parse(init.body);
          window.__reportSaves.push(args);
          return Promise.resolve(new Response(JSON.stringify('C:/exports/' + args.defaultName), { status: 200, headers: { 'content-type': 'application/json', 'Tauri-Response': 'ok' } }));
        }
        return originalFetch(input, init);
      };
      window.__reportSeam = true;
    }
    return true;
  })()`);
  await check('export buttons appear only in the done phase', async () => {
    await openRunner([{ n: "1" }, { n: "2" }], 0);
    await evaluate(`document.querySelector('#runner-run').click()`);
    const during = await evaluate(`(() => ({
      json: document.querySelector('#runner-export-json').hidden,
      junit: document.querySelector('#runner-export-junit').hidden,
    }))()`);
    await evaluate(`new Promise(resolve => {
      const start = Date.now();
      const timer = setInterval(() => {
        if (__verify.runnerState.phase === 'done') { clearInterval(timer); resolve(true); }
        else if (Date.now() - start > 20000) { clearInterval(timer); resolve(true); }
      }, 30);
    })`);
    const after = await evaluate(`(() => ({
      json: document.querySelector('#runner-export-json').hidden,
      junit: document.querySelector('#runner-export-junit').hidden,
    }))()`);
    return during.json === true && during.junit === true && after.json === false && after.junit === false;
  });
  await check('the JSON report captures the full run', async () => {
    await evaluate(`window.__reportSaves = []; document.querySelector('#runner-export-json').click()`);
    return await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 80));
      const args = window.__reportSaves.filter(a => a.defaultName.endsWith('.json')).at(-1);
      if (!args) return false;
      const report = JSON.parse(args.contents);
      const entries = report.entries ?? [];
      return report.kind === 'postlite.runner-report'
        && report.collection === 'Automation suite'
        && report.dataFile === 'data.json'
        && entries.length === 6
        && entries[0].iteration === 1 && entries[3].iteration === 2
        && entries.every(e => Array.isArray(e.tests));
    })()`);
  });
  await check('the JUnit XML is well-formed with suites and failures', async () => {
    // Give the run a failing assertion so the report carries <failure> nodes.
    await evaluate(`(() => {
      const c = __verify.state.collections[0];
      for (const item of c.items) item.scripts = { pre: '', test: '' };
      c.items[1].scripts = { pre: '', test: 'pm.test("boom", () => pm.expect(1).to.equal(2));' };
      return true;
    })()`);
    await openRunner([{ n: "1" }, { n: "2" }], 0);
    await runAndFinish();
    await evaluate(`window.__reportSaves = []; document.querySelector('#runner-export-junit').click()`);
    return await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 80));
      const args = window.__reportSaves.filter(a => a.defaultName.endsWith('.xml')).at(-1);
      if (!args) return false;
      const doc = new DOMParser().parseFromString(args.contents, 'text/xml');
      if (doc.querySelector('parsererror')) return false;
      const suites = [...doc.querySelectorAll('testsuite')];
      const failures = [...doc.querySelectorAll('testcase failure')];
      return doc.documentElement.tagName === 'testsuites'
        && suites.length === 6
        // Only the Second suites carry assertions; First/Third have none (tests=0).
        && suites.filter(s => Number(s.getAttribute('tests')) >= 1).length === 2
        && failures.length === 2;
    })()`);
  });

  await check('folder scripts edited in the settings pane feed the runner', async () => {
    await evaluate(`(() => {
      const c = __verify.state.collections[0];
      // The settings-pane writer (nodeScript) mutates the collection's scripts;
      // the runner must execute what those editors saved.
      for (const item of c.items) item.scripts = { pre: '', test: '' };
      c.scripts = { pre: 'pm.collectionVariables.set("fromFolder", "folder-set");', test: '' };
      c.items[0].url = 'http://127.0.0.1:8100/api/echo?ff={{fromFolder}}';
      __verify.render();
      return true;
    })()`);
    await openRunner(null, 0);
    await runAndFinish();
    return await evaluate(`(() => {
      const s = __verify.runnerState;
      return s.endReason === 'done' && s.entries[0].response.body.includes('ff=folder-set');
    })()`);
  });
  await check('transport errors export as a JUnit failure testcase', async () => {
    await evaluate(`(() => {
      const c = __verify.state.collections[0];
      for (const item of c.items) item.scripts = { pre: '', test: '' };
      c.items[0].url = 'http://127.0.0.1:9/dead';
      __verify.render();
      return true;
    })()`);
    await openRunner(null, 0);
    // No data file: the transport failure on First stops the run.
    await runAndFinish();
    await evaluate(`window.__reportSaves = []; document.querySelector('#runner-export-junit').click()`);
    return await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 80));
      const args = window.__reportSaves.filter(a => a.defaultName.endsWith('.xml')).at(-1);
      if (!args) return false;
      const doc = new DOMParser().parseFromString(args.contents, 'text/xml');
      if (doc.querySelector('parsererror')) return false;
      const transport = [...doc.querySelectorAll('testcase')].find(tc => tc.getAttribute('name') === 'transport');
      const skipped = [...doc.querySelectorAll('testcase skipped')];
      return Boolean(transport?.querySelector('failure')) && skipped.length >= 2;
    })()`);
  });
  await check('HTTP 500 responses export as a JUnit failure testcase', async () => {
    await evaluate(`(() => {
      const c = __verify.state.collections[0];
      for (const item of c.items) item.scripts = { pre: '', test: '' };
      c.items[0].url = 'http://127.0.0.1:8100/api/server-error';
      __verify.render();
      return true;
    })()`);
    await openRunner(null, 0);
    await runAndFinish();
    await evaluate(`window.__reportSaves = []; document.querySelector('#runner-export-junit').click()`);
    return await evaluate(`(async () => {
      await new Promise(r => setTimeout(r, 80));
      const args = window.__reportSaves.filter(a => a.defaultName.endsWith('.xml')).at(-1);
      if (!args) return false;
      const doc = new DOMParser().parseFromString(args.contents, 'text/xml');
      if (doc.querySelector('parsererror')) return false;
      const http = [...doc.querySelectorAll('testcase')].find(tc => tc.getAttribute('name') === 'http_status');
      const skipped = [...doc.querySelectorAll('testcase skipped')];
      return Boolean(http?.querySelector('failure')) && http.textContent.includes('500') && skipped.length >= 2;
    })()`);
  });
  await check('data-file parse errors surface inside the dialog', async () => {
    return await evaluate(`(async () => {
      const doc = { broken: true };
      const dt = new DataTransfer();
      dt.items.add(new File([JSON.stringify(doc)], "broken.json", { type: "application/json" }));
      const input = document.querySelector("#runner-data-file");
      input.files = dt.files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
      await new Promise(r => setTimeout(r, 100));
      return document.querySelector("#runner-data-error").textContent.length > 0
        && __verify.runnerState.dataRows === null;
    })()`);
  });

  console.log(`\n${results.length} checks passed`);
} catch (err) {
  console.error(`FAIL ${err.message}`);
  process.exitCode = 1;
} finally {
  await close();
}
