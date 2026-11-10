import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { writeFileSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.argv[2] ?? 9223);
const COLLECTION_FILE = join(here, "sample-collection.json").replace(/\//g, "\\");
const ENV_FILE = join(here, "sample-environment.json").replace(/\//g, "\\");

const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const page = targets.find((t) => t.type === "page");
if (!page) throw new Error("no CDP page target");

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));

let nextId = 0;
const pending = new Map();
const eventWaiters = new Map();
ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
  } else if (msg.method && eventWaiters.has(msg.method)) {
    const queue = eventWaiters.get(msg.method);
    queue.shift()();
    if (!queue.length) eventWaiters.delete(msg.method);
  }
};

const cmd = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });

const wait = (method) =>
  new Promise((resolve) => {
    const queue = eventWaiters.get(method) ?? [];
    queue.push(resolve);
    eventWaiters.set(method, queue);
  });

const evaluate = async (expression) => {
  const result = await cmd("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? JSON.stringify(result.exceptionDetails));
  }
  return result.result.value;
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function until(expression, label, timeout = 8000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await evaluate(expression);
    if (value) return value;
    await sleep(150);
  }
  throw new Error(`timed out waiting for ${label}`);
}

async function reload() {
  const loaded = wait("Page.loadEventFired");
  await cmd("Page.reload", {});
  await loaded;
  await sleep(400);
}

const checks = [];
function check(label, actual, expected) {
  const pass = String(actual).includes(expected);
  checks.push(
    `${pass ? "PASS" : "FAIL"}  ${label}: ${JSON.stringify(String(actual)).slice(0, 220)}` +
      (pass ? "" : ` -> expected to contain ${JSON.stringify(expected)}`)
  );
  if (!pass) process.exitCode = 1;
}

const setInput = (selector, value, type = "change") =>
  evaluate(
    `(() => { const el = document.querySelector(${JSON.stringify(selector)}); ` +
      `if (!el) throw new Error("missing " + ${JSON.stringify(selector)}); el.value = ${JSON.stringify(value)}; ` +
      `el.dispatchEvent(new Event(${JSON.stringify(type)}, { bubbles: true })); return el.value; })()`
  );

const click = (selector) =>
  evaluate(
    `(() => { const el = document.querySelector(${JSON.stringify(selector)}); ` +
      `if (!el) throw new Error("missing " + ${JSON.stringify(selector)}); el.click(); return "clicked"; })()`
  );

const sidebarText = `document.getElementById("collections").textContent.replace(/\\s+/g, " ")`;

const openRequestByName = (name) =>
  evaluate(`(() => {
    const row = [...document.querySelectorAll(".node.request")].find(n => n.textContent.includes(${JSON.stringify(name)}));
    if (!row) return "not found";
    row.click();
    return "opened";
  })()`);

const openFolderByName = (name) =>
  evaluate(`(() => {
    const head = [...document.querySelectorAll(".node.group > .node-head")]
      .find(h => h.querySelector(":scope > .node-name").textContent.trim() === ${JSON.stringify(name)});
    if (!head) return "not found";
    head.querySelector(":scope > .node-name").click();
    return document.getElementById("settings-title").textContent;
  })()`);

const nested = (parent, child) =>
  evaluate(`(() => {
    const nameOf = (node) => {
      const head = node.querySelector(":scope > .node-head > .node-name");
      return (head ?? node.querySelector(":scope > .node-name"))?.textContent.trim();
    };
    const parentNode = [...document.querySelectorAll(".node")].find(n => nameOf(n) === ${JSON.stringify(parent)});
    if (!parentNode) return "parent missing";
    const body = parentNode.querySelector(":scope > .node-body");
    if (!body) return "parent has no body";
    const childNode = [...body.querySelectorAll(".node")].find(n => nameOf(n) === ${JSON.stringify(child)});
    return childNode ? "nested" : "not nested";
  })()`);

// the pill is reset first so a slow response cannot be mistaken for the previous one
async function sendRequest() {
  await evaluate(`(() => { const p = document.getElementById("status"); p.className = "pill"; p.textContent = "idle"; return "ok"; })()`);
  await click("#send");
  await until(`document.getElementById("status").textContent !== "idle"`, "response");
  const pill = await evaluate(`document.getElementById("status").textContent`);
  if (pill === "failed") throw new Error(`request failed: ${await evaluate(`document.getElementById("error").textContent`)}`);
  return JSON.parse(await evaluate(`document.getElementById("pretty").textContent`));
}

