import assert from "node:assert/strict";
import { cmd, evaluate, reload, close } from "./tier1-cdp.mjs";

const settle = () => evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true))))");
const settled = async (selector = ".main") => {
  await settle();
  await evaluate(`(async () => {
    const element = document.querySelector(${JSON.stringify(selector)});
    const running = () => element.getAnimations().filter(animation => animation.playState === "running");
    for (let animations = running(); animations.length; animations = running()) {
      await Promise.allSettled(animations.map(animation => animation.finished));
    }
    return true;
  })()`);
};
const viewport = async (width, height) => {
  await cmd("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  await settle();
};
const layout = () => evaluate(`(() => {
  const main = document.querySelector(".main");
  const address = document.querySelector(".urlbar");
  const req = document.querySelector("#req-pane");
  const res = document.querySelector("#res-pane");
  const toggle = document.querySelector("#response-layout");
  const divider = document.querySelector("#res-resizer");
  const request = req.getBoundingClientRect();
  const response = res.getBoundingClientRect();
  return {
    split: main.classList.contains("is-split"),
    addressAbove: address.parentElement === main && address.getBoundingClientRect().bottom <= request.top + 1,
    toggle: toggle ? { disabled: toggle.disabled, pressed: toggle.getAttribute("aria-pressed") } : null,
    orientation: divider.getAttribute("aria-orientation"),
    separated: request.right < response.left && Math.abs(request.top - response.top) < 1,
    responseWidth: Math.round(response.width),
    responseHeight: Math.round(response.height),
  };
})()`);
const click = (selector) => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
const resizeByKey = async (key) => {
  await evaluate(`document.querySelector("#res-resizer").dispatchEvent(new KeyboardEvent("keydown", { key: ${JSON.stringify(key)}, bubbles: true, cancelable: true }))`);
  await settle();
};
const seed = () => evaluate(`(() => {
  const v = __verify;
  v.state.settings.sidebarWidth = 276;
  v.applyAppSettings();
  const tab = v.addTab(v.newRequest({ name: "Workbench request", url: "http://127.0.0.1:8100/echo?workbench=shortcut" }));
  tab.response = {
    status: 200, status_text: "OK", headers: [{ key: "Content-Type", value: "application/json" }],
    body: '{"workbench": true}', elapsed_ms: 24, size_bytes: 19, results: [],
  };
  v.renderResponse();
})()`);

try {
  await reload();
  await evaluate(`["postlite.responseLayout", "postlite.responseHeight", "postlite.responseWidth"].forEach(key => localStorage.removeItem(key))`);
  await viewport(1440, 820);
  await reload();
  await seed();
  await settle();

  const stacked = await layout();
  assert.ok(stacked.toggle, "The request bar offers a response layout switch");
  assert.deepEqual(stacked.toggle, { disabled: false, pressed: "false" });
  assert.equal(stacked.addressAbove, true);
  assert.equal(stacked.split, false);
  assert.equal(stacked.orientation, "horizontal");

  await resizeByKey("ArrowUp");
  const taller = await layout();
  assert.equal(taller.responseHeight, stacked.responseHeight + 16);

  await click("#response-layout");
  await settle();
  const split = await layout();
  assert.deepEqual(split.toggle, { disabled: false, pressed: "true" });
  assert.equal(split.addressAbove, true);
  assert.equal(split.split, true);
  assert.equal(split.separated, true);
  assert.equal(split.orientation, "vertical");

  await resizeByKey("ArrowLeft");
  assert.equal((await layout()).responseWidth, split.responseWidth + 16);
  const divider = await evaluate(`(() => {
    const box = document.querySelector("#res-resizer").getBoundingClientRect();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  })()`);
  const hit = await evaluate(`document.elementFromPoint(${divider.x}, ${divider.y})?.id`);
  assert.equal(hit, "res-resizer", "The divider receives pointer input");
  await cmd("Input.dispatchMouseEvent", { type: "mousePressed", ...divider, button: "left", buttons: 1, clickCount: 1 });
  assert.equal(await evaluate('document.body.classList.contains("resizing-response-x")'), true, "Pointer down starts the horizontal drag");
  const dragged = { x: divider.x - 22, y: divider.y };
  await cmd("Input.dispatchMouseEvent", { type: "mouseMoved", ...dragged, buttons: 1 });
  await settle();
  const dragging = await layout();
  assert.equal(dragging.responseWidth, split.responseWidth + 38, "Moving the pointer updates split width");
  await cmd("Input.dispatchMouseEvent", { type: "mouseReleased", ...dragged, button: "left", buttons: 0, clickCount: 1 });
  await settle();
  const wider = await layout();
  assert.equal(wider.responseWidth, split.responseWidth + 38);
  assert.equal(await evaluate('localStorage.getItem("postlite.responseWidth")'), String(wider.responseWidth));
  await click("#res-tabs [data-tab='raw']");
  assert.equal(await evaluate("document.querySelector('#raw.is-active').textContent"), '{"workbench": true}');

  await evaluate(`document.querySelector("#res-resizer").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true, cancelable: true }))`);
  assert.equal(await evaluate('localStorage.getItem("postlite.responseWidth")'), String(wider.responseWidth),
    "Ctrl+Enter sends without resetting the divider");

  await click("#code-link");
  await viewport(1240, 820);
  await settled();
  assert.equal((await layout()).split, false, "The code drawer leaves room for the stacked editor");
  await click("#drawer-close");
  await viewport(1080, 600);
  const compact = await layout();
  assert.equal(compact.split, false);
  assert.deepEqual(compact.toggle, { disabled: true, pressed: "false" });
  await click("#code-link");
  await settled();
  assert.ok(await evaluate(`document.querySelector("#url").getBoundingClientRect().width >= 100`),
    "The default sidebar and drawer leave a usable URL input at minimum window width");
  await click("#drawer-close");

  await viewport(1440, 820);
  await settled();
  const restored = await layout();
  assert.equal(restored.split, true);
  assert.equal(restored.responseWidth, wider.responseWidth);
  await reload();
  await seed();
  await settle();
  assert.equal((await layout()).responseWidth, wider.responseWidth, "Split width is restored after reload");

  await click("#response-layout");
  await settle();
  assert.equal((await layout()).responseHeight, taller.responseHeight, "Stacked height is stored independently");

  await evaluate(`(() => {
    const input = document.querySelector("#url");
    input.value = "http://127.0.0.1:8100/echo?workbench=1";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    return __verify.send();
  })()`);
  assert.equal(await evaluate("document.querySelector('#status').textContent"), "200 OK");
  await click("#res-tabs [data-tab='raw']");
  assert.match(await evaluate("document.querySelector('#raw').textContent"), /workbench=1/);

  await evaluate(`(() => {
    __verify.state.settings.sidebarWidth = 480;
    __verify.applyAppSettings();
  })()`);
  await viewport(1080, 600);
  await click("#code-link");
  await settled();
  assert.equal(await evaluate(`(() => {
    const url = document.querySelector("#url").getBoundingClientRect();
    const send = document.querySelector("#send").getBoundingClientRect();
    const drawer = document.querySelector("#drawer").getBoundingClientRect();
    return url.width > 70 && url.right < drawer.left && send.top >= url.bottom;
  })()`), true, "A large sidebar and open drawer leave the URL and Send usable");
  await click("#drawer-close");
  await settled();

  await evaluate(`(() => {
    __verify.state.tabs = [];
    __verify.state.activeTabId = null;
    __verify.state.selected = null;
    __verify.render();
  })()`);
  assert.equal(await evaluate(`document.querySelector(".urlbar").offsetHeight`), 0);
  assert.equal(await evaluate(`document.querySelector("#workspace-empty").hidden`), false);
  const newRequest = "#workspace-empty .primary";
  const action = await evaluate(`(() => {
    const box = document.querySelector(${JSON.stringify(newRequest)}).getBoundingClientRect();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  })()`);
  await cmd("Input.dispatchMouseEvent", { type: "mouseMoved", ...action });
  await settled(newRequest);
  assert.ok(await evaluate(`(() => {
    const button = document.querySelector(${JSON.stringify(newRequest)});
    const styles = getComputedStyle(button);
    return button.matches(":hover") && styles.color !== styles.backgroundColor;
  })()`), "The New request label stays distinct from the hover background");
  console.log("PASS workbench layouts, resize keyboard actions, width fallback, response continuity, and reload");
} finally {
  await cmd("Emulation.clearDeviceMetricsOverride");
  close();
}
