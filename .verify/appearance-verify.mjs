import assert from "node:assert/strict";
import { cmd, evaluate, reload, close } from "./tier1-cdp.mjs";

const click = (selector) => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
const settle = () => evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true))))");
const openAppearance = async () => {
  await click("#open-app-settings");
  await click("#app-settings-tabs [data-tab='pref-appearance']");
};
const selectAppearance = (value) => click(`#pref-appearance-${value}`);
const appearances = ["workbench", "terminal", "studio", "ledger"];
const styleSignature = () => evaluate(`(() => {
  const send = getComputedStyle(document.querySelector("#send"));
  const heading = getComputedStyle(document.querySelector(".request-head h2"));
  const tab = getComputedStyle(document.querySelector("#req-tabs .tab"));
  return [send.borderTopLeftRadius, send.height, heading.fontFamily, heading.fontSize, tab.paddingTop];
})()`);
const seed = () => evaluate(`(() => {
  const v = __verify;
  const tab = v.addTab(v.newRequest({ name: "Field notes", url: "http://127.0.0.1:8100/echo" }));
  tab.response = {
    status: 200, status_text: "OK", headers: [{key: "content-type", value: "application/json"}],
    body: '{"appearance":true}', elapsed_ms: 18, size_bytes: 19, results: [],
  };
  v.renderResponse();
})()`);

try {
  await reload();
  await cmd("Emulation.setDeviceMetricsOverride", { width: 1440, height: 820, deviceScaleFactor: 1, mobile: false });
  await evaluate(`(() => {
    const v = __verify;
    v.state.settings = v.normalizeSettings({general: {theme: "dark", accent: "ocean"}});
    v.applyAppSettings();
  })()`);
  await seed();
  await click("#res-tabs [data-tab='raw']");
  await openAppearance();
  assert.deepEqual(await evaluate(`[...document.querySelectorAll('[data-setting="general.appearance"]')].map(choice => choice.value)`), appearances);
  assert.equal(await evaluate("document.documentElement.dataset.appearance"), "workbench");
  await evaluate('document.querySelector("#pref-appearance-workbench").focus()');
  await cmd("Input.dispatchKeyEvent", { type: "keyDown", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39 });
  await cmd("Input.dispatchKeyEvent", { type: "keyUp", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39 });
  assert.equal(await evaluate("document.documentElement.dataset.appearance"), "terminal", "Arrow keys select the next appearance");
  await cmd("Emulation.setEmulatedMedia", { features: [{ name: "forced-colors", value: "active" }] });
  await settle();
  assert.deepEqual(await evaluate(`(() => {
    const selected = document.querySelector("#pref-appearance-terminal");
    return { appearance: getComputedStyle(selected).appearance, dot: getComputedStyle(selected, "::after").content };
  })()`), { appearance: "auto", dot: "none" }, "High contrast uses the native checked indicator");
  await cmd("Emulation.setEmulatedMedia", { features: [] });

  const signatures = [];
  for (const appearance of appearances) {
    await selectAppearance(appearance);
    await settle();
    assert.deepEqual(await evaluate(`({
      style: document.documentElement.dataset.appearance,
      theme: document.documentElement.dataset.theme,
      accent: document.documentElement.dataset.accent,
      raw: document.querySelector("#raw.is-active")?.textContent,
    })`), { style: appearance, theme: "dark", accent: "ocean", raw: '{"appearance":true}' });
    signatures.push(await styleSignature());
  }
  assert.equal(new Set(signatures.map(signature => JSON.stringify(signature))).size, 4, "Styles differ in shape, type, or spacing");
  await evaluate("__verify.flushWorkspace().then(() => true)");
  await reload();
  assert.equal(await evaluate("document.documentElement.dataset.appearance"), "ledger", "The style survives a workspace reload");
  await openAppearance();
  assert.equal(await evaluate('document.querySelector("#pref-appearance-ledger").checked'), true);

  await evaluate(`(() => {
    const input = document.querySelector("#pref-search");
    input.value = "workbench style";
    input.dispatchEvent(new Event("input", {bubbles: true}));
  })()`);
  assert.equal(await evaluate('document.querySelector("#pref-appearance-workbench").closest(".preferences-row").hidden'), false);
  assert.equal(await evaluate(`__verify.normalizeSettings({general:{appearance:"unknown"}}).general.appearance`), "workbench");
  await evaluate(`__verify.importSettings({kind:"postlite.settings",version:1,settings:{general:{appearance:"studio", theme:"light", accent:"mint"}}})`);
  assert.deepEqual(await evaluate(`[
    document.documentElement.dataset.appearance, document.documentElement.dataset.theme,
    document.documentElement.dataset.accent, document.querySelector("#pref-appearance-studio").checked,
  ]`), ["studio", "light", "mint", true]);
  console.log("PASS appearance selection, distinct geometry, response preservation, search, reload, and settings import");
} finally {
  await cmd("Emulation.setEmulatedMedia", { features: [] });
  await cmd("Emulation.clearDeviceMetricsOverride");
  close();
}
