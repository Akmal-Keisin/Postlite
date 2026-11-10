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

let passed = 0;
let failed = 0;
const check = (label, actual, expected) => {
  const ok = typeof expected === "function" ? expected(actual) : String(actual).includes(String(expected));
  ok ? passed++ : failed++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${label}${ok ? "" : `  (got ${JSON.stringify(actual)}, want ${expected})`}`);
};

const width = () => evaluate(`Math.round(document.getElementById("sidebar").getBoundingClientRect().width)`);
const savedWidth = () => evaluate(`localStorage.getItem("postlite.sidebarWidth")`);
const inlineVar = () => evaluate(`document.querySelector(".shell").style.getPropertyValue("--sidebar-w")`);
const ariaNow = () => evaluate(`document.getElementById("sidebar-resizer").getAttribute("aria-valuenow")`);
const handleCenter = () =>
  evaluate(`(() => { const r = document.getElementById("sidebar-resizer").getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);

const mouse = (type, x, y, extra = {}) =>
  cmd("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: 1, clickCount: 1, ...extra });

async function dragBy(dx) {
  const { x, y } = await handleCenter();
  await cmd("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await mouse("mousePressed", x, y);
  for (let i = 1; i <= 6; i++) await mouse("mouseMoved", x + (dx * i) / 6, y);
  await mouse("mouseReleased", x + dx, y, { buttons: 0 });
  await sleep(120);
}

async function doubleClickHandle() {
  const { x, y } = await handleCenter();
  await mouse("mousePressed", x, y);
  await mouse("mouseReleased", x, y, { buttons: 0 });
  await mouse("mousePressed", x, y, { clickCount: 2 });
  await mouse("mouseReleased", x, y, { buttons: 0, clickCount: 2 });
  await sleep(150);
}

async function key(name, code, vk) {
  await cmd("Input.dispatchKeyEvent", { type: "keyDown", key: name, code, windowsVirtualKeyCode: vk });
  await cmd("Input.dispatchKeyEvent", { type: "keyUp", key: name, code, windowsVirtualKeyCode: vk });
  await sleep(80);
}

async function reload() {
  await cmd("Page.reload", { ignoreCache: true });
  for (let i = 0; i < 60; i++) {
    await sleep(150);
    try {
      if ((await evaluate(`document.readyState`)) === "complete" && (await evaluate(`!!document.getElementById("sidebar-resizer")`))) break;
    } catch {}
  }
  await sleep(400);
}

await cmd("Runtime.enable");
await cmd("Page.enable");

/* clean slate */
await evaluate(`localStorage.removeItem("postlite.sidebarWidth")`);
await reload();

check("separator role present", await evaluate(`document.getElementById("sidebar-resizer").getAttribute("role")`), "separator");
check("default width 276", await width(), (w) => Math.abs(w - 276) <= 2);

/* drag wider */
await dragBy(120);
check("drag widens sidebar", await width(), (w) => Math.abs(w - 396) <= 3);
check("width persisted to localStorage", await savedWidth(), (v) => Math.abs(Number(v) - 396) <= 3);
check("aria-valuenow synced", await ariaNow(), (v) => Math.abs(Number(v) - 396) <= 3);
check("inline css var set", await inlineVar(), "px");

/* survives reload */
await reload();
check("width restored after reload", await width(), (w) => Math.abs(w - 396) <= 3);

/* clamping */
await dragBy(2000);
check("clamped at max 480", await width(), (w) => w === 480);
await dragBy(-2000);
check("clamped at min 180", await width(), (w) => w === 180);

/* double-click reset */
await doubleClickHandle();
check("dblclick clears inline var", await inlineVar(), (v) => v === "");
check("dblclick restores default", await width(), (w) => Math.abs(w - 276) <= 2);
check("dblclick clears storage", await savedWidth(), (v) => v === null);

/* keyboard */
await evaluate(`document.getElementById("sidebar-resizer").focus()`);
await key("ArrowRight", "ArrowRight", 39);
check("ArrowRight grows 16px", await width(), (w) => Math.abs(w - 292) <= 2);
check("keyboard change persisted", await savedWidth(), (v) => Math.abs(Number(v) - 292) <= 2);
await key("ArrowLeft", "ArrowLeft", 37);
check("ArrowLeft shrinks 16px", await width(), (w) => Math.abs(w - 276) <= 2);
await key("Enter", "Enter", 13);
check("Enter resets to default", await inlineVar(), (v) => v === "");

/* collapsed rail hides handle */
await evaluate(`document.getElementById("rail-collections").click()`);
await sleep(150);
check("handle hidden when collapsed", await evaluate(`getComputedStyle(document.getElementById("sidebar-resizer")).display`), "none");
await evaluate(`document.getElementById("rail-collections").click()`);
await sleep(150);
check("handle back after expand", await evaluate(`getComputedStyle(document.getElementById("sidebar-resizer")).display`), (v) => v !== "none");

/* hover feedback screenshot */
const { x, y } = await handleCenter();
await cmd("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
await sleep(350);
const image = await cmd("Page.captureScreenshot", { format: "png" });
writeFileSync(join(here, "ui-resize.png"), Buffer.from(image.data, "base64"));
console.log("shot: ui-resize");

console.log(errors.length ? `RUNTIME ERRORS:\n${errors.join("\n")}` : "no runtime errors");
console.log(`${passed}/${passed + failed} resize checks passed`);
ws.close();
process.exitCode = failed || errors.length ? 1 : 0;