import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { writeFileSync } from "node:fs";

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
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
  } else if (msg.method === "Runtime.exceptionThrown") {
    errors.push(`exception: ${msg.params.exceptionDetails.exception?.description ?? JSON.stringify(msg.params.exceptionDetails)}`);
  } else if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error") {
    errors.push(`console: ${msg.params.args.map((a) => a.value ?? a.description).join(" ")}`);
  }
};
const cmd = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => {
  const result = await cmd("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description);
  return result.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const click = (selector) =>
  evaluate(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) throw new Error("missing " + ${JSON.stringify(selector)}); el.click(); return "ok"; })()`);
const open = (name) =>
  evaluate(`(() => {
    const row = [...document.querySelectorAll(".node.request")].find(n => n.textContent.includes(${JSON.stringify(name)}));
    if (!row) throw new Error("request not found: " + ${JSON.stringify(name)});
    row.click(); return "ok";
  })()`);
const shot = async (name) => {
  const image = await cmd("Page.captureScreenshot", { format: "png" });
  const path = join(here, `${name}.png`);
  writeFileSync(path, Buffer.from(image.data, "base64"));
  console.log(`shot: ${name}`);
};

await cmd("Runtime.enable");
await cmd("Page.enable");

/* pretty JSON with syntax highlighting */
await open("List users");
await sleep(250);
await evaluate(`(() => { const p = document.getElementById("status"); p.className = "pill"; p.textContent = "idle"; })()`);
await click("#send");
for (let i = 0; i < 40; i++) {
  if ((await evaluate(`document.getElementById("status").textContent`)) !== "idle") break;
  await sleep(150);
}
await click('#res-tabs [data-tab="pretty"]');
await sleep(500);
console.log("highlight spans:", await evaluate(`document.querySelectorAll("#pretty span").length`));
console.log("pretty text intact:", await evaluate(`(() => { try { JSON.parse(document.getElementById("pretty").textContent); return true; } catch { return false; } })()`));
await shot("ui-pretty");

/* form-data body editor */
await open("Upload form");
await sleep(250);
await click('#req-tabs [data-tab="body"]');
await sleep(350);
await shot("ui-body");

/* variable peek overlay under the url bar */
await click("#var-peek");
await sleep(350);
await shot("ui-vars");
await evaluate(`document.dispatchEvent(new KeyboardEvent("keydown", {key:"Escape", bubbles:true}))`);
await sleep(150);

/* sidebar filter narrows the tree to matching requests */
await evaluate(`(() => { const el = document.getElementById("tree-search"); el.value = "user"; el.dispatchEvent(new Event("input", { bubbles: true })); })()`);
await sleep(350);
await shot("ui-search");
await evaluate(`document.getElementById("tree-search").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`);
await sleep(200);

/* environments: rail view, editor with secret masking, collection binding */
const setVal = async (selector, value, type = "input") =>
  evaluate(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) throw new Error("missing " + ${JSON.stringify(selector)}); el.value = ${JSON.stringify(value)}; el.dispatchEvent(new Event(${JSON.stringify(type)}, { bubbles: true })); return "ok"; })()`);
await click("#rail-environments");
await sleep(250);
await click("#new-environment");
await sleep(250);
await evaluate(`document.getElementById("collection-name").value = "Staging"`);
await click('#collection-dialog button[value="create"]');
await sleep(350);
for (const [key, value] of [["api_url", "http://127.0.0.1:8099"], ["token", "super-secret-token"]]) {
  await click("[data-add-vars]");
  await sleep(120);
  await setVal('#vars-table tr:last-child input[data-field="key"]', key);
  await setVal('#vars-table tr:last-child input[data-field="value"]', value);
}
await sleep(200);
await evaluate(`(() => {
  const row = [...document.querySelectorAll('#vars-table tbody tr')].find(tr => tr.querySelector('input[data-field="key"]').value === "token");
  const el = row.querySelector('input[data-field="secret"]');
  el.checked = true;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  return "ok";
})()`);
await sleep(300);
console.log("secret masked in table:", await evaluate(`[...document.querySelectorAll('#vars-table input[data-field="value"]')].map(i => i.type).join(",")`));
console.log("list text:", await evaluate(`document.getElementById("environments").textContent.replace(/\\s+/g, " ")`));
await shot("ui-env");
await click("#rail-collections");
await sleep(250);
await open("List users");
await sleep(300);
console.log("env bar hidden for a collection tab:", await evaluate(`document.getElementById("env-bar").hidden`));
console.log("binding options:", await evaluate(`[...document.querySelectorAll("#env-select option")].map(o => o.textContent).join("|")`));
await setVal("#env-select", await evaluate(`[...document.querySelectorAll("#env-select option")].find(o => o.textContent === "Staging").value`), "change");
await sleep(300);
await shot("ui-env-bind");

/* tab right-click menu */
await evaluate(`(() => {
  const tab = document.querySelector("#tabbar .tab-item");
  if (!tab) throw new Error("no tab to right click");
  tab.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 240, clientY: 120 }));
  return "ok";
})()`);
await sleep(250);
await shot("ui-tabmenu");
await evaluate(`document.dispatchEvent(new KeyboardEvent("keydown", {key:"Escape", bubbles:true}))`);
await sleep(150);

/* save dialog */
await click("#save");
await sleep(350);
await shot("ui-dialog");
await click('#save-dialog button[value="cancel"]');
await sleep(200);

/* narrow window: no horizontal overflow, controls reachable */
await cmd("Emulation.setDeviceMetricsOverride", { width: 900, height: 620, deviceScaleFactor: 1, mobile: false });
await sleep(450);
console.log("narrow overflow:", await evaluate(`document.documentElement.scrollWidth > window.innerWidth`));
console.log("narrow send visible:", await evaluate(`(() => { const r = document.getElementById("send").getBoundingClientRect(); return r.right <= window.innerWidth && r.width > 40; })()`));
await shot("ui-narrow");
await cmd("Emulation.clearDeviceMetricsOverride");
await sleep(300);

console.log(errors.length ? `RUNTIME ERRORS:\n${errors.join("\n")}` : "no runtime errors");
ws.close();
process.exitCode = errors.length ? 1 : 0;