const header = (echo, name) => String(echo.headers[name] ?? "(missing)");

async function screenshot(name) {
  const image = await cmd("Page.captureScreenshot", { format: "png" });
  const path = join(here, `${name}.png`);
  writeFileSync(path, Buffer.from(image.data, "base64"));
  return path;
}

await cmd("Runtime.enable");
await cmd("Page.enable");
await cmd("DOM.enable");

/* 1. a workspace.json written before folders existed must still load */
check("legacy format migrated", await evaluate(sidebarText), "Old flat request");
check("folders collapsed by default", await evaluate(`[...document.querySelectorAll(".node.group")].every(n => n.classList.contains("collapsed"))`), "true");
check("open legacy request", await openRequestByName("Old flat request"), "opened");
const legacy = await sendRequest();
check("legacy request sends", legacy.method, "GET");
check("status pill", await evaluate(`document.getElementById("status").textContent`), "200");
check("no tests yet", await evaluate(`document.getElementById("tests").textContent`), "No tests ran");

/* 2. save the active request into a collection through the dialog */
await click("#save");
await until(`document.getElementById("save-dialog").open`, "save dialog");
await evaluate(`document.getElementById("save-name").value = "Hello check"`);
await evaluate(`document.querySelector('#save-dialog button[value="save"]').click()`);
await until(`${sidebarText}.includes("Hello check")`, "saved request in sidebar");

/* 3. persistence across a reload */
await sleep(700);
await reload();
check("persisted after reload", await evaluate(sidebarText), "Hello check");

/* 4. import keeps Postman folders, including a folder inside a folder */
const { root } = await cmd("DOM.getDocument", { depth: -1 });
const inputNode = await cmd("DOM.querySelector", { nodeId: root.nodeId, selector: "#import-file" });
await cmd("DOM.setFileInputFiles", { files: [COLLECTION_FILE], nodeId: inputNode.nodeId });
await until(`${sidebarText}.includes("Sample Import")`, "imported collection");
const imported = await evaluate(sidebarText);
for (const label of ["Users", "Admin", "List users", "Purge cache", "Create user", "Upload form", "Ping"]) {
  check(`import shows ${label}`, imported, label);
}
check("Users contains List users", await nested("Users", "List users"), "nested");
check("Users contains subfolder Admin", await nested("Users", "Admin"), "nested");
check("Admin contains Purge cache", await nested("Admin", "Purge cache"), "nested");
check("Users contains Upload form", await nested("Users", "Upload form"), "nested");

/* 5. a request two levels deep opens with its mapping intact */
check("open nested request", await openRequestByName("Purge cache"), "opened");
await sleep(200);
check("nested method", await evaluate(`document.getElementById("method").value`), "DELETE");
check("nested url", await evaluate(`document.getElementById("url").value`), "/admin/purge");

/* 6. collection variables interpolate the url and folder auth is inherited */
check("open List users", await openRequestByName("List users"), "opened");
await sleep(200);
const listUsers = await sendRequest();
check("{{baseUrl}} substituted in url", listUsers.path, "/users?page=2&limit=10");
check("apikey inherited from folder", header(listUsers, "x-api-key"), "secret-key");
check("collection variable used in auth field", header(listUsers, "x-api-key"), "secret-key");

/* 7. the deepest folder wins for auth, its pre-request script feeds the token */
await openRequestByName("Purge cache");
await sleep(200);
const purged = await sendRequest();
check("parent apikey not inherited", header(purged, "x-api-key"), "(missing)");
check("folder pre-request var reached bearer token", header(purged, "authorization"), "Bearer admin-token");

/* 8. request level auth wins over every ancestor */
check("open Ping", await openRequestByName("Ping"), "opened");
await sleep(200);
check("request auth type rendered", await evaluate(`document.getElementById("req-auth-type").value`), "basic");
check("request auth field rendered", await evaluate(`document.querySelector('#req-auth-fields input[data-k="username"]').value`), "alice");
const pinged = await sendRequest();
check("basic auth header", header(pinged, "authorization"), `Basic ${Buffer.from("alice:wonderland").toString("base64")}`);
check("collection level test ran", await evaluate(`document.getElementById("tests").textContent`), "collection level test ran");

