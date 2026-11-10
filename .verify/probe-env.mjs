import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.argv[2] ?? 9229);
const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const page = targets.find((t) => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let nextId = 0;
const pending = new Map();
const errors = [];
ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg.result);
    pending.delete(msg.id);
  } else if (msg.method === "Runtime.exceptionThrown") {
    errors.push(`exception: ${msg.params.exceptionDetails.exception?.description ?? JSON.stringify(msg.params.exceptionDetails)}`);
  }
};
const cmd = (method, params = {}) =>
  new Promise((resolve) => {
    const id = ++nextId;
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => {
  const r = await cmd("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? JSON.stringify(r.exceptionDetails));
  return r.result.value;
};
await cmd("Runtime.enable");

const NL = "String.fromCharCode(10)";
const probe = `(() => {
  const out = [];
  const envRows = [...document.querySelectorAll('#environments .node > .node-head')].map(h => (h.querySelector(":scope > .node-name") || {}).textContent);
  out.push('env rows in DOM: ' + JSON.stringify(envRows));
  out.push('environments panel hidden now: ' + document.getElementById('environments').hidden);
  document.getElementById('rail-collections').click();
  out.push('collections hidden after rail click: ' + document.getElementById('collections').hidden);
  const head = [...document.querySelectorAll('.node.group > .node-head')].find(h => h.querySelector(":scope > .node-name").textContent.trim() === 'Sample Import');
  out.push('sample head found: ' + !!head);
  head.querySelector(":scope > .node-name").click();
  out.push('settings-title: ' + document.getElementById('settings-title').textContent + ' | kind: ' + document.getElementById('settings-kind').textContent);
  out.push('pane hidden: ' + document.getElementById('settings-pane').hidden + ' | env-wrap hidden: ' + document.getElementById('node-env-wrap').hidden);
  out.push('node-env options: ' + [...document.querySelectorAll('#node-env option')].map(o => o.textContent).join(' | '));
  out.push('active settings tab: ' + (document.querySelector('#settings-tabs .tab.is-active') || {}).dataset?.tab);
  out.push('vars table keys: ' + [...document.querySelectorAll('#vars-table tbody tr')].map(tr => (tr.querySelector('input[data-field="key"]') || {}).value).join(','));
  document.querySelector('[data-tab="node-vars"]').click();
  out.push('after vars tab click -> options: ' + [...document.querySelectorAll('#node-env option')].map(o => o.textContent).join(' | '));
  return out.join(${NL});
})()`;

console.log(await evaluate(probe));
console.log(errors.length ? `ERRORS:\n${errors.join("\n")}` : "no exceptions during probe");
ws.close();
