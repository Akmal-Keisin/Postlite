// Checks the release binary (isolated identifier) over CDP: the embedded UI
// must render the new response stage end to end. Verification only.
import { writeFileSync } from "node:fs";

const port = process.argv[2] ?? "9241";
const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = targets.find((t) => t.type === "page");
if (!page) throw new Error("app page not found on " + port);
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve) => (socket.onopen = resolve));
const pending = new Map();
let next = 0;
const problems = [];
socket.onmessage = ({ data }) => {
  const msg = JSON.parse(data);
  if (msg.method === "Runtime.exceptionThrown") problems.push("exception: " + JSON.stringify(msg.params.exceptionDetails?.exception?.description ?? msg.params));
  if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error") problems.push("console: " + msg.params.args?.map((a) => a.value ?? a.description).join(" "));
  if (!msg.id) return;
  const item = pending.get(msg.id);
  if (!item) return;
  pending.delete(msg.id);
  msg.error ? item.reject(new Error(msg.error.message)) : item.resolve(msg.result);
};
const cmd = (method, params = {}) => new Promise((resolve, reject) => {
  const id = ++next;
  pending.set(id, { resolve, reject });
  socket.send(JSON.stringify({ id, method, params }));
});
const evaluate = async (expression) => {
  const result = await cmd("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? JSON.stringify(result.exceptionDetails));
  return result.result.value;
};
const shot = async (name) => {
  const { data } = await cmd("Page.captureScreenshot", { format: "png" });
  writeFileSync(new URL(name, import.meta.url), Buffer.from(data, "base64"));
  console.log("shot", name);
};
const stage = () => evaluate(`(() => { const r = document.querySelector('#res-pane'); return { cls: r.className, empty: getComputedStyle(document.querySelector('.res-empty')).display, failed: getComputedStyle(document.querySelector('.res-failed')).display, pretty: getComputedStyle(document.querySelector('#pretty')).display, status: document.querySelector('#status').textContent, msg: document.querySelector('#res-failed-msg').textContent }; })()`);
const waitStage = (pred, ms = 15000) => evaluate(`new Promise((resolve, reject) => { const start = Date.now(); const timer = setInterval(() => { const r = document.querySelector('#res-pane'); if (${pred}) { clearInterval(timer); resolve(true); } else if (Date.now() - start > ${ms}) { clearInterval(timer); reject(new Error('stage timeout: ' + r.className)); } }, 60); })`);
const sendTo = (url) => evaluate(`(() => { const u = document.querySelector('#url'); u.value = ${JSON.stringify(url)}; u.dispatchEvent(new Event('input', { bubbles: true })); u.dispatchEvent(new Event('change', { bubbles: true })); document.querySelector('#send').click(); return true; })()`);

let failures = 0;
const check = (name, ok, detail = "") => {
  console.log((ok ? "PASS " : "FAIL ") + name + (detail ? " — " + detail : ""));
  if (!ok) failures++;
};

await cmd("Page.enable");
await cmd("Runtime.enable");
await evaluate(`new Promise((resolve, reject) => { const start = Date.now(); const timer = setInterval(() => { if (window.__TAURI__ && document.querySelector('#res-pane')) { clearInterval(timer); resolve(true); } else if (Date.now() - start > 10000) { clearInterval(timer); reject(new Error('app startup failed')); } }, 50); })`);

const markers = await evaluate(`(() => ({ empty: !!document.querySelector('#res-empty'), idleIcon: !!document.querySelector('.res-idle-icon use'), failedMsg: !!document.querySelector('#res-failed-msg'), layout: !!document.querySelector('.main > .urlbar #response-layout'), dots: getComputedStyle(document.querySelector('#workspace-empty')).backgroundImage.includes('radial-gradient') }))()`);
check("embedded UI has response stage markup", markers.empty && markers.idleIcon && markers.failedMsg, JSON.stringify(markers));
check("embedded UI has the layout switch and empty-workspace grid", markers.layout && markers.dots);

await evaluate(`document.querySelector('#workspace-empty [data-act="new-tab"]').click()`);
check("new tab opens request pane", await evaluate(`document.querySelectorAll('#tabbar .tab-item').length === 1`));
let s = await stage();
check("idle stage on a new request", s.cls.includes("is-empty") && s.empty === "grid" && s.pretty === "none", JSON.stringify(s));
await shot("buildcheck-idle.png");

await sendTo("http://127.0.0.1:8099/buildcheck");
await waitStage(`!r.classList.contains('is-empty') && !r.classList.contains('is-failed') && document.querySelector('#status').textContent !== '—'`);
s = await stage();
check("success stage renders response", s.empty === "none" && s.pretty !== "none" && /^2\d\d/.test(s.status), JSON.stringify(s));
await shot("buildcheck-success.png");

await sendTo("http://127.0.0.1:9/refused");
await waitStage(`r.classList.contains('is-failed')`);
s = await stage();
check("failed stage shows error card", s.empty === "grid" && s.failed === "grid" && s.pretty === "none" && s.msg.length > 0, JSON.stringify(s));
await shot("buildcheck-failed.png");

await evaluate(`document.querySelector('#res-tabs [data-tab="res-headers"]').click()`);
s = await stage();
check("headers reachable while failed", !s.cls.includes("is-failed") && s.failed === "none" && s.empty === "none");
await evaluate(`document.querySelector('#res-tabs [data-tab="pretty"]').click()`);
s = await stage();
check("pretty restores error card", s.cls.includes("is-failed") && s.failed === "grid");

check("no runtime problems", problems.length === 0, problems.join(" | "));
console.log(failures === 0 ? "build check: all passed" : `build check: ${failures} failure(s)`);
socket.close();
process.exit(failures === 0 ? 0 : 1);