/* 9. form-data body plus request scripts */
check("open Upload form", await openRequestByName("Upload form"), "opened");
await sleep(200);
check("form-data mode selected", await evaluate(`document.querySelector('input[name="body-mode"]:checked').value`), "form-data");
check("form table visible", await evaluate(`document.getElementById("form-wrap").className`), "is-visible");
check("form keys imported", await evaluate(`[...document.querySelectorAll('#form-table input[data-field="key"]')].map(i => i.value).join(",")`), "note,scope");
check("form values imported", await evaluate(`[...document.querySelectorAll('#form-table input[data-field="value"]')].map(i => i.value).join(",")`), "{{apiKey}},{{scope}}");
check("raw textarea disabled in form-data", await evaluate(`document.getElementById("body").disabled`), "true");
const uploaded = await sendRequest();
check("multipart content type", header(uploaded, "content-type"), "multipart/form-data; boundary=");
check("form field interpolated from vars", uploaded.body, "secret-key");
check("folder variable overrides collection variable", uploaded.body, "folder-users-scope");
check("collection variable not used when folder defines it", String(uploaded.body.includes("collection-scope")), "false");
check("form field names echoed", uploaded.body, `name="scope"`);
check("pre-request script header", header(uploaded, "x-scripted"), "yes");
const tests = await evaluate(`document.getElementById("tests").textContent.replace(/\\s+/g, " ")`);
check("passing test listed", tests, "server accepted a multipart body");
check("failing test listed", tests, "deliberately failing assertion");
check("failing test reason", tests, "expected 200 to equal 418");
check("summary line", tests, "3 assertions, 1 failing");
await click('[data-tab="tests"]');
await sleep(450);
await screenshot("ui-tests");

/* 10. create a folder through the UI and save a request into it */
check("reopen Purge cache", await openRequestByName("Purge cache"), "opened");
await evaluate(`(() => {
  const collection = [...document.querySelectorAll(".node")].find(n => n.dataset.kind === "collection" && n.dataset.cid === n.dataset.id);
  collection.querySelector(":scope > .node-head [data-act=\\"new-folder\\"]").click();
  return "ok";
})()`);
await until(`document.getElementById("collection-dialog").open`, "folder name dialog");
await evaluate(`document.getElementById("collection-name").value = "Sandbox"`);
await evaluate(`document.querySelector('#collection-dialog button[value="create"]').click()`);
await until(`${sidebarText}.includes("Sandbox")`, "folder in sidebar");

await click("#save");
await until(`document.getElementById("save-dialog").open`, "save dialog for folder");
await evaluate(`(() => {
  const select = document.getElementById("save-collection");
  const option = [...select.options].find(o => o.textContent.trim().endsWith("Sandbox"));
  if (!option) throw new Error("Sandbox not offered in save dialog: " + [...select.options].map(o => o.textContent).join("|"));
  select.value = option.value;
  document.getElementById("save-name").value = "Moved POST";
  return option.value;
})()`);
await evaluate(`document.querySelector('#save-dialog button[value="save"]').click()`);
await until(`${sidebarText}.includes("Moved POST")`, "request saved into new folder");
check("Sandbox contains Moved POST", await nested("Sandbox", "Moved POST"), "nested");
check("moving a request leaves its old folder", await nested("Admin", "Purge cache"), "not nested");
check("folder path offered in save dialog", await evaluate(`[...document.querySelectorAll("#save-dialog option")].map(o => o.textContent).join(" | ")`), "Legacy Collection / Sandbox");

/* 11. folder settings typed in the UI, then inherited by the request inside it */
check("open Moved POST", await openRequestByName("Moved POST"), "opened");
check("Sandbox settings open", await openFolderByName("Sandbox"), "Sandbox");
check("settings pane visible", await evaluate(`document.getElementById("settings-pane").hidden`), "false");
check("request view hidden", await evaluate(`document.querySelector(".main").className`), "settings-open");
await setInput("#node-auth-type", "bearer");
await setInput('#node-auth-fields input[data-k="token"]', "typed-token", "input");
for (const [key, value] of [["baseUrl", "http://127.0.0.1:8099"], ["typedVar", "typedValue"]]) {
  await click("[data-add-vars]");
  await setInput('#vars-table tr:last-child input[data-field="key"]', key, "input");
  await setInput('#vars-table tr:last-child input[data-field="value"]', value, "input");
}
check("variable count badge", await evaluate(`document.getElementById("vars-count").textContent`), "2");
await click('[data-tab="node-vars"]');
await sleep(350);
await screenshot("ui-settings");

await click(".tab-item.is-active");
check("back to request view", await evaluate(`document.getElementById("settings-pane").hidden`), "true");
check("request auth stays inherit", await evaluate(`document.getElementById("req-auth-type").value`), "inherit");
await click('[data-tab="params"]');
await click('[data-add="params"]');
await setInput('#params-table tr:last-child input[data-field="key"]', "v", "input");
await setInput('#params-table tr:last-child input[data-field="value"]', "{{typedVar}}", "input");
const moved = await sendRequest();
check("auth typed in settings is sent", header(moved, "authorization"), "Bearer typed-token");
check("variable typed in settings interpolated", moved.path, "v=typedValue");
check("folder variable resolves baseUrl in url", moved.path, "/admin/purge");
check("request script result shown", await evaluate(`document.getElementById("tests").textContent`), "No tests ran");

/* 12. folder auth and variables come back after a reload */
await sleep(700);
await reload();
check("folder auth persisted", await evaluate(`(() => {
  const head = [...document.querySelectorAll(".node.group > .node-head")]
    .find(h => h.querySelector(":scope > .node-name").textContent.trim() === "Sandbox");
  head.querySelector(":scope > .node-name").click();
  return document.getElementById("node-auth-type").value + "/" + document.querySelector('#node-auth-fields input[data-k="token"]').value;
})()`), "bearer/typed-token");
check("folder variables persisted", await evaluate(`(() => {
  const keys = [...document.querySelectorAll('#vars-table input[data-field="key"]')].map(i => i.value);
  const values = [...document.querySelectorAll('#vars-table input[data-field="value"]')].map(i => i.value);
  return keys.join(",") + "/" + values.join(",");
})()`), "typedVar");
check("both folder variables persisted", await evaluate(`document.getElementById("vars-count").textContent`), "2");

/* 13. folding still works now that clicking a name opens settings */
const caretToggle = () => evaluate(`(() => {
  const find = () => [...document.querySelectorAll(".node.group > .node-head")]
    .find(h => h.querySelector(":scope > .node-name").textContent.trim() === "Sandbox");
  const before = find().closest(".node.group").classList.contains("collapsed");
  find().querySelector(":scope > .caret").click();
  return before !== find().closest(".node.group").classList.contains("collapsed");
})()`);
check("caret folds the folder", await caretToggle(), "true");
check("caret expands the folder", await caretToggle(), "true");

/* 14. variable peek overlay resolves {{tokens}} with their defining scope */
check("open Moved POST for peek", await openRequestByName("Moved POST"), "opened");
check("var peek visible", await evaluate(`document.getElementById("var-peek").hidden`), "false");
await click("#var-peek");
check("overlay resolves baseUrl", await evaluate(`document.getElementById("var-overlay").textContent`), "http://127.0.0.1:8099");
check("overlay shows defining scope", await evaluate(`document.getElementById("var-overlay").textContent`), "Sandbox");
check("overlay resolved preview", await evaluate(`document.querySelector("#var-overlay .var-resolved code").textContent`), "http://127.0.0.1:8099/admin/purge");
check("method options on dark bg", await evaluate(`getComputedStyle(document.querySelector('#method option[value="POST"]')).backgroundColor`), "21, 26, 33");
await evaluate(`document.dispatchEvent(new KeyboardEvent("keydown", {key:"Escape", bubbles:true}))`);
check("esc closes overlay", await evaluate(`document.getElementById("var-overlay").hidden`), "true");

/* 15. UX pass: double-click fold, keyboard caret, filter, expand/collapse all,
   middle-click close, dirty dot, copy response */
const sandboxHead = `[...document.querySelectorAll(".node.group > .node-head")]
  .find(h => h.querySelector(":scope > .node-name").textContent.trim() === "Sandbox")`;

check("dblclick toggles folder", await evaluate(`(() => {
  const head = ${sandboxHead};
  const before = head.closest(".node.group").classList.contains("collapsed");
  head.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  return before !== ${sandboxHead}.closest(".node.group").classList.contains("collapsed");
})()`), "true");
check("dblclick toggles back", await evaluate(`(() => {
  const head = ${sandboxHead};
  const was = head.closest(".node.group").classList.contains("collapsed");
  head.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  const now = ${sandboxHead}.closest(".node.group").classList.contains("collapsed");
  return was !== now;
})()`), "true");

check("caret is a button", await evaluate(`document.querySelector(".node.group > .node-head > .caret").tagName`), "BUTTON");
check("caret has aria-expanded", await evaluate(`document.querySelector(".node.group > .node-head > .caret").getAttribute("aria-expanded") !== null`), "true");

await evaluate(`document.querySelector(".node.group > .node-head > .caret").focus()`);
const ariaBefore = await evaluate(`document.activeElement.getAttribute("aria-expanded")`);
await cmd("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13, text: "\r", unmodifiedText: "\r" });
await cmd("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 });
const ariaAfter = await evaluate(`document.querySelector(".node.group > .node-head > .caret")?.getAttribute("aria-expanded") ?? document.activeElement.getAttribute("aria-expanded")`);
check("keyboard Enter toggles caret", String(ariaBefore !== ariaAfter), "true");

await setInput("#tree-search", "moved", "input");
check("filter keeps only matching requests", await evaluate(`(() => {
  const rows = [...document.querySelectorAll("#collections .node.request")];
  return rows.length > 0 && rows.every(n => n.textContent.toLowerCase().includes("moved"));
})()`), "true");
check("filter expands ancestors", await evaluate(`[...document.querySelectorAll("#collections .node.group")].every(n => !n.classList.contains("collapsed"))`), "true");
check("filter clear button visible", await evaluate(`document.getElementById("tree-search-clear").hidden`), "false");
await evaluate(`(() => { const el = document.getElementById("tree-search"); el.focus(); el.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); })()`);
check("esc clears filter", await evaluate(`document.getElementById("tree-search").value === "" && document.getElementById("tree-search-clear").hidden`), "true");
await setInput("#tree-search", "zz-no-match", "input");
check("no-match empty state", await evaluate(`document.getElementById("collections").textContent`), "No matches");
await click("#tree-search-clear");
check("clear button restores tree", await evaluate(`document.getElementById("collections").textContent.includes("Sample Import")`), "true");

await click("#expand-all");
check("expand all opens every group", await evaluate(`[...document.querySelectorAll("#collections .node.group")].every(n => !n.classList.contains("collapsed"))`), "true");
await click("#collapse-all");
check("collapse all folds every group", await evaluate(`[...document.querySelectorAll("#collections .node.group")].every(n => n.classList.contains("collapsed"))`), "true");

await openRequestByName("Ping");
await sleep(200);
const tabsBefore = await evaluate(`document.querySelectorAll("#tabbar .tab-item").length`);
await evaluate(`(() => {
  const item = document.querySelector("#tabbar .tab-item:not(.is-active)");
  item.dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true }));
})()`);
check("middle click closes tab", String((await evaluate(`document.querySelectorAll("#tabbar .tab-item").length`)) === tabsBefore - 1), "true");

await openRequestByName("List users");
await sleep(200);
check("no dirty dot on clean tab", await evaluate(`!!document.querySelector(".tab-item.is-active .tab-dirty")`), "false");
await setInput("#url", "{{baseUrl}}/users?page=2&limit=11", "change");
await sleep(550);
check("edit shows dirty dot", await evaluate(`!!document.querySelector(".tab-item.is-active .tab-dirty")`), "true");
await click("#save");
await until(`document.getElementById("save-dialog").open`, "save dialog for dirty tab");
await evaluate(`document.querySelector('#save-dialog button[value="save"]').click()`);
await until(`!document.querySelector(".tab-item.is-active .tab-dirty")`, "dirty dot cleared after save");
check("save clears dirty dot", await evaluate(`!!document.querySelector(".tab-item.is-active .tab-dirty")`), "false");

const copied = await sendRequest();
check("response sent for copy", copied.method, "GET");
check("copy button visible", await evaluate(`document.getElementById("copy-res").hidden`), "false");
const copyRect = await evaluate(`(() => { const r = document.getElementById("copy-res").getBoundingClientRect(); return JSON.stringify({ x: r.x + r.width / 2, y: r.y + r.height / 2 }); })()`);
const { x: cx, y: cy } = JSON.parse(copyRect);
await cmd("Input.dispatchMouseEvent", { type: "mousePressed", x: cx, y: cy, button: "left", clickCount: 1 });
await cmd("Input.dispatchMouseEvent", { type: "mouseReleased", x: cx, y: cy, button: "left", clickCount: 1 });
await sleep(300);
check("copy feedback shown", await evaluate(`document.getElementById("copy-res").classList.contains("copied")`), "true");
await sleep(1500);
check("copy feedback resets", await evaluate(`document.getElementById("copy-res").classList.contains("copied")`), "false");

/* 16. QUERY method is selectable, colored, and actually sent */
check("QUERY option exists", await evaluate(`[...document.querySelectorAll("#method option")].map(o => o.value).join(",")`), "QUERY");
await setInput("#method", "QUERY", "change");
check("method select set to QUERY", await evaluate(`document.getElementById("method").value`), "QUERY");
check("urlbar data-method QUERY", await evaluate(`document.getElementById("method").dataset.method`), "QUERY");
const queryResp = await sendRequest();
check("server received QUERY method", queryResp.method, "QUERY");
check("QUERY select color is cyan", await evaluate(`getComputedStyle(document.getElementById("method")).color`), "53, 201, 221");
check("QUERY tab badge rendered", await evaluate(`!!document.querySelector(".tab-item.is-active .method.QUERY")`), "true");

/* 17. environments: rail view, editor, secret masking, precedence over collection,
       Postman environment export/import, and a binding that survives a reload */
const envText = `document.getElementById("environments").textContent.replace(/\\s+/g, " ")`;
const envHead = (name) =>
  `[...document.querySelectorAll('#environments .node > .node-head')]
     .find(h => h.querySelector(":scope > .node-name").textContent.trim() === ${JSON.stringify(name)})`;
const openEnvironmentByName = (name) =>
  evaluate(`(() => {
    const head = ${envHead(name)};
    if (!head) return "not found";
    head.querySelector(":scope > .node-name").click();
    return document.getElementById("settings-title").textContent;
  })()`);
const varsRow = (key) =>
  `[...document.querySelectorAll('#vars-table tbody tr')].find(tr => tr.querySelector('input[data-field="key"]').value === ${JSON.stringify(key)})`;
const sampleHead = `[...document.querySelectorAll('.node.group > .node-head')]
  .find(h => h.querySelector(":scope > .node-name").textContent.trim() === "Sample Import")`;

await click("#rail-environments");
check("environments panel shown", await evaluate(`document.getElementById("environments").hidden`), "false");
check("collections panel folded away", await evaluate(`document.getElementById("collections").hidden`), "true");
check("sidebar retitled", await evaluate(`document.getElementById("sidebar-title").textContent`), "Environments");
check("folder toolbar hidden here", await evaluate(`document.getElementById("expand-all").hidden && document.getElementById("new-collection").hidden`), "true");
check("add environment offered", await evaluate(`document.getElementById("new-environment").hidden`), "false");
check("environment empty state", await evaluate(envText), "No environments yet");
check("rail marks environments", await evaluate(`document.getElementById("rail-environments").classList.contains("is-active")`), "true");

await click("#new-environment");
await until(`document.getElementById("collection-dialog").open`, "environment name dialog");
await evaluate(`document.getElementById("collection-name").value = "Staging"`);
await evaluate(`document.querySelector('#collection-dialog button[value="create"]').click()`);
await until(`${envText}.includes("Staging")`, "environment row");
check("environment editor open", await openEnvironmentByName("Staging"), "Staging");
check("settings kind is environment", await evaluate(`document.getElementById("settings-kind").textContent`), "environment");
check("auth tab hidden for environment", await evaluate(`document.querySelector('#settings-tabs .tab[data-tab="node-auth"]').hidden`), "true");
check("scripts tab hidden for environment", await evaluate(`document.querySelector('#settings-tabs .tab[data-tab="node-scripts"]').hidden`), "true");
check("variables pane shown", await evaluate(`document.querySelector('.pane[data-pane="node-vars"]').classList.contains("is-active")`), "true");
check("binding picker moved out of settings", await evaluate(`!document.getElementById("node-env-wrap")`), "true");

for (const [key, value] of [["apiKey", "env-api-key"], ["envSecret", "s3cr3t-value"], ["scope", "env-scope"]]) {
  await click("[data-add-vars]");
  await setInput('#vars-table tr:last-child input[data-field="key"]', key, "input");
  await setInput('#vars-table tr:last-child input[data-field="value"]', value, "input");
}
check("environment variable count", await evaluate(`document.getElementById("vars-count").textContent`), "3");
await evaluate(`(() => {
  const el = (${varsRow("envSecret")}).querySelector('input[data-field="secret"]');
  el.checked = true;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  return "ok";
})()`);
check("secret value masked in table", await evaluate(`(${varsRow("envSecret")}).querySelector('input[data-field="value"]').type`), "password");
check("list masks the secret", await evaluate(envText), "envSecret=••••••••");
check("list hides the raw secret", await evaluate(`${envText}.includes("s3cr3t-value")`), "false");

await setInput("#tree-search", "stag", "input");
check("environment filter keeps match", await evaluate(`document.querySelectorAll("#environments .node").length`), "1");
await setInput("#tree-search", "zz-no-env", "input");
check("environment filter empty state", await evaluate(envText), "No matches");
await click("#tree-search-clear");
check("filter cleared restores list", await evaluate(`document.querySelectorAll("#environments .node").length`), "1");

await evaluate(`(() => {
  window.__origClick = HTMLAnchorElement.prototype.click;
  window.__dl = null;
  HTMLAnchorElement.prototype.click = function () { window.__dl = { name: this.download, href: this.href }; };
  return "patched";
})()`);
await click(`#environments .node [data-act="export"]`);
check("export produced a file", await evaluate(`!!window.__dl`), "true");
check("export file name", await evaluate(`(window.__dl || {}).name || "none"`), "Staging.postman_environment.json");
const exported = JSON.parse(await evaluate(`fetch(window.__dl.href).then(r => r.text())`));
await evaluate(`(() => { HTMLAnchorElement.prototype.click = window.__origClick; return "restored"; })()`);
check("export keeps environment name", exported.name, "Staging");
check("export writes value rows", JSON.stringify(exported.values), "env-api-key");
check("export marks secret type", JSON.stringify(exported.values), '"type":"secret"');

const { root: envRoot } = await cmd("DOM.getDocument", { depth: -1 });
const envInput = await cmd("DOM.querySelector", { nodeId: envRoot.nodeId, selector: "#env-import-file" });
await cmd("DOM.setFileInputFiles", { files: [ENV_FILE], nodeId: envInput.nodeId });
await until(`${envText}.includes("Prod Imported")`, "imported environment");
check("imported environment opens", await openEnvironmentByName("Prod Imported"), "Prod Imported");
check("import marks one secret", await evaluate(`[...document.querySelectorAll('#vars-table tbody tr')].filter(tr => tr.querySelector('input[data-field="secret"]').checked).length`), "1");
check("import keeps plain value", await evaluate(`[...document.querySelectorAll('#vars-table input[data-field="value"]:not([type="password"])')].map(i => i.value).join(",")`), "prod-api-key");
check("import keeps disabled row", await evaluate(`[...document.querySelectorAll('#vars-table tbody tr')].filter(tr => !tr.querySelector('input[data-field="enabled"]').checked).length`), "1");
check("imported secret masked in list", await evaluate(`${envText}.includes("prod-secret-token")`), "false");
check("two environments listed", await evaluate(`document.querySelectorAll("#environments .node").length`), "2");

await click("#rail-collections");
check("collections panel back", await evaluate(`document.getElementById("collections").hidden`), "false");
check("environments panel hidden", await evaluate(`document.getElementById("environments").hidden`), "true");
check("rail marks collections", await evaluate(`document.getElementById("rail-collections").classList.contains("is-active")`), "true");
await openRequestByName("List users");
await sleep(200);
check("env bar shown for collection tab", await evaluate(`document.getElementById("env-bar").hidden`), "false");
check("binding options listed", await evaluate(`[...document.querySelectorAll("#env-select option")].map(o => o.textContent).join("|")`), "No environment|Staging|Prod Imported");
const stagingId = await evaluate(`[...document.querySelectorAll("#env-select option")].find(o => o.textContent === "Staging").value`);
await setInput("#env-select", stagingId, "change");
check("binding saved", await evaluate(`document.getElementById("env-select").value`), stagingId);

await openRequestByName("List users");
await sleep(200);
const envBound = await sendRequest();
check("environment beats collection variable", header(envBound, "x-api-key"), "env-api-key");
await openRequestByName("Upload form");
await sleep(200);
const envForm = await sendRequest();
check("folder beats environment variable", envForm.body, "folder-users-scope");
check("environment beats collection in body", envForm.body, "env-api-key");
check("environment scope unused when folder wins", String(envForm.body.includes("env-scope")), "false");

await evaluate(`${sampleHead}.querySelector(":scope > .node-acts [data-act=\\"new-request\\"]").click()`);
await sleep(250);
await setInput("#url", "{{baseUrl}}/{{envSecret}}", "change");
await click("#var-peek");
const peekEnv = await evaluate(`document.getElementById("var-overlay").textContent.replace(/\\s+/g, " ")`);
check("overlay masks the secret", peekEnv, "••••••••");
check("overlay hides raw secret", String(peekEnv.includes("s3cr3t-value")), "false");
check("overlay names environment scope", peekEnv, "Environment: Staging");
check("resolved preview masks secret", await evaluate(`document.querySelector("#var-overlay .var-resolved code").textContent`), "••••••••");
const secretSent = await sendRequest();
check("secret still sent in full", secretSent.path, "/s3cr3t-value");
await evaluate(`document.dispatchEvent(new KeyboardEvent("keydown", {key:"Escape", bubbles:true}))`);
await sleep(200);
await screenshot("ui-environments");

await sleep(700);
await reload();
await click("#rail-environments");
check("environments persisted", await evaluate(envText), "Staging");
check("secret still masked after reload", await evaluate(`${envText}.includes("s3cr3t-value")`), "false");
await click("#rail-collections");
await openRequestByName("List users");
await sleep(200);
check("binding persisted", await evaluate(`document.getElementById("env-select").value !== ""`), "true");

await setInput("#env-select", "", "change");
check("unbind selects no environment", await evaluate(`document.getElementById("env-select").selectedIndex`), "0");
await openRequestByName("List users");
await sleep(200);
const unbound = await sendRequest();
check("collection variable used again", header(unbound, "x-api-key"), "secret-key");

await click("#rail-environments");
await evaluate(`(() => {
  const head = ${envHead("Staging")};
  head.querySelector('[data-act="delete"]').click();
  return "ok";
})()`);
await until(`${envText}.includes("Staging") === false`, "environment deleted");
check("deleting keeps the rest", await evaluate(envText), "Prod Imported");
await click("#rail-collections");
await openRequestByName("List users");
await sleep(200);
const prodId = await evaluate(`[...document.querySelectorAll('#env-select option')].find(o => o.textContent === "Prod Imported").value`);
await setInput("#env-select", prodId, "change");
check("rebound to imported environment", await evaluate(`document.getElementById("env-select").value`), prodId);
await click("#rail-environments");
await evaluate(`(() => {
  const head = ${envHead("Prod Imported")};
  head.querySelector('[data-act="delete"]').click();
  return "ok";
})()`);
await until(`document.getElementById("env-select").value === ""`, "binding cleared with its environment");
check("dangling binding cleared", await evaluate(`document.getElementById("env-select").selectedIndex`), "0");
check("empty state returns", await evaluate(envText), "No environments yet");
await click("#rail-collections");

/* 18. tab right-click menu: close other tabs, close all tabs */
const tabCount = () => evaluate(`document.querySelectorAll("#tabbar .tab-item").length`);
const rightClickTab = (idx) => evaluate(`(() => {
  const tab = document.querySelectorAll("#tabbar .tab-item")[${idx}];
  if (!tab) throw new Error("no tab at index " + ${idx});
  tab.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 240, clientY: 120 }));
  return tab.dataset.id;
})()`);

await openRequestByName("Ping");
await sleep(150);
await openRequestByName("List users");
await sleep(150);
check("several tabs open for menu test", String((await tabCount()) >= 3), "true");
const clickedId = await rightClickTab(0);
check("menu opens on right click", await evaluate(`document.getElementById("tab-menu").hidden`), "false");
check("menu offers close others", await evaluate(`document.getElementById("tab-menu").textContent`), "Close other tabs");
check("menu offers close all", await evaluate(`document.getElementById("tab-menu").textContent`), "Close all tabs");
await click('#tab-menu [data-menu="close-others"]');
check("close others keeps one tab", String(await tabCount()), "1");
check("close others keeps the clicked tab", await evaluate(`document.querySelector("#tabbar .tab-item").dataset.id`), clickedId);
check("close others activates it", await evaluate(`document.querySelector("#tabbar .tab-item").classList.contains("is-active")`), "true");
check("menu closes after action", await evaluate(`document.getElementById("tab-menu").hidden`), "true");

await openRequestByName("Upload form");
await sleep(150);
await rightClickTab(0);
await click('#tab-menu [data-menu="close-all"]');
check("close all leaves one tab", String(await tabCount()), "1");
check("close all opens a blank request", await evaluate(`document.querySelector("#tabbar .tab-name").textContent`), "Untitled Request");
check("close all hides menu", await evaluate(`document.getElementById("tab-menu").hidden`), "true");

await rightClickTab(0);
await evaluate(`document.dispatchEvent(new KeyboardEvent("keydown", {key:"Escape", bubbles:true}))`);
check("esc closes the tab menu", await evaluate(`document.getElementById("tab-menu").hidden`), "true");

await sleep(300);
const shotPath = await screenshot("ui-state");

console.log(checks.join("\n"));
console.log(`\n${checks.filter((c) => c.startsWith("PASS")).length}/${checks.length} checks passed`);
console.log(`screenshot: ${shotPath}`);
ws.close();
