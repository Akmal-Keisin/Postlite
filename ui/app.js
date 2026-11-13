const invoke = window.__TAURI__.core.invoke;

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
const uid = () => Math.random().toString(36).slice(2, 10);

const esc = (text) =>
  String(text).replace(/[&<>"]/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[ch]);

const AUTH_LABELS = {
  inherit: "Inherit from parent",
  noauth: "No auth",
  apikey: "API key",
  bearer: "Bearer token",
  basic: "Basic auth",
};

const SECRET_MASK = "••••••••";

const DEFAULT_SETTINGS = {
  general: { theme: "dark", autoSave: true, autoSaveIntervalMs: 350, confirmBeforeDelete: true, restoreLastSession: true },
  request: { defaultMethod: "GET", requestTimeoutMs: 60000, followRedirects: true, maxRedirects: 10, sslVerify: true, disableCache: false, decodeResponse: true, persistCookies: true, proxy: { mode: "direct", url: "", bypass: "" } },
  response: { defaultView: "pretty", syntaxHighlight: true, jsonIndent: 2, wordWrap: true, showHeaders: true, showTime: true, showSize: true, maxResponseSizeKB: 2000 },
  privacy: { maskSensitiveHeaders: true, maskSecretVariables: true },
  sidebarWidth: 276,
};
const SETTING_OPTIONS = {
  theme: ["light", "dark", "system"],
  defaultMethod: ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "QUERY"],
  defaultView: ["pretty", "raw", "preview"],
  jsonIndent: [2, 4, "tab"],
  mode: ["direct", "manual", "environment"],
};
const SETTING_RANGES = {
  autoSaveIntervalMs: [100, 60000], requestTimeoutMs: [1, 3600000], maxRedirects: [0, 100],
  maxResponseSizeKB: [1, 102400], sidebarWidth: [180, 480],
};

function normalizeSettings(raw, defaults = DEFAULT_SETTINGS) {
  const source = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
  return Object.fromEntries(Object.entries(defaults).map(([key, fallback]) => {
    const value = Object.hasOwn(source, key) ? source[key] : undefined;
    if (fallback && typeof fallback === "object") return [key, normalizeSettings(value, fallback)];
    const range = SETTING_RANGES[key];
    const valid = SETTING_OPTIONS[key] ? SETTING_OPTIONS[key].includes(value)
      : range ? Number.isInteger(value) && value >= range[0] && value <= range[1]
      : typeof value === typeof fallback;
    return [key, valid ? value : fallback];
  }));
}

const state = {
  settings: normalizeSettings(),
  workspaces: [],
  activeWorkspaceId: null,
  collections: [],
  environments: [],
  tabs: [],
  activeTabId: null,
  expanded: new Set(),
  filter: "",
  selected: null,
  sidebarView: "collections",
};

/* ---------------------------------------------------------------------- model */

function normAuth(raw, fallbackType) {
  const src = raw ?? {};
  return {
    type: src.type ?? fallbackType,
    bearer: { token: src.bearer?.token ?? "" },
    basic: { username: src.basic?.username ?? "", password: src.basic?.password ?? "" },
    apikey: { key: src.apikey?.key ?? "", value: src.apikey?.value ?? "", addTo: src.apikey?.addTo ?? "header" },
  };
}

function normRows(rows) {
  return (rows ?? []).map((row) => ({
    key: row.key ?? "",
    value: row.value ?? "",
    enabled: row.enabled !== false,
    secret: row.secret === true,
  }));
}

const normScripts = (raw) => ({ pre: raw?.pre ?? "", test: raw?.test ?? "" });

function normBody(raw) {
  const mode = raw?.mode ?? "none";
  return {
    mode: mode === "form-data" || mode === "raw" || mode === "none" ? mode : "raw",
    raw: raw?.raw ?? "",
    contentType: raw?.contentType ?? "application/json",
    form: normRows(raw?.form),
  };
}

function newRequest(fields = {}) {
  return {
    kind: "request",
    id: fields.id ?? uid(),
    name: fields.name ?? "Untitled Request",
    method: fields.method ?? state.settings.request.defaultMethod,
    url: fields.url ?? "",
    params: normRows(fields.params),
    headers: fields.headers === undefined ? [{ key: "Accept", value: "*/*", enabled: true }] : normRows(fields.headers),
    body: normBody(fields.body),
    auth: normAuth(fields.auth, "inherit"),
    scripts: normScripts(fields.scripts),
  };
}

function newFolder(fields = {}) {
  return {
    kind: "folder",
    id: fields.id ?? uid(),
    name: fields.name ?? "New folder",
    items: (fields.items ?? []).map(fromStored),
    auth: normAuth(fields.auth, "noauth"),
    variables: normRows(fields.variables),
    scripts: normScripts(fields.scripts),
  };
}

const fromStored = (item) => (item.kind === "folder" ? newFolder(item) : newRequest(item));

function newCollection(name, fields = {}) {
  return {
    id: fields.id ?? uid(),
    name,
    items: (fields.items ?? []).map(fromStored),
    auth: normAuth(fields.auth, "noauth"),
    variables: normRows(fields.variables),
    scripts: normScripts(fields.scripts),
    environmentId: fields.environmentId ?? null,
  };
}

function newEnvironment(fields = {}) {
  return {
    id: fields.id ?? uid(),
    name: fields.name ?? "New environment",
    variables: normRows(fields.variables),
  };
}

// A workspace groups collections and environments; openTabs/openTabId are the
// runtime tab strip that belongs to it (never serialized directly — persist
// writes requests and environment references back into `tabs`).
function newWorkspace(fields = {}) {
  return {
    id: fields.id ?? uid(),
    name: fields.name ?? "My Workspace",
    collections: fields.collections ?? [],
    environments: fields.environments ?? [],
    openTabs: fields.openTabs ?? [],
    openTabId: fields.openTabId ?? null,
  };
}

// workspace.json written before folders existed kept requests in a flat field.
const migrateCollection = (stored) =>
  newCollection(stored.name ?? "Collection", { ...stored, items: stored.items ?? stored.requests ?? [] });

const activeTab = () => state.tabs.find((t) => t.id === state.activeTabId) ?? null;
const activeReq = () => activeTab()?.req ?? null;

const locateWorkspace = (id) => state.workspaces.find((w) => w.id === id) ?? null;
const activeWorkspace = () => locateWorkspace(state.activeWorkspaceId);

/* --------------------------------------------------------------- url / headers */

function joinQuery(base, pairs) {
  const query = pairs
    .filter((p) => String(p.key).trim())
    .map((p) => `${encodeURIComponent(p.key)}=${encodeURIComponent(p.value)}`)
    .join("&");
  if (!query) return base;
  return `${base}${base.includes("?") ? "&" : "?"}${query}`;
}

function splitUrl(raw) {
  const [base, query = ""] = String(raw).split("?");
  const decode = (text) => {
    try {
      return decodeURIComponent(text);
    } catch {
      return text;
    }
  };
  const params = query
    .split("&")
    .filter(Boolean)
    .map((pair) => {
      const [key, ...rest] = pair.split("=");
      return { key: decode(key), value: decode(rest.join("=")), enabled: true };
    });
  return { base, params };
}

function enabledHeaders(req) {
  const headers = req.headers
    .filter((h) => h.enabled && h.key.trim())
    .map((h) => ({ key: h.key.trim(), value: h.value }));
  // form-data takes its Content-Type (with the multipart boundary) from reqwest.
  if (req.body.mode === "raw" && req.body.contentType) {
    if (!headers.some((h) => h.key.toLowerCase() === "content-type")) {
      headers.push({ key: "Content-Type", value: req.body.contentType });
    }
  }
  return headers;
}

/* --------------------------------------------------------- inheritance plumbing */

function pathTo(id) {
  const walk = (node, acc) => {
    const chain = [...acc, node];
    if (node.id === id) return chain;
    for (const child of node.items ?? []) {
      const found = walk(child, chain);
      if (found) return found;
    }
    return null;
  };
  for (const collection of state.collections) {
    const found = walk(collection, []);
    if (found) return found;
  }
  return null;
}

// The collection and folders a request sits inside; empty for an unsaved tab.
function parentChain(id) {
  const chain = pathTo(id) ?? [];
  const last = chain[chain.length - 1];
  return last && last.kind === "request" ? chain.slice(0, -1) : chain;
}

function envForCollection(collection) {
  return state.environments.find((env) => env.id === collection?.environmentId) ?? null;
}

// Precedence: collection < environment < folder, so the environment slots in
// right after the collection instead of joining the chain.
function collectVars(parents) {
  const env = envForCollection(parents[0]);
  const chain = env ? [parents[0], env, ...parents.slice(1)] : parents;
  const vars = new Map();
  for (const node of chain) {
    for (const row of node.variables ?? []) {
      if (row.enabled !== false && row.key.trim()) vars.set(row.key.trim(), row.value);
    }
  }
  return vars;
}

function resolveAuth(req, parents) {
  if (req.auth.type !== "inherit") return req.auth;
  for (const node of [...parents].reverse()) {
    if (node.auth && node.auth.type !== "inherit") return node.auth;
  }
  return normAuth(null, "noauth");
}

function interpolate(text, vars) {
  return String(text).replace(/\{\{\s*([^{}]+?)\s*\}\}/g, (whole, name) =>
    vars.has(name) ? String(vars.get(name)) : whole
  );
}

const base64 = (text) => {
  let binary = "";
  for (const byte of new TextEncoder().encode(text)) binary += String.fromCharCode(byte);
  return btoa(binary);
};

function authHeaders(auth, vars) {
  const value = (text) => interpolate(text, vars);
  if (auth.type === "bearer" && value(auth.bearer.token)) {
    return [{ key: "Authorization", value: `Bearer ${value(auth.bearer.token)}` }];
  }
  if (auth.type === "basic") {
    const credentials = `${value(auth.basic.username)}:${value(auth.basic.password)}`;
    return [{ key: "Authorization", value: `Basic ${base64(credentials)}` }];
  }
  if (auth.type === "apikey" && auth.apikey.addTo === "header" && value(auth.apikey.key)) {
    return [{ key: value(auth.apikey.key), value: value(auth.apikey.value) }];
  }
  return [];
}

function authQuery(auth, vars) {
  if (auth.type !== "apikey" || auth.apikey.addTo !== "query") return [];
  const key = interpolate(auth.apikey.key, vars);
  return key ? [{ key, value: interpolate(auth.apikey.value, vars) }] : [];
}

/* ------------------------------------------------------------ script mini engine */

const fmt = (value) => {
  try {
    const text = typeof value === "string" ? value : JSON.stringify(value);
    return text ?? String(value);
  } catch {
    return String(value);
  }
};

function deepEqual(a, b) {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  return ka.length === kb.length && ka.every((key) => deepEqual(a[key], b[key]));
}

class Expectation {
  constructor(value, negated = false, deep = false) {
    this.value = value;
    this.negatedByNot = negated;
    this.assumeDeep = deep;
  }

  next(negated = this.negatedByNot, deep = this.assumeDeep) {
    return new Expectation(this.value, negated, deep);
  }

  get to() { return this }
  get be() { return this }
  get been() { return this }
  get is() { return this }
  get that() { return this }
  get have() { return this }
  get has() { return this }
  get with() { return this }
  get and() { return this }
  get at() { return this }
  get not() { return this.next(!this.negatedByNot) }
  get deep() { return this.next(this.negatedByNot, true) }

  get ok() {
    return this.check(Boolean(this.value), `expected ${fmt(this.value)} to be truthy`);
  }

  check(passed, message) {
    if (this.negatedByNot ? passed : !passed) throw new Error(this.negatedByNot ? `expected NOT ${message}` : message);
    return this;
  }

  equal(target) {
    const same = this.assumeDeep ? deepEqual(this.value, target) : this.value === target;
    return this.check(same, `expected ${fmt(this.value)} to equal ${fmt(target)}`);
  }

  eql(target) {
    return new Expectation(this.value, this.negatedByNot, true).equal(target);
  }

  include(item) {
    let found = false;
    if (typeof this.value === "string") found = this.value.includes(String(item));
    else if (this.value && typeof this.value[Symbol.iterator] === "function")
      found = [...this.value].some((entry) => deepEqual(entry, item));
    return this.check(found, `expected ${fmt(this.value)} to include ${fmt(item)}`);
  }

  a(type) {
    const actual = this.value === null ? "null" : Array.isArray(this.value) ? "array" : typeof this.value;
    return this.check(actual === type, `expected type ${actual} to be ${type}`);
  }

  length(n) {
    const actual = this.value?.length;
    return this.check(actual === n, `expected length ${actual} to be ${n}`);
  }

  above(n) { return this.check(this.value > n, `expected ${fmt(this.value)} to be above ${n}`) }
  below(n) { return this.check(this.value < n, `expected ${fmt(this.value)} to be below ${n}`) }
}

function requestApi(prepared) {
  const find = (name) => prepared.headers.find((h) => h.key.toLowerCase() === String(name).toLowerCase());
  return {
    get url() { return prepared.url },
    set url(next) { prepared.url = String(next) },
    get method() { return prepared.method },
    set method(next) { prepared.method = String(next).toUpperCase() },
    get query() { return prepared.params },
    headers: {
      get: (name) => find(name)?.value,
      set: (name, value) => {
        const existing = find(name);
        if (existing) existing.value = String(value);
        else prepared.headers.push({ key: String(name), value: String(value) });
      },
      unset: (name) => {
        const index = prepared.headers.findIndex((h) => h.key.toLowerCase() === String(name).toLowerCase());
        if (index >= 0) prepared.headers.splice(index, 1);
      },
      all: () => prepared.headers.map((h) => ({ ...h })),
    },
  };
}

function responseApi(response) {
  const get = (name) => response.headers.find((h) => h.key.toLowerCase() === String(name).toLowerCase())?.value;
  return {
    status: response.status_text,
    code: response.status,
    responseTime: response.elapsed_ms,
    headers: { get, all: () => response.headers.map((h) => ({ ...h })) },
    text: () => response.body,
    json: () => JSON.parse(response.body),
  };
}

function buildPm({ vars, request, response, results }) {
  const pm = {
    test(name, fn) {
      try {
        fn();
        results.push({ name: String(name), passed: true });
      } catch (err) {
        results.push({ name: String(name), passed: false, message: String(err?.message ?? err) });
      }
    },
    expect: (value) => new Expectation(value),
    variables: {
      get: (name) => vars.get(String(name)),
      set: (name, value) => vars.set(String(name), value),
      replace: (text) => interpolate(text, vars),
    },
  };
  if (request) pm.request = requestApi(request);
  if (response) pm.response = responseApi(response);
  return pm;
}

function runScript(code, pm, label, results) {
  if (!code || !code.trim()) return;
  try {
    new Function("pm", code)(pm);
  } catch (err) {
    results.push({ name: `${label} script error`, passed: false, message: String(err?.message ?? err) });
  }
}

/* ------------------------------------------------------------------- workspace */

let workspaceReady = false;
let saveTimer;
let saveQueue = Promise.resolve();
let revision = 0;
let savedRevision = 0;

function syncWorkspace() {
  const current = activeWorkspace();
  if (current) {
    current.collections = state.collections;
    current.environments = state.environments;
    current.openTabs = state.tabs;
    current.openTabId = state.activeTabId;
  }
}

function workspacePayload() {
  syncWorkspace();
  return {
    version: 2,
    settings: state.settings,
    activeWorkspaceId: state.activeWorkspaceId,
    workspaces: state.workspaces.map((ws) => ({
      id: ws.id,
      name: ws.name,
      collections: ws.collections,
      environments: ws.environments,
      tabs: (ws.openTabs ?? []).map((t) => (t.kind === "environment" ? { kind: "environment", envId: t.envId } : t.req)),
      activeTabIndex: (ws.openTabs ?? []).findIndex((t) => t.id === ws.openTabId),
      activeTabId: ws.openTabId ?? null,
    })),
  };
}

function flushWorkspace() {
  clearTimeout(saveTimer);
  if (!workspaceReady) return Promise.resolve();
  const savingRevision = revision;
  const payload = JSON.stringify(workspacePayload());
  const save = saveQueue.catch(() => {}).then(async () => {
    $(".statusbar").classList.add("saving");
    $("#save-state").textContent = "Saving…";
    try {
      await invoke("save_workspace", { state: payload });
      savedRevision = savingRevision;
      $(".statusbar").classList.toggle("unsaved", savedRevision !== revision);
      $("#save-state").textContent = savedRevision !== revision ? "Unsaved"
        : `Saved ${new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}`;
    } catch (err) {
      $(".statusbar").classList.add("unsaved");
      $("#save-state").textContent = "Save failed";
      showError(String(err));
      throw err;
    } finally {
      $(".statusbar").classList.remove("saving");
    }
  });
  saveQueue = save;
  return save;
}

function persist(structural = true) {
  if (!workspaceReady) return;
  revision++;
  syncWorkspace();
  renderTabbar();
  $(".statusbar").classList.add("unsaved");
  $("#save-state").textContent = "Unsaved";
  clearTimeout(saveTimer);
  if (structural) void flushWorkspace().catch(() => {});
  else if (state.settings.general.autoSave) {
    saveTimer = setTimeout(() => void flushWorkspace().catch(() => {}), state.settings.general.autoSaveIntervalMs);
  }
}

function showError(message) {
  $("#error").textContent = message;
  if ($("#app-settings-dialog")?.open) $("#preferences-message").textContent = message;
}

function hydrateWorkspace(saved) {
  const ws = newWorkspace({ id: saved.id, name: saved.name ?? "My Workspace" });
  ws.collections = (saved.collections ?? []).map(migrateCollection);
  ws.environments = (saved.environments ?? []).map(newEnvironment);
  ws.openTabs = state.settings.general.restoreLastSession
    ? (saved.tabs ?? [])
        .filter((t) => t?.kind !== "environment" || ws.environments.some((env) => env.id === t.envId))
        .map((t) => (t?.kind === "environment"
          ? { id: uid(), kind: "environment", envId: t.envId }
          : { id: uid(), req: newRequest(t), response: null }))
    : [];
  ws.openTabId = ws.openTabs[saved.activeTabIndex]?.id ?? ws.openTabs[0]?.id ?? null;
  return ws;
}

async function loadWorkspace() {
  migrateSidebarWidth();
  const raw = await invoke("load_workspace");
  if (raw) {
    try {
      const saved = JSON.parse(raw);
      state.settings = normalizeSettings(saved.settings);
      if (!Object.hasOwn(saved.settings ?? {}, "sidebarWidth")) migrateSidebarWidth();
      // v1 files carry one implicit workspace; applySavedPayload handles both.
      if (saved.workspaces?.length || saved.collections || saved.environments || saved.tabs)
        applySavedPayload(saved, "load");
    } catch {
      showError("saved workspace was unreadable, starting fresh");
    }
  }
  if (!state.workspaces.length) {
    const ws = newWorkspace();
    state.workspaces.push(ws);
    state.activeWorkspaceId = ws.id;
  }
  if (!state.activeWorkspaceId) state.activeWorkspaceId = state.workspaces[0].id;
  enterWorkspace(activeWorkspace());
}

// Rebind the live state onto a workspace's data; each workspace keeps its own
// tab strip, collections, and environments.
function enterWorkspace(ws) {
  state.activeWorkspaceId = ws.id;
  state.collections = ws.collections;
  state.environments = ws.environments;
  state.tabs = ws.openTabs ?? [];
  state.activeTabId = ws.openTabId ?? null;
  const tab = state.tabs.find((t) => t.id === state.activeTabId);
  state.selected = tab?.kind === "environment" ? { id: tab.envId, kind: "environment" } : null;
  state.expanded = new Set();
  state.filter = "";
  $("#tree-search").value = "";
}

function switchWorkspace(id) {
  const next = locateWorkspace(id);
  if (!next || next.id === state.activeWorkspaceId) {
    render();
    return;
  }
  syncWorkspace();
  enterWorkspace(next);
  cookieDomainFilter = null;
  render();
  persist();
  refreshCookiesIfVisible();
}

const confirmDialog = $("#confirm-dialog");

function confirmDialogShow({ title, message, action = "Delete" }) {
  $("#confirm-title").textContent = title;
  $("#confirm-message").textContent = message;
  $("#confirm-ok").textContent = action;
  if (confirmDialog.open) return Promise.resolve(false);
  confirmDialog.returnValue = "cancel";
  confirmDialog.showModal();
  return new Promise((resolve) => {
    confirmDialog.addEventListener("close", () => resolve(confirmDialog.returnValue === "confirm"), { once: true });
  });
}

// Removing an environment also closes its tab(s) in every workspace and clears
// any binding to it.
async function deleteEnvironment(id) {
  const env = locateEnvironment(id);
  if (!env || !await confirmDeletion("environment", env.name)) return;
  state.environments = state.environments.filter((e) => e.id !== id);
  state.tabs = state.tabs.filter((t) => t.kind !== "environment" || t.envId !== id);
  for (const ws of state.workspaces) {
    for (const collection of ws.collections) {
      if (collection.environmentId === id) collection.environmentId = null;
    }
    if (ws !== activeWorkspace()) {
      ws.openTabs = (ws.openTabs ?? []).filter((t) => t.kind !== "environment" || t.envId !== id);
      if (ws.openTabId && !ws.openTabs.some((t) => t.id === ws.openTabId)) ws.openTabId = ws.openTabs.at(-1)?.id ?? null;
    }
  }
  if (state.selected?.kind === "environment" && state.selected.id === id) state.selected = null;
  if (!state.tabs.some((t) => t.id === state.activeTabId)) return selectTab(state.tabs.at(-1)?.id ?? null);
  persist();
  render();
}

async function deleteWorkspace(id) {
  const ws = locateWorkspace(id);
  if (!ws) return;
  const confirmed = await confirmDialogShow({
    title: "Delete workspace",
    message: `“${ws.name}” will be removed with its collections, environments, and open tabs. This cannot be undone.`,
  });
  if (!confirmed) return;
  // The jar is keyed by workspace id, so deleting the workspace must delete its cookies too.
  try { await invoke("cookie_purge", { workspaceId: id }); }
  catch (err) { notify(`Cookies for the deleted workspace: ${String(err)}`); }
  state.workspaces = state.workspaces.filter((w) => w.id !== id);
  if (!state.workspaces.length) state.workspaces.push(newWorkspace());
  if (!locateWorkspace(state.activeWorkspaceId)) enterWorkspace(state.workspaces[0]);
  persist();
  render();
}

function confirmDeletion(kind, name) {
  if (!state.settings.general.confirmBeforeDelete) return Promise.resolve(true);
  return confirmDialogShow({ title: `Delete ${kind}`, message: `Remove “${name}”? This cannot be undone.` });
}

function addTab(req = newRequest()) {
  const tab = { id: uid(), req, response: null };
  state.tabs.push(tab);
  state.activeTabId = tab.id;
  state.selected = null;
  render();
  persist();
  return tab;
}

// Activates a tab and syncs the sidebar selection with it: an environment tab
// selects its environment, a request tab clears any stale selection.
function selectTab(id) {
  state.activeTabId = id;
  const tab = state.tabs.find((t) => t.id === id);
  state.selected = tab?.kind === "environment" ? { id: tab.envId, kind: "environment" } : null;
  render();
  persist();
}

// Environments live in the tab strip next to requests, so editing variables
// never pulls the user away from an open request tab.
function openEnvTab(envId) {
  let tab = state.tabs.find((t) => t.kind === "environment" && t.envId === envId);
  if (!tab) {
    tab = { id: uid(), kind: "environment", envId };
    state.tabs.push(tab);
  }
  selectTab(tab.id);
  return tab;
}

/* Ctrl+PgUp / Ctrl+PgDown: move to the previous/next open request tab (wraps around) */
function cycleTab(offset) {
  if (state.tabs.length < 2) return;
  const index = state.tabs.findIndex((t) => t.id === state.activeTabId);
  const next = state.tabs[(index + offset + state.tabs.length) % state.tabs.length];
  selectTab(next.id);
}

// A tab is dirty when its request was never saved to a collection, or when it
// differs from the stored copy. Environment tabs save in place, so never dirty.
function tabDirty(tab) {
  if (tab.kind === "environment") return false;
  const stored = locate(tab.req.id);
  return !stored || JSON.stringify(stored) !== JSON.stringify(tab.req);
}

// Only tabs with unsaved changes ask for confirmation; closing a saved request
// or an environment tab is instant.
async function confirmCloseTabs(tabs) {
  const dirty = tabs.filter((tab) => tabDirty(tab));
  if (!dirty.length || !state.settings.general.confirmBeforeDelete) return true;
  const message = dirty.length === 1
    ? `“${dirty[0].req.name}” has changes not saved to a collection. Close it? This cannot be undone.`
    : `${dirty.length} of these tabs have changes not saved to a collection. Close them? This cannot be undone.`;
  return confirmDialogShow({ title: "Close tabs", message, action: "Close tabs" });
}

async function closeTab(id) {
  const tab = state.tabs.find((t) => t.id === id);
  if (!tab || !await confirmCloseTabs([tab])) return;
  state.tabs = state.tabs.filter((t) => t.id !== id);
  if (state.activeTabId === id) return selectTab(state.tabs.at(-1)?.id ?? null);
  render();
  persist();
}

async function closeOtherTabs(id) {
  if (!await confirmCloseTabs(state.tabs.filter((t) => t.id !== id))) return;
  state.tabs = state.tabs.filter((t) => t.id === id);
  state.activeTabId = id;
  state.selected = null;
  render();
  persist();
}

async function closeAllTabs() {
  if (!await confirmCloseTabs(state.tabs)) return;
  state.tabs = [];
  state.activeTabId = null;
  render();
  persist();
}

function findItem(items, id) {
  for (const item of items) {
    if (item.id === id) return item;
    if (item.kind === "folder") {
      const nested = findItem(item.items, id);
      if (nested) return nested;
    }
  }
  return null;
}

function locate(id) {
  for (const collection of state.collections) {
    const found = findItem(collection.items, id);
    if (found) return found;
  }
  return null;
}

const locateCollection = (id) => state.collections.find((c) => c.id === id) ?? null;
const locateEnvironment = (id) => state.environments.find((e) => e.id === id) ?? null;

const nodeFor = ({ kind, id }) =>
  !id ? null : kind === "collection" ? locateCollection(id) : kind === "environment" ? locateEnvironment(id) : locate(id);

function containerItems(id) {
  const collection = state.collections.find((c) => c.id === id);
  if (collection) return collection.items;
  const folder = state.collections.map((c) => findItem(c.items, id)).find((f) => f?.kind === "folder");
  return folder ? folder.items : null;
}

function expandPathTo(id) {
  const trail = [];
  const walk = (items, path) => {
    for (const item of items) {
      if (item.id === id) {
        trail.push(...path);
        return true;
      }
      if (item.kind === "folder" && walk(item.items, [...path, item.id])) return true;
    }
    return false;
  };
  for (const collection of state.collections) {
    if (collection.id === id || walk(collection.items, [collection.id])) break;
  }
  for (const nodeId of trail) state.expanded.add(nodeId);
  state.expanded.add(id);
}

function removeFromTree(items, id) {
  const index = items.findIndex((item) => item.id === id);
  if (index >= 0) {
    items.splice(index, 1);
    return true;
  }
  return items.some((item) => item.kind === "folder" && removeFromTree(item.items, id));
}

function containerOptions() {
  const options = [];
  const walk = (items, prefix) => {
    for (const item of items) {
      if (item.kind !== "folder") continue;
      options.push({ id: item.id, label: `${prefix}${item.name}` });
      walk(item.items, `${prefix}${item.name} / `);
    }
  };
  for (const collection of state.collections) {
    options.push({ id: collection.id, label: collection.name });
    walk(collection.items, `${collection.name} / `);
  }
  return options;
}

function homeOf(requestId) {
  for (const collection of state.collections) {
    if (collection.items.some((item) => item.id === requestId)) return collection.id;
    const stack = [...collection.items];
    while (stack.length) {
      const item = stack.pop();
      if (item.kind !== "folder") continue;
      if (item.items.some((child) => child.id === requestId)) return item.id;
      stack.push(...item.items);
    }
  }
  return null;
}

function saveToCollection(name, containerId) {
  const req = activeReq();
  if (!req) return;
  const target = containerItems(containerId) ?? containerItems(state.collections[0]?.id);
  if (!target) return;
  req.name = name;
  for (const collection of state.collections) removeFromTree(collection.items, req.id);
  target.push(structuredClone(req));
  expandPathTo(containerItems(containerId) ? containerId : state.collections[0].id);
  persist();
  render();
}

/* ---------------------------------------------------------------------- import */

function fromPostmanAuth(raw) {
  if (!raw || !raw.type) return null;
  if (raw.type === "noauth") return normAuth(null, "noauth");
  const list = (name) => (Array.isArray(raw[name]) ? raw[name] : []);
  const field = (name, key) => list(name).find((entry) => entry.key === key)?.value ?? "";
  if (raw.type === "bearer") return normAuth({ type: "bearer", bearer: { token: field("bearer", "token") } }, "noauth");
  if (raw.type === "basic")
    return normAuth(
      { type: "basic", basic: { username: field("basic", "username"), password: field("basic", "password") } },
      "noauth"
    );
  if (raw.type === "apikey")
    return normAuth(
      {
        type: "apikey",
        apikey: {
          key: field("apikey", "key"),
          value: field("apikey", "value"),
          addTo: field("apikey", "in") === "query" ? "query" : "header",
        },
      },
      "noauth"
    );
  return null;
}

function fromPostmanEvents(events) {
  const read = (listen) => (events ?? []).find((event) => event.listen === listen)?.script?.exec?.join("\n") ?? "";
  return { pre: read("prerequest"), test: read("test") };
}

const postmanVars = (rows) =>
  (rows ?? []).map((row) => ({ key: row.key ?? "", value: String(row.value ?? ""), enabled: row.enabled !== false && !row.disabled, secret: row.secret === true || row.type === "secret" }));

function importPostmanItems(items) {
  const out = [];
  for (const item of items ?? []) {
    if (Array.isArray(item.item)) {
      out.push(
        newFolder({
          name: item.name ?? "Folder",
          items: importPostmanItems(item.item),
          auth: fromPostmanAuth(item.auth) ?? normAuth(null, "inherit"),
          variables: postmanVars(item.variable),
          scripts: fromPostmanEvents(item.event),
        })
      );
    } else if (item.request) {
      out.push(mapPostmanItem(item));
    }
  }
  return out;
}

function fromPostman(doc) {
  return newCollection(doc?.info?.name ?? "Imported collection", {
    items: importPostmanItems(doc.item),
    auth: fromPostmanAuth(doc.auth),
    variables: postmanVars(doc.variable),
    scripts: fromPostmanEvents(doc.event),
  });
}

// Postman keeps events on the item while the payload itself sits under item.request.
function mapPostmanItem(item) {
  const raw = item.request;
  const req = typeof raw === "string" ? { method: "GET", url: raw } : raw;
  const url = typeof req.url === "string" ? { raw: req.url } : (req.url ?? {});
  const { base, params: fromRaw } = splitUrl(url.raw ?? "");
  const params = (url.query ?? []).map((query) => ({
    key: query.key ?? "",
    value: String(query.value ?? ""),
    enabled: !query.disabled,
  }));
  const headers = (req.header ?? [])
    .filter((h) => h.key)
    .map((h) => ({ key: h.key, value: h.value ?? "", enabled: !h.disabled }));

  const body = { mode: "none", raw: "", contentType: "", form: [] };
  if (req.body?.mode === "raw" && typeof req.body.raw === "string") {
    body.mode = "raw";
    body.raw = req.body.raw;
    const language = req.body.options?.raw?.language;
    body.contentType = req.body.options?.raw?.contentType
      ?? headers.find((h) => h.enabled && h.key.toLowerCase() === "content-type")?.value
      ?? ({ json: "application/json", xml: "application/xml", html: "text/html", text: "text/plain", javascript: "application/javascript" }[language] ?? "");
  } else if (req.body?.mode === "formdata") {
    body.mode = "form-data";
    body.form = (req.body.formdata ?? []).map((field) => ({
      key: field.key ?? "",
      value: String(field.value ?? ""),
      enabled: !field.disabled,
    }));
  } else if (req.body?.mode === "urlencoded") {
    body.mode = "raw";
    body.raw = (req.body.urlencoded ?? [])
      .map((field) => `${encodeURIComponent(field.key ?? "")}=${encodeURIComponent(field.value ?? "")}`)
      .join("&");
    body.contentType = "application/x-www-form-urlencoded";
  }

  const contentType = req.body?.options?.raw?.contentType
    ?? (body.contentType || headers.find((h) => h.key.toLowerCase() === "content-type")?.value || "application/json");

  return newRequest({
    name: item.name ?? "Request",
    method: (req.method ?? "GET").toUpperCase(),
    url: base,
    params: Array.isArray(url.query) ? params : fromRaw,
    headers: Array.isArray(req.header) ? headers : undefined,
    body: { ...body, contentType },
    auth: fromPostmanAuth(req.auth),
    scripts: fromPostmanEvents(item.event),
  });
}

// Postman environment exports keep rows under `values`; `type: "secret"` marks a hidden one.
function fromPostmanEnvironment(doc) {
  const rows = doc?.values ?? doc?.variables;
  if (!Array.isArray(rows)) throw new Error("not a Postman environment export");
  return newEnvironment({
    name: doc.name ?? "Imported environment",
    variables: rows.map((row) => ({
      key: row.key ?? "",
      value: String(row.value ?? ""),
      enabled: row.enabled !== false && !row.disabled,
      secret: row.secret === true || row.type === "secret",
    })),
  });
}

function toPostmanEnvironment(env) {
  return {
    id: env.id,
    name: env.name,
    values: env.variables.map((row) => ({
      key: row.key,
      value: row.value,
      type: row.secret ? "secret" : "default",
      enabled: row.enabled !== false,
    })),
    _postman_id: env.id,
    _postman_exported_using: "postlite",
  };
}

function toPostmanAuth(auth) {
  if (auth.type === "inherit") return undefined;
  if (auth.type === "noauth") return { type: "noauth" };
  const fields = auth.type === "apikey"
    ? { key: auth.apikey.key, value: auth.apikey.value, in: auth.apikey.addTo } : auth[auth.type];
  return { type: auth.type, [auth.type]: Object.entries(fields).map(([key, value]) => ({ key, value, type: "string" })) };
}

function toPostmanEvents(scripts) {
  return [["prerequest", scripts.pre], ["test", scripts.test]].filter(([, code]) => code)
    .map(([listen, code]) => ({ listen, script: { type: "text/javascript", exec: code.split("\n") } }));
}

function toPostmanVariables(rows) {
  return rows.map((row) => ({ key: row.key, value: row.value, type: "string", disabled: !row.enabled, secret: row.secret === true }));
}

function toPostmanItem(node) {
  const common = { name: node.name, event: toPostmanEvents(node.scripts) };
  if (node.kind === "folder") return { ...common, auth: toPostmanAuth(node.auth), variable: toPostmanVariables(node.variables), item: node.items.map(toPostmanItem) };
  const request = {
    method: node.method,
    url: { raw: joinQuery(node.url, node.params.filter((row) => row.enabled)), query: node.params.map((row) => ({ key: row.key, value: row.value, disabled: !row.enabled })) },
    header: node.headers.map((row) => ({ key: row.key, value: row.value, disabled: !row.enabled })),
    auth: toPostmanAuth(node.auth),
  };
  if (node.body.mode === "raw") {
    if (node.body.contentType && !node.headers.some((row) => row.enabled && row.key.toLowerCase() === "content-type")) {
      request.header.push({ key: "Content-Type", value: node.body.contentType, disabled: false });
    }
    request.body = { mode: "raw", raw: node.body.raw, options: { raw: {
      language: ({ "application/json": "json", "application/xml": "xml", "text/html": "html", "application/javascript": "javascript" })[node.body.contentType] ?? "text",
      contentType: node.body.contentType,
    } } };
  } else if (node.body.mode === "form-data") {
    request.body = { mode: "formdata", formdata: node.body.form.map((row) => ({ key: row.key, value: row.value, type: "text", disabled: !row.enabled })) };
  }
  return { ...common, request };
}

function toPostmanCollection(collection) {
  const variables = [...collection.variables, ...(envForCollection(collection)?.variables ?? []).filter((row) => row.enabled !== false)];
  return {
    info: { name: collection.name, schema: "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
    auth: toPostmanAuth(collection.auth), variable: toPostmanVariables(variables),
    event: toPostmanEvents(collection.scripts), item: collection.items.map(toPostmanItem),
  };
}

function downloadJson(file, data) {
  const url = URL.createObjectURL(
    new Blob([JSON.stringify(data, null, 2)], { type: "application/json" })
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = file;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
  persist();
}

function exportEnvironment(env) {
  downloadJson(`${String(env.name || "environment").replace(/[^\w.-]+/g, "_")}.postman_environment.json`, toPostmanEnvironment(env));
}

async function exportCollection(collection) {
  const confirmed = await confirmDialogShow({ title: "Export collection", message: "This file includes auth tokens, secret variables, and bound environment values unchanged. Store it securely and do not commit it to source control.", action: "Export" });
  if (confirmed) downloadJson(`${String(collection.name || "collection").replace(/[^\w.-]+/g, "_")}.postman_collection.json`, toPostmanCollection(collection));
}

function exportSettings() {
  downloadJson("postlite.settings.json", { kind: "postlite.settings", version: 1, settings: normalizeSettings(state.settings) });
}

function importSettings(doc) {
  if (doc?.kind !== "postlite.settings" || doc.version !== 1 || !doc.settings || typeof doc.settings !== "object" || Array.isArray(doc.settings)) {
    throw new Error("Not a supported postlite settings file.");
  }
  state.settings = normalizeSettings(doc.settings);
  applyAppSettings();
  persist();
}

/* -------------------------------------------------------------- text interop */

let noteTimer = null;
function notify(message) {
  clearTimeout(noteTimer);
  $("#tool-note").textContent = message;
  noteTimer = setTimeout(() => { $("#tool-note").textContent = ""; }, 8000);
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.tabIndex = -1;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    const copied = document.execCommand("copy");
    area.remove();
    return copied;
  }
}

// Exporters need a request with a URL; the reason is surfaced instead of
// copying an empty command.
function exportSource() {
  const req = activeReq();
  if (!req) return { error: "Open a request first." };
  if (!req.url.trim()) return { error: "Enter a URL before exporting." };
  return { request: exportTarget(req) };
}

const curlDialog = $("#curl-dialog");

/* Right-side drawer: one container, two contents (Code / Cookies). Non-modal so
   the response stays readable; opening the other content swaps in place. */
const drawer = $("#drawer");
let drawerOpener = null;
let drawerPane = null;

function openDrawer(which, opener) {
  const swap = drawer.open;
  drawerPane = which;
  if (opener) drawerOpener = opener;
  $("#drawer-title").textContent = which === "code" ? "Code" : "Cookies";
  for (const pane of $$("#drawer .drawer-pane")) pane.hidden = pane.dataset.drawerPane !== which;
  for (const link of $$(".drawer-link")) {
    const active = link.dataset.drawer === which;
    link.classList.toggle("is-active", active);
    link.setAttribute("aria-expanded", String(active));
  }
  if (which === "code") renderCodePane();
  if (which === "cookies") void loadCookies();
  if (!swap) {
    drawer.show();
    $(".main").classList.add("drawer-open");
  }
  const focusTarget = which === "code"
    ? ($("#code-target").disabled ? $("#drawer-close") : $("#code-target"))
    : $("#drawer-close");
  focusTarget.focus();
}

function closeDrawer() {
  if (!drawer.open) return;
  drawer.close();
  $(".main").classList.remove("drawer-open");
  for (const link of $$(".drawer-link")) {
    link.classList.remove("is-active");
    link.setAttribute("aria-expanded", "false");
  }
  drawerPane = null;
  drawerOpener?.focus();
  drawerOpener = null;
}

function renderCodePane() {
  const select = $("#code-target");
  if (!select.options.length) {
    select.insertAdjacentHTML("beforeend",
      postliteText.CODE_TARGETS.map((t) => `<option value="${t.id}">${t.label}</option>`).join(""));
  }
  renderCodeOutput();
}

function renderCodeOutput() {
  const output = $("#code-output");
  const note = $("#code-note");
  const { request, error } = exportSource();
  $("#code-copy").disabled = Boolean(error);
  $("#code-export").disabled = Boolean(error);
  $("#code-target").disabled = Boolean(error);
  if (error) {
    output.textContent = error;
    note.textContent = "A snippet needs the current request to have a URL.";
    return;
  }
  let code;
  try {
    code = postliteText.toCode(request, $("#code-target").value);
  } catch (err) {
    output.textContent = String(err.message ?? err);
    note.textContent = "";
    return;
  }
  output.textContent = code;
  // The emitters carry their own dependency line, so the note cannot drift.
  const required = /(?:\/\/|#) Requires: (.+)/.exec(code);
  note.textContent = `${required ? `Needs ${required[1]}.` : "No third-party package needed."} Variables are resolved in the output.`;
}

const SNIPPET_EXT = {
  curl: "sh", "js-fetch": "js", "js-axios": "js", "php-curl": "php",
  "php-guzzle": "php", "py-requests": "py", "py-urllib": "py", "go-http": "go",
};

function downloadText(file, text, mime) {
  const url = URL.createObjectURL(new Blob([text], { type: mime }));
  const link = document.createElement("a");
  link.href = url;
  link.download = file;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
}

function exportSnippet() {
  const code = $("#code-output").textContent;
  if (!code || $("#code-export").disabled) return;
  const target = $("#code-target").value;
  downloadText(`postlite-request.${SNIPPET_EXT[target] ?? "txt"}`, code, "text/plain;charset=utf-8");
  notify("Snippet exported.");
}

async function importCurlCommand() {
  const status = $("#curl-status");
  const text = $("#curl-input").value.trim();
  if (!text) {
    status.textContent = "Paste a cURL command first.";
    $("#curl-input").focus();
    return;
  }
  let parsed;
  try {
    parsed = postliteText.parseCurl(text);
  } catch (err) {
    status.textContent = String(err.message ?? err);
    return;
  }
  const { base, params } = splitUrl(parsed.url);
  addTab(newRequest({
    method: parsed.method,
    url: base,
    params,
    headers: parsed.headers,
    body: parsed.form.length
      ? { mode: "form-data", form: parsed.form }
      : { mode: parsed.body ? "raw" : "none", raw: parsed.body, contentType: parsed.contentType },
    auth: normAuth(parsed.auth, "inherit"),
  }));
  const skipped = [...parsed.ignored, ...parsed.notes];
  curlDialog.close();
  notify(skipped.length
    ? `cURL imported into a new tab. Not carried over: ${skipped.join(", ")}`
    : "cURL imported into a new tab.");
}

function openCurlDialog() {
  $("#curl-input").value = "";
  $("#curl-status").textContent = "";
  curlDialog.showModal();
  $("#curl-input").focus();
}

/* ---------------------------------------------------------- cookie manager */
/* The drawer's Cookies pane. Values arrive real from the backend and are masked
   only in the view (§10): type=password + a per-row eye, copy always real bytes. */
let cookieSnapshot = { rows: [], note: null, persisting: true };
let cookieDomainFilter = null;

const cookieMasked = () => state.settings.privacy.maskSensitiveHeaders;

function cookieExpiryLabel(row) {
  if (row.session || row.expiresMs == null) return "sesi";
  return new Date(row.expiresMs).toLocaleString(undefined, {
    day: "2-digit", month: "short", hour: "2-digit", minute: "2-digit",
  });
}

function cookieRowMarkup(row) {
  const chips = [
    row.secure ? '<span class="chip chip-secure">Secure</span>' : "",
    row.httpOnly ? '<span class="chip chip-httponly">HttpOnly</span>' : "",
    row.sameSite ? `<span class="chip chip-samesite">${esc(row.sameSite)}</span>` : "",
  ].join("");
  const masked = cookieMasked();
  return `
    <div class="cookie-row" data-domain="${esc(row.domain)}" data-path="${esc(row.path)}" data-name="${esc(row.name)}">
      <div class="cookie-row-top">
        <span class="cookie-name" title="${esc(row.name)}">${esc(row.name)}</span>
        <span class="cookie-chips">${chips}</span>
        <button class="icon-btn cookie-del" type="button" title="Delete this cookie" aria-label="Delete cookie ${esc(row.name)}">
          <svg class="ic" aria-hidden="true"><use href="#i-trash" /></svg>
        </button>
      </div>
      <div class="cookie-row-value">
        <input class="cookie-value" type="${masked ? "password" : "text"}" value="${esc(row.value)}" readonly spellcheck="false" autocomplete="off" aria-label="Value of cookie ${esc(row.name)}" />
        <button class="icon-btn cookie-eye" type="button" aria-pressed="false" title="${masked ? "Reveal value" : "Hide value"}" aria-label="Toggle value visibility for ${esc(row.name)}">
          <svg class="ic" aria-hidden="true"><use href="#${masked ? "i-eye" : "i-eye-off"}" /></svg>
        </button>
        <button class="icon-btn cookie-copy" type="button" title="Copy the real value" aria-label="Copy value of cookie ${esc(row.name)}">
          <svg class="ic" aria-hidden="true"><use href="#i-copy" /></svg>
        </button>
      </div>
      <div class="cookie-row-meta">
        <span class="cookie-path" title="Path ${esc(row.path)}">${esc(row.path)}</span>
        <span class="cookie-expiry ${row.session ? "is-session" : ""}" title="${row.session ? "Session cookie: cleared when the app closes" : `Expires ${new Date(row.expiresMs).toISOString()}`}">${esc(cookieExpiryLabel(row))}</span>
      </div>
    </div>`;
}

function renderCookies() {
  const rows = cookieSnapshot.rows;
  const filtered = cookieDomainFilter
    ? rows.filter((r) => r.domain === cookieDomainFilter)
    : rows;
  const domains = [...new Set(filtered.map((r) => r.domain))];

  const note = $("#cookie-note");
  if (cookieSnapshot.note) {
    note.textContent = cookieSnapshot.note;
    note.hidden = false;
  } else if (!cookieSnapshot.persisting) {
    note.textContent = "Persist cookies is off: these live only until the app closes.";
    note.hidden = false;
  } else {
    note.hidden = true;
  }

  $("#cookies-sub").textContent = cookieDomainFilter
    ? `${filtered.length} cookie${filtered.length === 1 ? "" : "s"} for ${cookieDomainFilter}`
    : `${rows.length} cookie${rows.length === 1 ? "" : "s"} across ${new Set(rows.map((r) => r.domain)).size} domain${new Set(rows.map((r) => r.domain)).size === 1 ? "" : "s"}`;

  const filterBar = $("#cookie-filter");
  filterBar.hidden = !cookieDomainFilter;
  if (cookieDomainFilter) $("#cookie-filter-domain").textContent = cookieDomainFilter;

  const list = $("#cookie-list");
  list.innerHTML = domains.map((domain) => {
    const groupRows = filtered.filter((r) => r.domain === domain);
    return `
      <section class="cookie-group">
        <button class="cookie-group-head" type="button" data-domain="${esc(domain)}" aria-pressed="${cookieDomainFilter === domain}" title="${cookieDomainFilter === domain ? "Show all domains" : "Filter to this domain"}">
          <svg class="ic" aria-hidden="true"><use href="#i-globe" /></svg>
          <span class="cookie-group-domain">${esc(domain)}</span>
          <span class="count">${groupRows.length}</span>
        </button>
        <div class="cookie-group-rows">${groupRows.map(cookieRowMarkup).join("")}</div>
      </section>`;
  }).join("");

  const empty = $("#cookie-empty");
  empty.hidden = filtered.length > 0;
  if (!filtered.length) {
    empty.querySelector("p").textContent = cookieDomainFilter ? `No cookies for ${cookieDomainFilter}.` : "No cookies yet.";
    empty.querySelector(".drawer-empty-sub").textContent = cookieDomainFilter
      ? "This domain's jar is empty. Open all domains to see the rest."
      : "Send a request to a site that sets cookies and they appear here, grouped by domain.";
  }

  $("#cookie-delete-domain").disabled = !cookieDomainFilter;
  $("#cookie-delete-domain").title = cookieDomainFilter
    ? `Delete every cookie for ${cookieDomainFilter}`
    : "Filter to one domain to delete its cookies";
  $("#cookie-show-all").disabled = !cookieDomainFilter;
  $("#cookie-show-all").title = cookieDomainFilter ? "Show all domains" : "All domains are already shown";
}

async function loadCookies() {
  const id = state.activeWorkspaceId;
  if (!id) {
    cookieSnapshot = { rows: [], note: null, persisting: true };
    renderCookies();
    return;
  }
  try {
    cookieSnapshot = await invoke("cookie_list", { workspaceId: id });
  } catch (err) {
    cookieSnapshot = { rows: [], note: String(err), persisting: true };
  }
  // A filter pointing at a domain that no longer exists would show a dead end.
  if (cookieDomainFilter && !cookieSnapshot.rows.some((r) => r.domain === cookieDomainFilter)) {
    cookieDomainFilter = null;
  }
  renderCookies();
}

function refreshCookiesIfVisible() {
  if (drawer.open && drawerPane === "cookies") void loadCookies();
}

async function deleteCookieFilter(filter, label) {
  const id = state.activeWorkspaceId;
  if (!id) return;
  try {
    const removed = await invoke("cookie_delete", { workspaceId: id, filter });
    notify(removed ? `${label}: ${removed} cookie${removed === 1 ? "" : "s"} deleted.` : `${label}: nothing to delete.`);
  } catch (err) {
    notify(String(err));
  }
  await loadCookies();
}

$("#cookie-list").addEventListener("click", (event) => {
  const head = event.target.closest(".cookie-group-head");
  if (head) {
    const domain = head.dataset.domain;
    cookieDomainFilter = cookieDomainFilter === domain ? null : domain;
    renderCookies();
    return;
  }
  const row = event.target.closest(".cookie-row");
  if (!row) return;
  const { domain, path, name } = row.dataset;
  if (event.target.closest(".cookie-eye")) {
    const input = row.querySelector(".cookie-value");
    const reveal = input.type === "password";
    input.type = reveal ? "text" : "password";
    const eye = row.querySelector(".cookie-eye");
    eye.setAttribute("aria-pressed", String(reveal));
    eye.title = reveal ? "Hide value" : "Reveal value";
    eye.querySelector("use").setAttribute("href", reveal ? "#i-eye-off" : "#i-eye");
    return;
  }
  if (event.target.closest(".cookie-copy")) {
    const value = row.querySelector(".cookie-value").value;
    void copyText(value).then((ok) => notify(ok ? `Cookie "${name}" copied (real value).` : "The clipboard is not available."));
    return;
  }
  if (event.target.closest(".cookie-del")) {
    void deleteCookieFilter({ domain, path, name }, name);
  }
});

$("#cookie-filter-clear").addEventListener("click", () => { cookieDomainFilter = null; renderCookies(); });
$("#cookie-show-all").addEventListener("click", () => { cookieDomainFilter = null; renderCookies(); });
$("#cookie-delete-domain").addEventListener("click", async () => {
  const domain = cookieDomainFilter;
  if (!domain) return;
  const count = cookieSnapshot.rows.filter((r) => r.domain === domain).length;
  const ok = await confirmDialogShow({
    title: "Delete domain cookies",
    message: `Delete ${count} cookie${count === 1 ? "" : "s"} for ${domain} in this workspace? Sites will see you as signed out.`,
    action: "Delete domain",
  });
  if (ok) await deleteCookieFilter({ domain }, domain);
});

/* ------------------------------------------------------------- full data backup */
const backupStamp = () => new Date().toISOString().slice(0, 10);

function describePayload(payload) {
  const workspaces = Array.isArray(payload.workspaces)
    ? payload.workspaces
    : payload.collections || payload.environments || payload.tabs ? [payload] : [];
  const count = (field) => workspaces.reduce((total, ws) => total + (ws[field]?.length ?? 0), 0);
  return {
    workspaces: workspaces.length,
    collections: count("collections"),
    environments: count("environments"),
    tabs: count("tabs"),
    summary: `${workspaces.length} workspace(s), ${count("collections")} collection(s), ${count("environments")} environment(s), ${count("tabs")} open tab(s)`,
  };
}

function validateBackup(doc) {
  if (doc?.kind !== "postlite.backup" || doc.version !== 1 || !doc.payload || typeof doc.payload !== "object" || Array.isArray(doc.payload))
    throw new Error("Not a supported postlite backup file.");
  const described = describePayload(doc.payload);
  // describePayload reports counts, not the list itself.
  if (!described.workspaces) throw new Error("The backup holds no workspace data.");
  return { payload: doc.payload, described, exportedAt: doc.exportedAt ?? "unknown" };
}

async function exportBackup() {
  await flushWorkspace();
  const payload = workspacePayload();
  const described = describePayload(payload);
  const confirmed = await confirmDialogShow({
    title: "Export full backup",
    message: `This file contains everything the app stores (${described.summary}), including auth tokens and secret variables. Store it securely and do not commit it to source control.`,
    action: "Export backup",
  });
  if (!confirmed) return;
  downloadJson(`postlite.backup.${backupStamp()}.json`, {
    kind: "postlite.backup", version: 1, exportedAt: new Date().toISOString(), payload,
  });
  notify(`Backup exported: ${described.summary}.`);
}

// Merge keeps the running preferences and adds the file's workspaces beside the
// current ones; replace is the caller's decision and is confirmed separately.
function applySavedPayload(saved, mode) {
  const workspaces = Array.isArray(saved.workspaces)
    ? saved.workspaces.map(hydrateWorkspace)
    : saved.collections || saved.environments || saved.tabs
      ? [hydrateWorkspace(saved)]
      : [];
  if (!workspaces.length) throw new Error("The backup holds no workspace data.");
  if (mode === "merge") {
    const taken = new Set(state.workspaces.map((ws) => ws.id));
    for (const ws of workspaces) {
      if (taken.has(ws.id)) ws.id = uid();
      taken.add(ws.id);
      state.workspaces.push(ws);
    }
    return workspaces;
  }
  state.workspaces = workspaces;
  state.activeWorkspaceId = workspaces.some((ws) => ws.id === saved.activeWorkspaceId)
    ? saved.activeWorkspaceId
    : workspaces[0].id;
  return workspaces;
}

let pendingRestore = null;
const restoreDialog = $("#restore-dialog");

function openRestoreDialog(file) {
  pendingRestore = null;
  let doc;
  try {
    doc = JSON.parse(file);
  } catch {
    notify("Restore failed: the file is not valid JSON.");
    return;
  }
  try {
    pendingRestore = validateBackup(doc);
  } catch (err) {
    notify(`Restore failed: ${err.message}`);
    return;
  }
  $("#restore-summary").textContent =
    `${pendingRestore.described.summary}, exported ${pendingRestore.exportedAt}.`;
  $("#restore-existing").textContent =
    `Currently open: ${describePayload(workspacePayload()).summary}. Your preferences stay as they are when you merge.`;
  restoreDialog.showModal();
}

async function confirmRestore() {
  if (!pendingRestore) return;
  const mode = restoreDialog.querySelector('input[name="restore-mode"]:checked').value;
  if (mode === "replace") {
    const ok = await confirmDialogShow({
      title: "Replace everything",
      message: `Discard the ${describePayload(workspacePayload()).summary} currently stored and use the backup instead? postlite cannot undo this.`,
      action: "Replace",
    });
    if (!ok) return;
    state.settings = normalizeSettings(pendingRestore.payload.settings);
    applyAppSettings();
  }
  try {
    applySavedPayload(pendingRestore.payload, mode);
  } catch (err) {
    notify(`Restore failed: ${err.message}`);
    return;
  }
  enterWorkspace(activeWorkspace());
  pendingRestore = null;
  restoreDialog.close();
  persist();
  await flushWorkspace();
  notify(mode === "merge"
    ? "Backup merged. The restored workspaces were added beside yours. Cookies are not part of a backup, so they start with an empty jar."
    : "Backup restored. Everything stored in the app was replaced.");
}

/* ---------------------------------------------------------------------- render */

function render() {
  renderRail();
  renderWorkspaceBar();
  renderSidebar();
  renderTabbar();
  renderEnvBar();
  renderRequest();
  renderSettings();
  renderResponse();
  if (drawer.open && drawerPane === "code") renderCodeOutput();
  $("#stat-workspaces").textContent = state.workspaces.length;
  $("#stat-collections").textContent = state.collections.length;
  $("#stat-tabs").textContent = state.tabs.length;
}

const wsMeta = (ws) => {
  const collections = ws.collections.length;
  const environments = ws.environments.length;
  const parts = [`${collections} collection${collections === 1 ? "" : "s"}`];
  if (environments) parts.push(`${environments} env${environments === 1 ? "" : "s"}`);
  return parts.join(" · ");
};

function renderWorkspaceBar() {
  $("#ws-button-label").textContent = activeWorkspace()?.name ?? "No workspace";
  $("#ws-list").innerHTML =
    state.workspaces
      .map((ws) => {
        const active = ws.id === state.activeWorkspaceId;
        return `
        <div class="ws-row ${active ? "is-active" : ""}" data-id="${ws.id}" role="option" aria-selected="${active}" title="${esc(ws.name)}">
          <svg class="ic" aria-hidden="true"><use href="#i-grid" /></svg>
          <span class="ws-row-name">${esc(ws.name)}</span>
          <svg class="ic ws-check" aria-hidden="true"><use href="#i-check" /></svg>
          <span class="ws-row-acts">
            <button class="node-act" data-act="rename" title="Rename workspace" aria-label="Rename ${esc(ws.name)}">${icon("pencil")}</button>
            <button class="node-act" data-act="delete" title="Delete workspace" aria-label="Delete ${esc(ws.name)}">${icon("trash")}</button>
          </span>
          <span class="ws-row-meta">${esc(wsMeta(ws))}</span>
        </div>`;
      })
      .join("") ||
    `<div class="ws-empty">No workspaces yet</div>`;
}

// The collection that owns the active tab's request; null for an unsaved tab
// or an environment tab.
function activeCollection() {
  const tab = activeTab();
  return tab && tab.kind !== "environment" ? parentChain(tab.req.id)[0] ?? null : null;
}

// The topbar picker binds the active collection, so it only appears when the
// open request actually belongs to one.
function renderEnvBar() {
  const bar = $("#env-bar");
  const collection = activeCollection();
  bar.hidden = !collection;
  if (!collection) return;
  const bound = envForCollection(collection);
  $("#env-button-label").textContent = bound ? bound.name : "No environment";
  $("#env-list").innerHTML =
    `<div class="ws-row ${bound ? "" : "is-active"}" data-id="" role="option" aria-selected="${!bound}" title="No environment">
      <svg class="ic env-none" aria-hidden="true"><use href="#i-x" /></svg>
      <span class="ws-row-name">No environment</span>
      <svg class="ic ws-check" aria-hidden="true"><use href="#i-check" /></svg>
      <span class="ws-row-meta">send without environment variables</span>
    </div>` + state.environments.map((env) => envRowMarkup(env, bound?.id === env.id)).join("");
}

const envRowMarkup = (env, bound) => `
  <div class="ws-row ${bound ? "is-active" : ""}" data-id="${env.id}" role="option" aria-selected="${bound}" title="${esc(env.name)}">
    <svg class="ic" aria-hidden="true"><use href="#i-globe" /></svg>
    <span class="ws-row-name">${esc(env.name)}</span>
    <svg class="ic ws-check" aria-hidden="true"><use href="#i-check" /></svg>
    <span class="ws-row-acts">
      <button class="node-act" data-act="export" title="Export Postman environment JSON" aria-label="Export ${esc(env.name)}">${icon("export")}</button>
      <button class="node-act" data-act="rename" title="Rename environment" aria-label="Rename ${esc(env.name)}">${icon("pencil")}</button>
      <button class="node-act" data-act="delete" title="Delete environment" aria-label="Delete ${esc(env.name)}">${icon("trash")}</button>
    </span>
    <span class="ws-row-meta">${esc(envPreview(env))}</span>
  </div>`;

function renderSidebar() {
  const isEnv = state.sidebarView === "environments";
  const envBtns = { "#expand-all": !isEnv, "#collapse-all": !isEnv, "#new-collection": !isEnv, "#new-environment": isEnv, "#env-import-btn": isEnv };
  for (const [sel, shown] of Object.entries(envBtns)) $(sel).hidden = !shown;
  $("#sidebar-title").textContent = isEnv ? "Environments" : "Collections";
  $("#collections").hidden = isEnv;
  $("#environments").hidden = !isEnv;
  $("#tree-search").placeholder = isEnv ? "Filter environments…" : "Filter collections…";
  $("#tree-search-clear").hidden = !state.filter.trim();
  // Only one list is kept in the DOM, so tree queries never see stale rows.
  $(isEnv ? "#collections" : "#environments").innerHTML = "";
  if (isEnv) renderEnvironments();
  else renderCollections();
}

function renderCollections() {
  const host = $("#collections");
  if (!state.collections.length) {
    host.innerHTML = `
      <div class="empty">
        <svg class="ic" aria-hidden="true"><use href="#i-stack" /></svg>
        <b>No collections yet</b>
        <span>Save a request with Ctrl+S, or import a Postman collection from the top bar.</span>
      </div>`;
    return;
  }
  const q = state.filter.trim().toLowerCase();
  if (!q) {
    host.innerHTML = state.collections.map((c) => nodeMarkup(c, 0, c.id, "collection")).join("");
    return;
  }
  const html = state.collections.map((c) => filteredMarkup(c, 0, c.id, "collection", q)).join("");
  host.innerHTML =
    html ||
    `<div class="empty">
      <svg class="ic" aria-hidden="true"><use href="#i-search" /></svg>
      <b>No matches</b>
      <span>Nothing matches “${esc(state.filter.trim())}”. Press Esc or × to clear the filter.</span>
    </div>`;
}

function envPreview(env) {
  const rows = (env.variables ?? []).filter((row) => row.enabled !== false && row.key.trim());
  return rows.length
    ? `${rows
        .slice(0, 2)
        .map((row) => `${esc(row.key)}=${maskVariable(row.key, row.secret) ? SECRET_MASK : esc(row.value)}`)
        .join(" · ")}${rows.length > 2 ? ` · +${rows.length - 2}` : ""}`
    : "no variables";
}

function refreshEnvRow(env) {
  if (state.selected?.kind !== "environment") return;
  const meta = $(`#environments .node[data-id="${env.id}"] > .env-meta`);
  if (meta) meta.textContent = envPreview(env);
}

function envMarkup(env) {
  const selected = state.selected?.kind === "environment" && state.selected.id === env.id;
  return `
    <div class="node group env ${selected ? "is-active" : ""}" data-id="${env.id}" data-kind="environment" style="padding-left:4px">
      <div class="node-head">
        <svg class="ic folder-ic" aria-hidden="true"><use href="#i-globe" /></svg>
        <span class="node-name">${esc(env.name)}</span>
        <span class="node-acts">
          <button class="node-act" data-act="export" title="Export Postman environment JSON" aria-label="Export ${esc(env.name)}">${icon("export")}</button>
          <button class="node-act" data-act="rename" title="Rename" aria-label="Rename ${esc(env.name)}">${icon("pencil")}</button>
          <button class="node-act" data-act="delete" title="Delete" aria-label="Delete ${esc(env.name)}">${icon("trash")}</button>
        </span>
      </div>
      <div class="env-meta">${envPreview(env)}</div>
    </div>`;
}

function renderEnvironments() {
  const host = $("#environments");
  if (!state.environments.length) {
    host.innerHTML = `
      <div class="empty">
        <svg class="ic" aria-hidden="true"><use href="#i-globe" /></svg>
        <b>No environments yet</b>
        <span>Add one with + above, or import a Postman environment JSON.</span>
      </div>`;
    return;
  }
  const q = state.filter.trim().toLowerCase();
  const list = state.environments.filter((env) => !q || env.name.toLowerCase().includes(q));
  host.innerHTML =
    list.map(envMarkup).join("") ||
    `<div class="empty">
      <svg class="ic" aria-hidden="true"><use href="#i-search" /></svg>
      <b>No matches</b>
      <span>Nothing matches “${esc(state.filter.trim())}”. Press Esc or × to clear the filter.</span>
    </div>`;
}

function matchTree(node, q) {
  return node.name.toLowerCase().includes(q) || (node.items ?? []).some((child) => matchTree(child, q));
}

function filteredMarkup(node, depth, cid, kind, q) {
  if (kind === "request") return node.name.toLowerCase().includes(q) ? nodeMarkup(node, depth, cid, kind) : "";
  if (!matchTree(node, q)) return "";
  if (node.name.toLowerCase().includes(q)) return nodeMarkup(node, depth, cid, kind, { open: true });
  const children = (node.items ?? []).map((child) => filteredMarkup(child, depth + 1, cid, child.kind, q)).join("");
  return children ? nodeMarkup(node, depth, cid, kind, { open: true, children }) : "";
}

const icon = (name) => `<svg class="ic" aria-hidden="true"><use href="#i-${name}" /></svg>`;

function nodeMarkup(node, depth, cid, kind = node.kind, override = null) {
  const collapsed = override ? !override.open : !state.expanded.has(node.id);
  const children = node.items ?? [];
  const active = activeReq();
  const selected = state.selected?.id === node.id;
  if (kind === "request") {
    return `
      <div class="node request ${active && active.id === node.id ? "is-active" : ""}"
           data-id="${node.id}" data-cid="${cid}" data-kind="request" style="padding-left:${4 + depth * 10}px">
        <span class="method ${esc(node.method)}">${esc(node.method)}</span>
        <span class="node-name">${esc(node.name)}</span>
        <button class="node-act" data-act="delete" title="Delete" aria-label="Delete ${esc(node.name)}">${icon("x")}</button>
      </div>`;
  }
  return `
    <div class="node group ${collapsed ? "collapsed" : ""} ${selected ? "is-active" : ""}" data-id="${node.id}"
         data-cid="${cid}" data-kind="${kind}" style="--d:${depth};padding-left:${4 + depth * 10}px">
      <div class="node-head">
        <button type="button" class="caret" aria-expanded="${!collapsed}" aria-label="${collapsed ? "Expand" : "Collapse"} ${esc(node.name)}">${icon("caret")}</button>
        <svg class="ic folder-ic" aria-hidden="true"><use href="#i-${kind === "collection" ? "stack" : "folder"}" /></svg>
        <span class="node-name">${esc(node.name)}</span>
        <span class="node-acts">
          ${kind === "collection" ? `<button class="node-act" data-act="export" title="Export collection (includes secrets)" aria-label="Export collection, includes secrets">${icon("export")}</button>` : ""}
          <button class="node-act" data-act="new-request" title="New request" aria-label="New request">${icon("file-plus")}</button>
          <button class="node-act" data-act="new-folder" title="New folder" aria-label="New folder">${icon("folder-plus")}</button>
          <button class="node-act" data-act="rename" title="Rename" aria-label="Rename">${icon("pencil")}</button>
          <button class="node-act" data-act="delete" title="Delete" aria-label="Delete">${icon("trash")}</button>
        </span>
      </div>
      <div class="node-body">${override?.children ?? children.map((child) => nodeMarkup(child, depth + 1, cid)).join("")}</div>
    </div>`;
}

function renderTabbar() {
  $("#tabbar").innerHTML = state.tabs
    .map((tab) => {
      if (tab.kind === "environment") {
        const env = locateEnvironment(tab.envId);
        return `
      <div class="tab-item ${tab.id === state.activeTabId ? "is-active" : ""}" data-id="${tab.id}" title="${esc(env?.name ?? "Environment")}">
        <svg class="ic folder-ic" aria-hidden="true"><use href="#i-globe" /></svg>
        <span class="tab-name">${esc(env?.name ?? "Environment")}</span>
        <button class="close" data-act="close-tab" title="Close" aria-label="Close tab">${icon("x")}</button>
      </div>`;
      }
      return `
      <div class="tab-item ${tab.id === state.activeTabId ? "is-active" : ""}" data-id="${tab.id}" title="${esc(tab.req.name)}">
        <span class="method ${esc(tab.req.method)}">${esc(tab.req.method)}</span>
        <span class="tab-name">${esc(tab.req.name)}</span>
        ${tabDirty(tab) ? `<span class="tab-dirty" title="Unsaved changes"></span>` : ""}
        <button class="close" data-act="close-tab" title="Close" aria-label="Close tab">${icon("x")}</button>
      </div>`;
    })
    .join("");
  $("#tabbar").insertAdjacentHTML("beforeend", `<button class="icon-btn" data-act="new-tab" title="New request (Ctrl+N)" aria-label="New request">${icon("plus")}</button>`);
  $("#tabbar .tab-item.is-active")?.scrollIntoView({ inline: "nearest", block: "nearest" });
}

function renderRequest() {
  const req = activeReq();
  const pane = $("#req-pane");
  renderVarPeek();
  if (!req) {
    pane.style.visibility = "hidden";
    return;
  }
  pane.style.visibility = "visible";
  $("#method").value = req.method;
  $("#method").dataset.method = req.method;
  $("#url").value = req.url;
  renderRows($("#params-table").tBodies[0], req.params);
  renderRows($("#headers-table").tBodies[0], req.headers, { mask: headerMasked });
  $("#params-count").textContent = req.params.filter((p) => p.enabled && p.key).length || "";
  $("#headers-count").textContent = req.headers.filter((h) => h.enabled && h.key).length || "";
  renderBody(req);
  renderAuth($("#req-auth-type"), $("#req-auth-fields"), req.auth, true);
  $("#req-pre").value = req.scripts.pre;
  $("#req-test").value = req.scripts.test;
}

function renderBody(req) {
  $(`input[name="body-mode"][value="${req.body.mode}"]`).checked = true;
  $("#content-type").value = req.body.contentType;
  $("#content-type").disabled = req.body.mode !== "raw";
  $("#body").value = req.body.raw;
  $("#body").disabled = req.body.mode !== "raw";
  $("#form-wrap").classList.toggle("is-visible", req.body.mode === "form-data");
  $("#body").hidden = req.body.mode !== "raw";
  $("#body-empty-hint").hidden = req.body.mode !== "none";
  renderRows($("#form-table").tBodies[0], req.body.form, { check: false });
}

function renderRows(tbody, rows, options = {}) {
  const { check = true, secret = false, mask = null } = options;
  tbody.innerHTML = rows
    .map((row, index) => {
      const masked = (secret && row.secret && state.settings.privacy.maskSecretVariables) || Boolean(mask && mask(row));
      const rowMasked = Boolean(mask && mask(row));
      const valueInput = `<input type="${masked ? "password" : "text"}" data-field="value" value="${esc(row.value)}" spellcheck="false" />`;
      // Only the header grid (mask) gets a reveal eye; the variables grid keeps its secret checkbox.
      const valueCell = mask
        ? `<td><div class="value-wrap">${valueInput}<button class="icon-btn row-eye" type="button" data-act="reveal-row" aria-pressed="false" title="Reveal value" aria-label="Toggle value visibility" ${rowMasked ? "" : "hidden"}>${icon("eye")}</button></div></td>`
        : `<td>${valueInput}</td>`;
      return `
      <tr data-index="${index}">
        ${check ? `<td class="c-check"><input type="checkbox" data-field="enabled" ${row.enabled ? "checked" : ""} /></td>` : ""}
        <td><input type="text" data-field="key" value="${esc(row.key)}" spellcheck="false" /></td>
        ${valueCell}
        ${secret ? `<td class="c-secret"><input type="checkbox" data-field="secret" ${row.secret ? "checked" : ""} aria-label="Secret" title="Mask this value in the UI" /></td>` : ""}
        <td class="c-del"><button class="del" data-act="drop-row" title="Remove" aria-label="Remove row">${icon("x")}</button></td>
      </tr>`;
    })
    .join("");
}

const authOptions = (includeInherit) =>
  (includeInherit ? ["inherit", "noauth", "apikey", "bearer", "basic"] : ["noauth", "apikey", "bearer", "basic"])
    .map((type) => `<option value="${type}">${AUTH_LABELS[type]}</option>`)
    .join("");

// Auth secrets are masked like the header grid; the identifier fields (API-key Key, basic
// Username) stay visible because they are names, not secrets.
const authSecretField = (label, a, k, value) => {
  const masked = state.settings.privacy.maskSensitiveHeaders;
  return `<label class="field"><span>${label}</span><div class="auth-value"><input type="${masked ? "password" : "text"}" data-a="${a}" data-k="${k}" value="${esc(value)}" spellcheck="false" /><button class="icon-btn auth-eye" type="button" data-act="reveal-auth" aria-pressed="false" title="Reveal value" aria-label="Toggle ${label} visibility" ${masked ? "" : "hidden"}>${icon("eye")}</button></div></label>`;
};

function authFieldsMarkup(auth) {
  if (auth.type === "bearer")
    return authSecretField("Token", "bearer", "token", auth.bearer.token);
  if (auth.type === "basic")
    return `
      <label class="field"><span>Username</span><input type="text" data-a="basic" data-k="username" value="${esc(auth.basic.username)}" spellcheck="false" /></label>
      ${authSecretField("Password", "basic", "password", auth.basic.password)}`;
  if (auth.type === "apikey")
    return `
      <label class="field"><span>Key</span><input type="text" data-a="apikey" data-k="key" value="${esc(auth.apikey.key)}" spellcheck="false" /></label>
      ${authSecretField("Value", "apikey", "value", auth.apikey.value)}
      <label class="field"><span>Add to</span>
        <select data-a="apikey" data-k="addTo">
          <option value="header"${auth.apikey.addTo === "header" ? " selected" : ""}>header</option>
          <option value="query"${auth.apikey.addTo === "query" ? " selected" : ""}>query parameter</option>
        </select>
      </label>`;
  if (auth.type === "inherit")
    return `<p class="hint">Uses the auth configured on the parent folder or collection.</p>`;
  return `<p class="hint">Requests at this level are sent without authorization.</p>`;
}

function renderAuth(select, host, auth, includeInherit) {
  select.innerHTML = authOptions(includeInherit);
  select.value = auth.type;
  host.innerHTML = authFieldsMarkup(auth);
}

// A collection/folder shows its settings when selected in the sidebar; an
// environment shows its variables whenever its tab is the active one.
function settingsNode() {
  if (state.selected && state.selected.kind !== "environment") {
    const node = nodeFor(state.selected);
    if (node && node.kind !== "request") return node;
  }
  const tab = activeTab();
  return tab?.kind === "environment" ? (locateEnvironment(tab.envId) ?? null) : null;
}

function renderSettings() {
  const node = settingsNode();
  const isEnv = state.environments.includes(node);
  const kind = isEnv ? "environment" : state.selected?.kind;
  $("#settings-pane").hidden = !node;
  $(".main").classList.toggle("settings-open", Boolean(node));
  if (!node) return;
  // An environment has no auth or scripts, so those tabs fold away in its editor.
  for (const tab of ["node-auth", "node-scripts"]) {
    $(`#settings-tabs .tab[data-tab="${tab}"]`).hidden = isEnv;
  }
  if (isEnv && $("#settings-tabs .tab.is-active")?.dataset.tab !== "node-vars") {
    activateTab("#settings-tabs .tab", "#settings-pane .pane", $("#settings-tabs .tab[data-tab='node-vars']"));
  }
  $("#settings-kind").textContent = kind;
  $("#settings-title").textContent = node.name;
  if (!isEnv) {
    renderAuth($("#node-auth-type"), $("#node-auth-fields"), node.auth, kind === "folder");
    $("#node-pre").value = node.scripts.pre;
    $("#node-test").value = node.scripts.test;
  }
  renderRows($("#vars-table").tBodies[0], node.variables, { secret: true });
  $("#vars-count").textContent = node.variables.filter((v) => v.enabled && v.key).length || "";
}

function highlightJson(text) {
  const re = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g;
  let out = "";
  let last = 0;
  let m;
  while ((m = re.exec(text))) {
    out += esc(text.slice(last, m.index));
    if (m[1]) {
      out += m[2]
        ? `<span class="j-key">${esc(m[1])}</span>${esc(m[2])}`
        : `<span class="j-str">${esc(m[1])}</span>`;
    } else if (m[3]) {
      out += `<span class="${m[3] === "null" ? "j-null" : "j-bool"}">${m[3]}</span>`;
    } else {
      out += `<span class="j-num">${m[4]}</span>`;
    }
    last = re.lastIndex;
  }
  return out + esc(text.slice(last));
}

const sensitiveName = (name) => /^(authorization|proxy-authorization|cookie|set-cookie)$|token|secret|key/i.test(name);
const headerMasked = (row) => state.settings.privacy.maskSensitiveHeaders && sensitiveName(row.key);
const maskVariable = (name, secret) => (state.settings.privacy.maskSecretVariables && secret)
  || (state.settings.privacy.maskSensitiveHeaders && sensitiveName(name));
const shownHeader = (header) => state.settings.privacy.maskSensitiveHeaders && sensitiveName(header.key) ? SECRET_MASK : header.value;

function responseCopyText() {
  const response = activeTab()?.response;
  if (!response) return "";
  const view = $("#res-tabs .tab.is-active")?.dataset.tab;
  // Copy is a path out of the app, so it carries real values even though the table shows masks.
  if (view === "res-headers") return response.headers.map((header) => `${header.key}: ${header.value}`).join("\n");
  if (view === "tests") return $("#tests").textContent;
  return response.body;
}

let lastRenderedResponse;

/* Preview tab: renders an HTML body in a fully sandboxed iframe — no scripts,
   forms, or same-origin access, so untrusted markup stays inert. Rendering is
   lazy (only while the tab is visible) and the iframe only reloads when a
   different response object arrives. */
const previewEmptyDoc = '<!doctype html><body style="font:12px system-ui,sans-serif;color:#888;margin:14px">Preview is available for HTML responses.</body>';

const previewable = (response) =>
  response.headers.some((h) => /(?:^|[\s:;])(?:text\/html|application\/xhtml\+xml)\b/i.test(`${h.key}: ${h.value}`));

let lastPreviewResponse; // response object currently loaded into the preview iframe

function renderPreview() {
  const frame = $("#preview");
  const response = activeTab()?.response;
  if (response === lastPreviewResponse) return;
  lastPreviewResponse = response;
  if (!response || response.error) frame.srcdoc = "";
  else frame.srcdoc = previewable(response) ? response.body : previewEmptyDoc;
}

function renderResponse() {
  const response = activeTab()?.response;
  const pill = $("#status");
  $("#error").textContent = "";
  $("#time").hidden = !state.settings.response.showTime;
  $("#size").hidden = !state.settings.response.showSize;
  $("#response-limit").hidden = !response?.truncated;
  $("#response-limit").textContent = response?.truncated ? `Response truncated, shown up to ${response.limitKB ?? state.settings.response.maxResponseSizeKB} KB.` : "";
  if (response !== lastRenderedResponse) {
    activateTab("#res-tabs .tab", "#pretty, #raw, #preview, #res-headers, .tests", $(`#res-tabs [data-tab="${state.settings.response.defaultView}"]`));
    lastRenderedResponse = response;
    const res = $("#res-pane");
    res.classList.remove("flash");
    void res.offsetWidth;
    res.classList.add("flash");
  }
  if (!response) {
    pill.className = "pill";
    pill.textContent = "No response";
    $("#time").textContent = "";
    $("#size").textContent = "";
    $("#pretty").textContent = "";
    $("#raw").textContent = "";
    $("#res-headers").tBodies[0].innerHTML = "";
    $("#res-headers-count").textContent = "";
    $("#copy-res").hidden = true;
    renderTests([]);
    renderPreview();
    return;
  }
  renderTests(response.results ?? []);
  if (response.error) {
    pill.className = "pill failed";
    pill.textContent = "failed";
    $("#time").textContent = `${response.elapsed_ms} ms`;
    $("#size").textContent = "";
    $("#copy-res").hidden = true;
    $("#pretty").textContent = "";
    $("#raw").textContent = "";
    $("#res-headers").tBodies[0].innerHTML = "";
    $("#res-headers-count").textContent = "";
    showError(response.error);
    renderPreview();
    return;
  }
  $("#copy-res").hidden = false;
  const group =
    response.status >= 500 ? "server" : response.status >= 400 ? "client" : response.status >= 300 ? "redirect" : "ok";
  pill.className = `pill ${group}`;
  pill.textContent = `${response.status} ${response.status_text}`;
  $("#time").textContent = `${response.elapsed_ms} ms`;
  $("#size").textContent = formatSize(response.size_bytes);
  $("#raw").textContent = response.body;
  $("#pretty").textContent = response.body;
  if (state.settings.request.decodeResponse) {
    try {
      const indent = state.settings.response.jsonIndent;
      const text = JSON.stringify(JSON.parse(response.body), null, indent === "tab" ? "\t" : indent);
      if (state.settings.response.syntaxHighlight) $("#pretty").innerHTML = highlightJson(text);
      else $("#pretty").textContent = text;
    } catch {}
  }
  $("#res-headers-count").textContent = response.headers.length;
  $("#res-headers").tBodies[0].innerHTML = response.headers
    .map((h) => `<tr><td>${esc(h.key)}</td><td>${esc(shownHeader(h))}</td></tr>`)
    .join("");
  if ($("#res-tabs .tab.is-active")?.dataset.tab === "preview") renderPreview();
}

function renderTests(results) {
  $("#tests-count").textContent = results.length || "";
  const failed = results.filter((r) => !r.passed).length;
  const summary = results.length
    ? `<li class="summary">${results.length} assertion${results.length === 1 ? "" : "s"}, ${failed} failing</li>`
    : `<li class="summary">No tests ran for this response.</li>`;
  $("#tests").innerHTML =
    summary +
    results
      .map(
        (r) => `
      <li class="${r.passed ? "pass" : "fail"}">
        <span class="mark">${r.passed ? "✓" : "✕"}</span>
        <div>
          <div>${esc(r.name)}</div>
          ${r.message ? `<p class="msg">${esc(r.message)}</p>` : ""}
        </div>
      </li>`
      )
      .join("");
}

function formatSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} kB`;
  return `${(bytes / 1048576).toFixed(2)} MB`;
}

/* ------------------------------------------------------------------------ send */

function prepareRequest(req) {
  const parents = parentChain(req.id);
  const vars = collectVars(parents);
  const auth = resolveAuth(req, parents);
  return {
    parents,
    vars,
    auth,
    prepared: {
      method: req.method,
      url: req.url,
      params: req.params.filter((p) => p.enabled && p.key.trim()).map((p) => ({ key: p.key, value: p.value })),
      headers: enabledHeaders(req),
      body: req.body.mode === "raw" ? req.body.raw : "",
      form:
        req.body.mode === "form-data"
          ? req.body.form.filter((f) => f.enabled !== false && f.key.trim()).map((f) => ({ key: f.key, value: f.value }))
          : [],
    },
  };
}

// Auth headers land after the pre-request scripts so a Content-Type set by a
// script still wins, and every field is interpolated last.
function finalizePrepared(prepared, auth, vars) {
  const userContentType = prepared.headers.some((h) => h.key.toLowerCase() === "content-type");
  for (const header of authHeaders(auth, vars)) {
    if (userContentType && header.key.toLowerCase() === "content-type") continue;
    prepared.headers.push(header);
  }

  prepared.url = interpolate(prepared.url, vars);
  prepared.body = interpolate(prepared.body, vars);
  prepared.params = prepared.params.map((p) => ({ key: interpolate(p.key, vars), value: interpolate(p.value, vars) }));
  prepared.headers = prepared.headers.map((h) => ({ key: interpolate(h.key, vars), value: interpolate(h.value, vars) }));
  prepared.form = prepared.form.map((f) => ({ key: interpolate(f.key, vars), value: interpolate(f.value, vars) }));
  return prepared;
}

const effectiveUrl = (prepared, auth, vars) =>
  joinQuery(prepared.url, [...prepared.params, ...authQuery(auth, vars)]);

// Text exporters resolve variables so the copied command or snippet runs
// unchanged, which means the clipboard can carry a live secret (see Settings,
// Import/Export note).
function exportTarget(req) {
  const { prepared, auth, vars } = prepareRequest(req);
  finalizePrepared(prepared, auth, vars);
  return {
    method: prepared.method,
    url: effectiveUrl(prepared, auth, vars),
    headers: prepared.headers,
    body: prepared.body,
    form: prepared.form,
    sslVerify: state.settings.request.sslVerify,
  };
}

async function send() {
  if ($("#send").disabled) return;
  const tab = activeTab();
  const req = tab?.req;
  if (!req || !req.url.trim()) {
    $("#url").focus();
    return;
  }
  const { parents, vars, auth, prepared } = prepareRequest(req);
  const results = [];

  // ancestors run first so a folder can prepare what its requests consume
  const prePm = () => buildPm({ vars, request: prepared, results });
  for (const node of parents) runScript(node.scripts?.pre, prePm(), node.name, results);
  runScript(req.scripts.pre, prePm(), req.name, results);
  finalizePrepared(prepared, auth, vars);

  const button = $("#send");
  button.disabled = true;
  button.textContent = "Sending";
  let response;
  const settings = state.settings;
  const limitKB = settings.response.maxResponseSizeKB;
  try {
    response = await invoke("http_send", {
      request: {
        method: prepared.method,
        url: effectiveUrl(prepared, auth, vars),
        headers: prepared.headers,
        body: prepared.body,
        form: prepared.form,
        settings: {
          request_timeout_ms: settings.request.requestTimeoutMs, follow_redirects: settings.request.followRedirects,
          max_redirects: settings.request.maxRedirects, ssl_verify: settings.request.sslVerify,
          disable_cache: settings.request.disableCache, max_response_size_kb: limitKB,
          workspace_id: state.activeWorkspaceId ?? "",
          proxy: settings.request.proxy.mode === "direct" ? null
            : { mode: settings.request.proxy.mode, url: settings.request.proxy.url, bypass: settings.request.proxy.bypass },
        },
      },
    });
  } catch (err) {
    response = { error: String(err), elapsed_ms: 0 };
  } finally {
    button.disabled = false;
    button.textContent = "Send";
  }

  if (!response.error) {
    for (const node of [req, ...[...parents].reverse()])
      runScript(node.scripts?.test, buildPm({ vars, response, results }), node.name, results);
  }

  tab.response = { ...response, results, limitKB };
  renderResponse();
  refreshCookiesIfVisible();
}

/* --------------------------------------------------------------------- dialogs */

const saveDialog = $("#save-dialog");
const collectionDialog = $("#collection-dialog");
let renameTarget = null;

function openSaveDialog() {
  const req = activeReq();
  if (!req) return;
  if (!state.collections.length) {
    state.collections.push(newCollection("My Collection"));
    persist();
    renderSidebar();
  }
  $("#save-name").value = req.name;
  const options = containerOptions();
  $("#save-collection").innerHTML = options.map((o) => `<option value="${o.id}">${esc(o.label)}</option>`).join("");
  $("#save-collection").value = homeOf(req.id) ?? options[0].id;
  saveDialog.returnValue = "cancel";
  saveDialog.showModal();
}

const DIALOG_SUB = {
  environment: "Environments hold a set of variables a collection can bind to.",
  workspace: "Workspaces group collections and environments.",
  default: "Collections and folders can hold requests and subfolders.",
};

function openRenameDialog(target) {
  renameTarget = target;
  $("#collection-name").value = target.name;
  $("#collection-dialog .dialog-sub").textContent = DIALOG_SUB[target.kind] ?? DIALOG_SUB.default;
  collectionDialog.returnValue = "cancel";
  collectionDialog.showModal();
}

function openCreateDialog(kind) {
  renameTarget = { kind, id: null };
  $("#collection-name").value = "";
  $("#collection-dialog .dialog-sub").textContent = DIALOG_SUB[kind] ?? DIALOG_SUB.default;
  collectionDialog.returnValue = "cancel";
  collectionDialog.showModal();
}

saveDialog.addEventListener("close", () => {
  if (saveDialog.returnValue !== "save") return;
  saveToCollection($("#save-name").value.trim() || "Untitled Request", $("#save-collection").value);
});

collectionDialog.addEventListener("close", () => {
  if (collectionDialog.returnValue !== "create") return;
  const name = $("#collection-name").value.trim();
  if (!name) return;
  if (renameTarget.id) {
    const target = renameTarget.kind === "workspace" ? locateWorkspace(renameTarget.id) : nodeFor(renameTarget);
    if (target) target.name = name;
  } else if (renameTarget.kind === "workspace") {
    const ws = newWorkspace({ name });
    state.workspaces.push(ws);
    switchWorkspace(ws.id);
    return;
  } else if (renameTarget.kind === "collection") {
    state.collections.push(newCollection(name));
  } else if (renameTarget.kind === "environment") {
    const env = newEnvironment({ name });
    state.environments.push(env);
    openEnvTab(env.id);
    return;
  }
  persist();
  render();
});

/* ---------------------------------------------------------------------- events */

$("#collections").addEventListener("click", async (event) => {
  const action = event.target.closest("[data-act]")?.dataset.act;
  const node = event.target.closest(".node");
  if (!node) return;
  const { id, cid, kind } = node.dataset;

  if (action === "export" && kind === "collection") {
    await exportCollection(locateCollection(id));
    return;
  }
  if (action === "delete") {
    if (!await confirmDeletion(kind, nodeFor({ kind, id })?.name ?? kind)) return;
    if (kind === "collection") state.collections = state.collections.filter((c) => c.id !== id);
    else {
      removeFromTree(containerItems(cid), id);
      if (kind === "folder") state.expanded.delete(id);
    }
    if (state.selected?.id === id) state.selected = null;
    persist();
    render();
    return;
  }

  if (action === "rename") {
    openRenameDialog({ kind, id, name: nodeFor({ kind, id })?.name ?? "" });
    return;
  }

  if (action === "new-folder") {
    const folder = newFolder();
    containerItems(id).push(folder);
    expandPathTo(id);
    persist();
    render();
    openRenameDialog({ kind: "folder", id: folder.id, name: folder.name });
    return;
  }

  if (action === "new-request") {
    const request = newRequest({ name: "New request" });
    containerItems(id).push(request);
    expandPathTo(id);
    persist();
    render();
    addTab(structuredClone(request));
    return;
  }

  if (kind === "request") {
    const stored = locate(id);
    if (!stored) return;
    // Reuse the tab already open for this request instead of spawning a duplicate.
    const existing = state.tabs.find((t) => t.req?.id === stored.id);
    existing ? selectTab(existing.id) : addTab(structuredClone(stored));
    return;
  }

  // the caret folds the node; clicking its name opens the node's settings
  if (event.target.closest(".caret")) {
    state.expanded.has(id) ? state.expanded.delete(id) : state.expanded.add(id);
    renderSidebar();
    return;
  }
  state.selected = { id, kind };
  // Clicking a collection or folder opens it; folding is the caret's job only,
  // so an open node never snaps shut when its name is clicked again.
  state.expanded.add(id);
  render();
});

$("#collections").addEventListener("dblclick", (event) => {
  const head = event.target.closest(".node-head");
  if (!head) return;
  const id = head.closest(".node").dataset.id;
  // The two clicks already toggled expansion once each, leaving it unchanged;
  // double-click's remaining job is to deselect and close the settings pane.
  if (state.selected?.id === id) state.selected = null;
  render();
});

$("#environments").addEventListener("click", (event) => {
  const row = event.target.closest(".env");
  if (!row) return;
  const id = row.dataset.id;
  const env = locateEnvironment(id);
  if (!env) return;
  const action = event.target.closest("[data-act]")?.dataset.act;

  if (action === "delete") {
    deleteEnvironment(id);
    return;
  }
  if (action === "rename") {
    openRenameDialog({ kind: "environment", id, name: env.name });
    return;
  }
  if (action === "export") {
    exportEnvironment(env);
    return;
  }
  openEnvTab(id);
});

/* ----------------------------------------------------------- workspace switcher */

function closeWsPop() {
  $("#ws-pop").hidden = true;
  $("#ws-button").setAttribute("aria-expanded", "false");
}

$("#ws-button").addEventListener("click", () => {
  const pop = $("#ws-pop");
  const opening = pop.hidden;
  pop.hidden = !opening;
  $("#ws-button").setAttribute("aria-expanded", String(opening));
});

$("#ws-list").addEventListener("click", (event) => {
  const action = event.target.closest("[data-act]")?.dataset.act;
  const row = event.target.closest(".ws-row");
  if (!row) return;
  const ws = locateWorkspace(row.dataset.id);
  if (!ws) return;
  if (action === "rename") {
    closeWsPop();
    openRenameDialog({ kind: "workspace", id: ws.id, name: ws.name });
    return;
  }
  if (action === "delete") {
    closeWsPop();
    deleteWorkspace(ws.id);
    return;
  }
  closeWsPop();
  switchWorkspace(ws.id);
});

$("#ws-new").addEventListener("click", () => {
  closeWsPop();
  openCreateDialog("workspace");
});

$("#tree-search").addEventListener("input", (event) => {
  state.filter = event.target.value;
  renderSidebar();
});

$("#tree-search").addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  event.target.value = "";
  state.filter = "";
  renderSidebar();
  event.target.blur();
});

$("#tree-search-clear").addEventListener("click", () => {
  $("#tree-search").value = "";
  state.filter = "";
  renderSidebar();
  $("#tree-search").focus();
});

function allGroupIds() {
  const ids = [];
  const walk = (items) => {
    for (const item of items) {
      if (item.kind !== "folder") continue;
      ids.push(item.id);
      walk(item.items ?? []);
    }
  };
  for (const collection of state.collections) {
    ids.push(collection.id);
    walk(collection.items ?? []);
  }
  return ids;
}

$("#expand-all").addEventListener("click", () => {
  state.expanded = new Set(allGroupIds());
  renderSidebar();
});

$("#collapse-all").addEventListener("click", () => {
  state.expanded = new Set();
  renderSidebar();
});

$("#tabbar").addEventListener("auxclick", (event) => {
  if (event.button !== 1) return;
  const item = event.target.closest(".tab-item");
  if (!item) return;
  event.preventDefault();
  closeTab(item.dataset.id);
});

let copyTimer;
$("#copy-res").addEventListener("click", async () => {
  const button = $("#copy-res");
  const body = responseCopyText();
  let ok = true;
  try {
    await navigator.clipboard.writeText(body);
  } catch {
    try {
      const ta = document.createElement("textarea");
      ta.value = body;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      ok = document.execCommand("copy");
      ta.remove();
    } catch {
      ok = false;
    }
  }
  button.classList.remove("copied", "copy-fail");
  void button.offsetWidth;
  button.classList.add(ok ? "copied" : "copy-fail");
  button.title = ok ? "Copied" : "Copy failed";
  button.querySelector("use").setAttribute("href", ok ? "#i-check" : "#i-x");
  clearTimeout(copyTimer);
  copyTimer = setTimeout(() => {
    button.classList.remove("copied", "copy-fail");
    button.title = "Copy response body or selected headers";
    button.querySelector("use").setAttribute("href", "#i-copy");
  }, 1400);
});

$("#tabbar").addEventListener("click", (event) => {
  if (event.target.closest('[data-act="new-tab"]')) { addTab(); return; }
  const item = event.target.closest(".tab-item");
  if (!item) return;
  if (event.target.closest('[data-act="close-tab"]')) closeTab(item.dataset.id);
  else selectTab(item.dataset.id);
});

/* tab right-click menu: close the other tabs, or all of them */
let menuTabId = null;
function closeTabMenu() {
  $("#tab-menu").hidden = true;
  menuTabId = null;
}
$("#tabbar").addEventListener("contextmenu", (event) => {
  const item = event.target.closest(".tab-item");
  if (!item) return;
  event.preventDefault();
  menuTabId = item.dataset.id;
  const menu = $("#tab-menu");
  menu.hidden = false;
  const pad = 6;
  menu.style.left = `${Math.max(pad, Math.min(event.clientX, window.innerWidth - menu.offsetWidth - pad))}px`;
  menu.style.top = `${Math.max(pad, Math.min(event.clientY, window.innerHeight - menu.offsetHeight - pad))}px`;
});
$("#tab-menu").addEventListener("click", (event) => {
  const action = event.target.closest("[data-menu]")?.dataset.menu;
  if (!action || !menuTabId) return;
  if (action === "close-others") closeOtherTabs(menuTabId);
  else if (action === "close-all") closeAllTabs();
  closeTabMenu();
});

/* drawer entry links: clicking the open content closes it, the other swaps in place */
for (const link of $$(".drawer-link")) {
  link.addEventListener("click", () => {
    if (drawer.open && drawerPane === link.dataset.drawer) closeDrawer();
    else openDrawer(link.dataset.drawer, link);
  });
}
$("#drawer-close").addEventListener("click", closeDrawer);

document.addEventListener("click", (event) => {
  if (!$("#tab-menu").hidden && !event.target.closest("#tab-menu")) closeTabMenu();
  if (!$("#ws-pop").hidden && !event.target.closest("#ws-bar")) closeWsPop();
  if (!$("#env-pop").hidden && !event.target.closest("#env-bar")) closeEnvPop();
});
window.addEventListener("blur", closeTabMenu);
window.addEventListener("blur", closeWsPop);
window.addEventListener("blur", closeEnvPop);

function activateTab(navSel, panelSel, tab) {
  for (const button of $$(navSel)) button.classList.toggle("is-active", button === tab);
  for (const panel of $$(panelSel)) {
    panel.classList.toggle("is-active", panel.dataset.tab === tab.dataset.tab || panel.dataset.pane === tab.dataset.tab);
  }
}

$("#req-tabs").addEventListener("click", (event) => {
  const tab = event.target.closest(".tab");
  if (tab) activateTab("#req-tabs .tab", "#req-pane .pane", tab);
});

$("#settings-tabs").addEventListener("click", (event) => {
  const tab = event.target.closest(".tab");
  if (tab) activateTab("#settings-tabs .tab", "#settings-pane .pane", tab);
});

$("#res-tabs").addEventListener("click", (event) => {
  const tab = event.target.closest(".tab");
  if (!tab) return;
  activateTab("#res-tabs .tab", "#pretty, #raw, #preview, #res-headers, .tests", tab);
  if (tab.dataset.tab === "preview") renderPreview();
});

const rowTarget = (key) => (key === "form" ? activeReq()?.body.form : activeReq()?.[key]);

for (const [sel, key] of [
  ["#params-table", "params"],
  ["#headers-table", "headers"],
  ["#form-table", "form"],
]) {
  $(sel).addEventListener("input", (event) => {
    const rows = rowTarget(key);
    const field = event.target.dataset.field;
    if (!rows || !field) return;
    rows[Number(event.target.closest("tr").dataset.index)][field] =
      event.target.type === "checkbox" ? event.target.checked : event.target.value;
    const counter = { params: "#params-count", headers: "#headers-count" }[key];
    if (counter) $(counter).textContent = rows.filter((row) => row.enabled !== false && row.key).length || "";
    // Renaming a header into/out of a sensitive name re-masks that row in place, without a
    // full re-render that would steal focus from the key being typed.
    if (key === "headers" && field === "key") {
      const tr = event.target.closest("tr");
      const masked = headerMasked(rows[Number(tr.dataset.index)]);
      tr.querySelector('[data-field="value"]').type = masked ? "password" : "text";
      const eye = tr.querySelector('[data-act="reveal-row"]');
      if (eye) {
        eye.hidden = !masked;
        eye.setAttribute("aria-pressed", "false");
        eye.title = "Reveal value";
        eye.querySelector("use").setAttribute("href", "#i-eye");
      }
    }
    persist(event.target.type === "checkbox");
  });
  $(sel).addEventListener("click", (event) => {
    if (event.target.closest("[data-act]")?.dataset.act !== "drop-row") return;
    const rows = rowTarget(key);
    if (!rows) return;
    rows.splice(Number(event.target.closest("tr").dataset.index), 1);
    renderRows($(sel).tBodies[0], rows, { check: key !== "form", mask: key === "headers" ? headerMasked : null });
    persist();
  });
}

$$("[data-add]").forEach((button) =>
  button.addEventListener("click", () => {
    const rows = rowTarget(button.dataset.add);
    if (!rows) return;
    rows.push({ key: "", value: "", enabled: true });
    renderRows($(`#${button.dataset.add}-table`).tBodies[0], rows, { check: button.dataset.add !== "form", mask: button.dataset.add === "headers" ? headerMasked : null });
    persist();
  })
);

$("#vars-table").addEventListener("input", (event) => {
  const node = settingsNode();
  const field = event.target.dataset.field;
  if (!node || !field) return;
  node.variables[Number(event.target.closest("tr").dataset.index)][field] =
    event.target.type === "checkbox" ? event.target.checked : event.target.value;
  if (field === "secret") renderRows($("#vars-table").tBodies[0], node.variables, { secret: true });
  $("#vars-count").textContent = node.variables.filter((row) => row.enabled !== false && row.key).length || "";
  renderVarPeek();
  refreshEnvRow(node);
  persist(event.target.type === "checkbox");
});

$("#vars-table").addEventListener("click", (event) => {
  if (event.target.closest("[data-act]")?.dataset.act !== "drop-row") return;
  const node = settingsNode();
  if (!node) return;
  node.variables.splice(Number(event.target.closest("tr").dataset.index), 1);
  renderRows($("#vars-table").tBodies[0], node.variables, { secret: true });
  renderVarPeek();
  refreshEnvRow(node);
  persist();
});

$("[data-add-vars]").addEventListener("click", () => {
  const node = settingsNode();
  if (!node) return;
  node.variables.push({ key: "", value: "", enabled: true, secret: false });
  renderRows($("#vars-table").tBodies[0], node.variables, { secret: true });
  renderVarPeek();
  refreshEnvRow(node);
  persist();
});

function closeEnvPop() {
  $("#env-pop").hidden = true;
  $("#env-button").setAttribute("aria-expanded", "false");
}

$("#env-button").addEventListener("click", () => {
  const pop = $("#env-pop");
  const opening = pop.hidden;
  pop.hidden = !opening;
  $("#env-button").setAttribute("aria-expanded", String(opening));
});

$("#env-list").addEventListener("click", (event) => {
  const action = event.target.closest("[data-act]")?.dataset.act;
  const row = event.target.closest(".ws-row");
  if (!row) return;
  const env = locateEnvironment(row.dataset.id);
  if (action === "export") {
    if (env) exportEnvironment(env);
    return;
  }
  if (action === "rename") {
    closeEnvPop();
    if (env) openRenameDialog({ kind: "environment", id: env.id, name: env.name });
    return;
  }
  if (action === "delete") {
    closeEnvPop();
    if (env) deleteEnvironment(env.id);
    return;
  }
  const collection = activeCollection();
  if (!collection) return;
  collection.environmentId = row.dataset.id || null;
  closeEnvPop();
  render();
  persist();
});

$("#env-new").addEventListener("click", () => {
  closeEnvPop();
  openCreateDialog("environment");
});

function wireAuthControls(selectSel, fieldsSel, readAuth) {
  const select = $(selectSel);
  const host = $(fieldsSel);
  select.addEventListener("change", () => {
    const auth = readAuth();
    if (!auth) return;
    auth.type = select.value;
    host.innerHTML = authFieldsMarkup(auth);
    persist();
  });
  const write = (event) => {
    const el = event.target;
    const auth = el.dataset.a ? readAuth() : null;
    if (!auth) return;
    auth[el.dataset.a][el.dataset.k] = el.value;
    persist(el.tagName === "SELECT");
  };
  host.addEventListener("input", write);
  host.addEventListener("change", write);
}

wireAuthControls("#req-auth-type", "#req-auth-fields", () => activeReq()?.auth);
wireAuthControls("#node-auth-type", "#node-auth-fields", () => settingsNode()?.auth);

// One reveal affordance for every masked editable surface (header grid rows, Auth fields).
// Reveal is per-row and not sticky: the next renderRows/renderAuth rebuilds it masked.
document.addEventListener("click", (event) => {
  const eye = event.target.closest('[data-act="reveal-row"], [data-act="reveal-auth"]');
  if (!eye) return;
  const input = eye.closest(".value-wrap, .auth-value")?.querySelector("input");
  if (!input) return;
  const reveal = input.type === "password";
  input.type = reveal ? "text" : "password";
  eye.setAttribute("aria-pressed", String(reveal));
  eye.title = reveal ? "Hide value" : "Reveal value";
  eye.querySelector("use").setAttribute("href", reveal ? "#i-eye-off" : "#i-eye");
});

// Environments share the settings pane but have no scripts of their own.
const nodeScript = (key) => (value) => {
  const node = settingsNode();
  if (node?.scripts) node.scripts[key] = value;
};

for (const [sel, apply] of [
  ["#req-pre", (value) => (activeReq().scripts.pre = value)],
  ["#req-test", (value) => (activeReq().scripts.test = value)],
  ["#node-pre", nodeScript("pre")],
  ["#node-test", nodeScript("test")],
]) {
  $(sel).addEventListener("input", (event) => {
    apply(event.target.value);
    persist(false);
  });
}

$("#method").addEventListener("change", (event) => {
  activeReq().method = event.target.value;
  event.target.dataset.method = event.target.value;
  renderTabbar();
  renderSidebar();
  persist();
});

$("#url").addEventListener("change", (event) => {
  const req = activeReq();
  const { base, params } = splitUrl(event.target.value.trim());
  req.url = base;
  if (params.length) req.params = params;
  renderRequest();
  persist(false);
});

$$('input[name="body-mode"]').forEach((radio) =>
  radio.addEventListener("change", () => {
    activeReq().body.mode = radio.value;
    renderBody(activeReq());
    persist();
  })
);

$("#content-type").addEventListener("change", (event) => {
  activeReq().body.contentType = event.target.value;
  persist();
});

$("#body").addEventListener("input", (event) => {
  activeReq().body.raw = event.target.value;
  persist(false);
});

$("#send").addEventListener("click", send);
$("#save").addEventListener("click", openSaveDialog);
$("#new-collection").addEventListener("click", () => openCreateDialog("collection"));
$("#new-environment").addEventListener("click", () => openCreateDialog("environment"));
$("#env-import-btn").addEventListener("click", () => $("#env-import-file").click());

const RAIL_VIEWS = [
  ["#rail-collections", "collections"],
  ["#rail-environments", "environments"],
];

function renderRail() {
  const collapsed = $(".shell").classList.contains("rail-collapsed");
  for (const [sel, view] of RAIL_VIEWS) {
    $(sel).classList.toggle("is-active", !collapsed && state.sidebarView === view);
  }
}

// The button for the view already showing behaves as the sidebar hide toggle.
function clickRail(view) {
  const shellEl = $(".shell");
  if (state.sidebarView === view) shellEl.classList.toggle("rail-collapsed");
  else {
    state.sidebarView = view;
    state.filter = "";
    $("#tree-search").value = "";
    shellEl.classList.remove("rail-collapsed");
  }
  renderRail();
  renderSidebar();
}

for (const [sel, view] of RAIL_VIEWS) {
  $(sel).addEventListener("click", () => clickRail(view));
}

/* ------------------------------------------------------ variable peek overlay */

const VAR_TOKEN = /\{\{\s*([^{}]+?)\s*\}\}/g;

function urlTokens(text) {
  const names = [];
  for (const match of text.matchAll(VAR_TOKEN)) {
    const name = match[1].trim();
    if (name && !names.includes(name)) names.push(name);
  }
  return names;
}

function varSource(label, variables) {
  const rows = (variables ?? []).filter((row) => row.enabled !== false && row.key.trim());
  return {
    label,
    vars: new Map(rows.map((row) => [row.key.trim(), row.value])),
    secret: new Set(rows.filter((row) => row.secret).map((row) => row.key.trim())),
  };
}

function varSources(req) {
  const chain = parentChain(req.id);
  const sources = chain.map((node) =>
    varSource(parentChain(node.id).map((n) => n.name).join(" / "), node.variables)
  );
  const env = envForCollection(chain[0]);
  if (env) sources.splice(1, 0, varSource(`Environment: ${env.name}`, env.variables));
  return sources;
}

function resolveVar(name, sources) {
  for (let i = sources.length - 1; i >= 0; i--) {
    if (sources[i].vars.has(name)) {
      return { value: sources[i].vars.get(name), scope: sources[i].label, secret: maskVariable(name, sources[i].secret.has(name)) };
    }
  }
  return null;
}

function renderVarPeek(liveText) {
  const req = activeReq();
  const peek = $("#var-peek");
  const names = req ? urlTokens(liveText ?? req.url) : [];
  peek.hidden = names.length === 0;
  $(".urlbox").classList.toggle("has-vars", names.length > 0);
  peek.querySelector(".var-count").textContent = names.length;
  if (!$("#var-overlay").hidden) renderVarOverlay(names, liveText);
}

function renderVarOverlay(names, liveText) {
  const overlay = $("#var-overlay");
  const req = activeReq();
  const text = liveText ?? req?.url ?? "";
  if (!req || !names.length) {
    closeVarOverlay();
    return;
  }
  const sources = varSources(req);
  const shown = (hit) => (hit.secret ? `<code class="var-value secret">${SECRET_MASK}</code>` : `<code class="var-value">${esc(hit.value) || '""'}</code>`);
  const rows = names
    .map((name) => {
      const hit = resolveVar(name, sources);
      return `
      <div class="var-row ${hit ? "" : "missing"}">
        <code class="var-token">{{${esc(name)}}}</code>
        <span class="var-arrow">${icon("caret")}</span>
        ${
          hit
            ? `${shown(hit)}<span class="var-scope" title="${esc(hit.scope)}">${esc(hit.scope)}</span>`
            : `<code class="var-value">undefined</code><span class="var-scope">not defined in any scope</span>`
        }
      </div>`;
    })
    .join("");
  let resolved = "";
  let last = 0;
  for (const match of text.matchAll(VAR_TOKEN)) {
    resolved += esc(text.slice(last, match.index));
    const hit = resolveVar(match[1].trim(), sources);
    resolved += hit
      ? `<b class="var-sub${hit.secret ? " secret" : ""}">${hit.secret ? SECRET_MASK : esc(hit.value)}</b>`
      : `<b class="var-sub missing">${esc(match[0])}</b>`;
    last = match.index + match[0].length;
  }
  resolved += esc(text.slice(last));
  overlay.innerHTML = `
    <div class="var-overlay-head">
      <span>Variables in URL</span>
      <span class="var-hint"><kbd>Esc</kbd> close</span>
    </div>
    ${rows}
    <div class="var-resolved">
      <span class="var-resolved-label">Resolved URL</span>
      <code>${resolved}</code>
    </div>`;
}

function closeVarOverlay() {
  $("#var-overlay").hidden = true;
  $("#var-peek").setAttribute("aria-expanded", "false");
}

$("#var-peek").addEventListener("click", () => {
  const overlay = $("#var-overlay");
  const opening = overlay.hidden;
  overlay.hidden = !opening;
  $("#var-peek").setAttribute("aria-expanded", String(opening));
  if (opening) renderVarOverlay(urlTokens($("#url").value), $("#url").value);
});

document.addEventListener("click", (event) => {
  if ($("#var-overlay").hidden) return;
  if (event.target.closest("#var-overlay, #var-peek")) return;
  closeVarOverlay();
});

$("#url").addEventListener("input", (event) => {
  if (!activeReq()) return;
  const { base, params } = splitUrl(event.target.value);
  activeReq().url = base;
  if (params.length) activeReq().params = params;
  renderVarPeek(event.target.value);
  if (drawer.open && drawerPane === "code") renderCodeOutput();
  persist(false);
});

/* ---------------------------------------------------------- sidebar resize */

const shell = $(".shell");
const resizer = $("#sidebar-resizer");
const SIDEBAR_KEY = "postlite.sidebarWidth";
const SIDEBAR_MIN = 180;
const SIDEBAR_MAX = 480;
let resizing = false;
let dragStartX = 0;
let dragStartW = 0;

const sidebarMax = () =>
  Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, window.innerWidth - 48 - 320));

function applySidebarWidth(width, save = false) {
  const w = Math.round(Math.min(sidebarMax(), Math.max(SIDEBAR_MIN, width)));
  shell.style.setProperty("--sidebar-w", `${w}px`);
  resizer.setAttribute("aria-valuenow", String(w));
  if (save) {
    state.settings.sidebarWidth = w;
    persist();
  }
  return w;
}

function resetSidebarWidth() {
  applySidebarWidth(DEFAULT_SETTINGS.sidebarWidth, true);
  try { localStorage.removeItem(SIDEBAR_KEY); } catch {}
}

function migrateSidebarWidth() {
  try {
    const saved = Number(localStorage.getItem(SIDEBAR_KEY));
    if (Number.isFinite(saved) && saved > 0) state.settings.sidebarWidth = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, saved));
  } catch {}
}
resizer.setAttribute(
  "aria-valuenow",
  String(Math.round($("#sidebar").getBoundingClientRect().width))
);

resizer.addEventListener("pointerdown", (event) => {
  if (event.button !== 0) return;
  event.preventDefault();
  resizing = true;
  dragStartX = event.clientX;
  dragStartW = $("#sidebar").getBoundingClientRect().width;
  resizer.setPointerCapture(event.pointerId);
  resizer.focus({ preventScroll: true });
  document.body.classList.add("resizing");
});

resizer.addEventListener("pointermove", (event) => {
  if (!resizing) return;
  applySidebarWidth(dragStartW + event.clientX - dragStartX);
});

function endResize(event) {
  if (!resizing) return;
  resizing = false;
  document.body.classList.remove("resizing");
  try {
    resizer.releasePointerCapture(event.pointerId);
  } catch {}
  const w = Math.round($("#sidebar").getBoundingClientRect().width);
  resizer.setAttribute("aria-valuenow", String(w));
  applySidebarWidth(w, true);
}

resizer.addEventListener("pointerup", endResize);
resizer.addEventListener("pointercancel", endResize);
resizer.addEventListener("dblclick", resetSidebarWidth);

resizer.addEventListener("keydown", (event) => {
  const current =
    Math.round($("#sidebar").getBoundingClientRect().width) || 276;
  const step = event.shiftKey ? 48 : 16;
  if (event.key === "ArrowLeft") {
    event.preventDefault();
    applySidebarWidth(current - step, true);
  } else if (event.key === "ArrowRight") {
    event.preventDefault();
    applySidebarWidth(current + step, true);
  } else if (event.key === "Home") {
    event.preventDefault();
    applySidebarWidth(SIDEBAR_MIN, true);
  } else if (event.key === "End") {
    event.preventDefault();
    applySidebarWidth(sidebarMax(), true);
  } else if (event.key === "Enter") {
    event.preventDefault();
    resetSidebarWidth();
  }
});

window.addEventListener("resize", () => {
  const inline = shell.style.getPropertyValue("--sidebar-w");
  if (!inline) return;
  const current = parseFloat(inline);
  if (current > sidebarMax()) applySidebarWidth(current, true);
});

$("#import-btn").addEventListener("click", () => $("#import-file").click());
$("#import-file").addEventListener("change", async (event) => {
  const file = event.target.files?.[0];
  if (!file) return;
  try {
    const doc = JSON.parse(await file.text());
    if (!Array.isArray(doc?.item)) throw new Error("not a Postman collection export");
    const collection = fromPostman(doc);
    state.collections.push(collection);
    persist();
    render();
    $("#error").textContent = "";
  } catch (err) {
    showError(`import failed: ${err.message}`);
  } finally {
    event.target.value = "";
  }
});

$("#env-import-file").addEventListener("change", async (event) => {
  const files = [...(event.target.files ?? [])];
  if (!files.length) return;
  const failures = [];
  for (const file of files) {
    try {
      state.environments.push(fromPostmanEnvironment(JSON.parse(await file.text())));
    } catch (err) {
      failures.push(`${file.name}: ${err.message}`);
    }
  }
  if (failures.length) showError(`environment import failed: ${failures.join(" · ")}`);
  else $("#error").textContent = "";
  persist();
  render();
  event.target.value = "";
});

const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
const appSettingsDialog = $("#app-settings-dialog");

function applyTheme() {
  const theme = state.settings.general.theme;
  document.documentElement.dataset.theme = theme === "system" ? (systemTheme.matches ? "dark" : "light") : theme;
}

const settingPath = (path) => path.split(".").reduce((node, key) => node?.[key], state.settings);

const PROXY_HELP = {
  direct: "No proxy. Requests connect directly to the target.",
  manual: "Every request goes through this proxy. Bypass is a comma list of hosts that connect directly.",
  environment: "Reads HTTP_PROXY / HTTPS_PROXY / ALL_PROXY (and lowercase), plus NO_PROXY for bypass. Nothing set means direct.",
};

function refreshProxyControls() {
  const manual = state.settings.request.proxy.mode === "manual";
  for (const id of ["#pref-proxy-url", "#pref-proxy-bypass", "#pref-proxy-user", "#pref-proxy-pass"]) $(id).disabled = !manual;
  $("#pref-proxy-help").textContent = PROXY_HELP[state.settings.request.proxy.mode];
}

let lastCookiePersist = null;
function syncCookiePersist() {
  const on = state.settings.request.persistCookies;
  if (on === lastCookiePersist) return;
  lastCookiePersist = on;
  invoke("cookie_set_persist", { on }).catch((err) => notify(`Cookie persistence: ${String(err)}`));
}

function refreshPreferenceControls() {
  for (const control of $$('[data-setting]', appSettingsDialog)) {
    const value = settingPath(control.dataset.setting);
    if (control.type === "checkbox") control.checked = value;
    else control.value = String(value);
  }
  $("#pref-autoSaveIntervalMs").disabled = !state.settings.general.autoSave;
  $("#pref-maxRedirects").disabled = !state.settings.request.followRedirects;
  refreshProxyControls();
}

function applyAppSettings() {
  applyTheme();
  applySidebarWidth(state.settings.sidebarWidth);
  for (const pane of $$("#pretty, #raw")) pane.classList.toggle("word-wrap", state.settings.response.wordWrap);
  $('#res-tabs [data-tab="res-headers"]').hidden = !state.settings.response.showHeaders;
  activateTab("#res-tabs .tab", "#pretty, #raw, #preview, #res-headers, .tests", $(`#res-tabs [data-tab="${state.settings.response.defaultView}"]`));
  refreshPreferenceControls();
  syncCookiePersist();
  render();
}

function refreshTransferOptions() {
  for (const [selector, items] of [["#settings-collection", state.collections], ["#settings-environment", state.environments]]) {
    const select = $(selector);
    select.innerHTML = items.length ? items.map((item) => `<option value="${esc(item.id)}">${esc(item.name)}</option>`).join("")
      : '<option value="">None in this workspace</option>';
    select.disabled = !items.length;
    $(selector.replace("settings-", "settings-export-")).disabled = !items.length;
  }
}

function openAppSettings() {
  if ($("dialog[open]")) return;
  closeWsPop();
  closeEnvPop();
  closeVarOverlay();
  refreshPreferenceControls();
  refreshTransferOptions();
  void loadProxyCredentials();
  $("#preferences-message").textContent = "Changes apply immediately across all workspaces.";
  appSettingsDialog.showModal();
}

systemTheme.addEventListener("change", applyTheme);
$("#open-app-settings").addEventListener("click", openAppSettings);
$("#app-settings-close").addEventListener("click", () => appSettingsDialog.close());
$("#app-settings-tabs").addEventListener("click", (event) => {
  const tab = event.target.closest(".tab");
  if (tab) activateTab("#app-settings-tabs .tab", "#app-settings-dialog .pane", tab);
});
appSettingsDialog.addEventListener("change", (event) => {
  const control = event.target.closest("[data-setting]");
  if (!control) return;
  if (!control.checkValidity()) { control.reportValidity(); return; }
  const path = control.dataset.setting.split(".");
  const key = path.at(-1);
  let value = control.type === "checkbox" ? control.checked : control.value;
  if (control.type === "number" || (key === "jsonIndent" && value !== "tab")) value = Number(value);
  let node = state.settings;
  for (const part of path.slice(0, -1)) node = node[part];
  node[key] = value;
  state.settings = normalizeSettings(state.settings);
  applyAppSettings();
  persist();
  $("#preferences-message").textContent = "Preference applied.";
});

// Proxy credentials are secrets: they live in the OS secret store, never in settings or a save.
let proxyCredentials = { username: "", password: "" };
async function loadProxyCredentials() {
  try { proxyCredentials = (await invoke("proxy_credentials_get")) ?? { username: "", password: "" }; }
  catch { proxyCredentials = { username: "", password: "" }; }
  $("#pref-proxy-user").value = proxyCredentials.username;
  $("#pref-proxy-pass").value = proxyCredentials.password;
}
for (const [id, field] of [["#pref-proxy-user", "username"], ["#pref-proxy-pass", "password"]]) {
  $(id).addEventListener("change", async (event) => {
    proxyCredentials[field] = event.target.value;
    try {
      await invoke("proxy_credentials_set", { username: proxyCredentials.username, password: proxyCredentials.password });
      $("#preferences-message").textContent = "Proxy credentials saved to the OS secret store.";
    } catch (err) {
      $("#preferences-message").textContent = `Proxy credentials: ${String(err)}`;
    }
  });
}

$("#settings-clear-cookies").addEventListener("click", async () => {
  const ws = activeWorkspace();
  const ok = await confirmDialogShow({
    title: "Clear all cookies",
    message: `Delete every cookie stored for “${ws.name}”? This cannot be undone.`,
    action: "Clear cookies",
  });
  if (!ok) return;
  try {
    await invoke("cookie_purge", { workspaceId: ws.id });
    refreshCookiesIfVisible();
    $("#preferences-message").textContent = `Cookies cleared for “${ws.name}”.`;
  } catch (err) {
    $("#preferences-message").textContent = `Clear cookies: ${String(err)}`;
  }
});

async function saveWorkspaceNow() {
  try {
    await flushWorkspace();
    $("#preferences-message").textContent = "Workspace saved.";
  } catch (err) {
    $("#preferences-message").textContent = `Save failed: ${String(err)}`;
  }
}
$("#save-workspace-now").addEventListener("click", saveWorkspaceNow);
$("#status-save-now").addEventListener("click", saveWorkspaceNow);
$("#settings-export").addEventListener("click", () => {
  exportSettings();
  $("#preferences-message").textContent = "Settings exported. This file contains preferences only.";
});
$("#settings-import").addEventListener("click", () => $("#settings-file").click());
$("#settings-file").addEventListener("change", async (event) => {
  const file = event.target.files?.[0];
  if (!file) return;
  $("#preferences-message").textContent = "Importing settings…";
  try {
    importSettings(JSON.parse(await file.text()));
    $("#preferences-message").textContent = "Settings imported.";
  } catch (err) {
    $("#preferences-message").textContent = `Import failed: ${err.message}`;
  } finally { event.target.value = ""; }
});
$("#settings-reset").addEventListener("click", async () => {
  if (!await confirmDialogShow({ title: "Reset application settings", message: "Restore default preferences? Collections, environments, and requests will not be deleted.", action: "Reset settings" })) return;
  state.settings = normalizeSettings();
  try { localStorage.removeItem(SIDEBAR_KEY); } catch {}
  applyAppSettings();
  persist();
  $("#preferences-message").textContent = "Default settings restored. Your workspace data is unchanged.";
});
$("#settings-import-collection").addEventListener("click", () => {
  appSettingsDialog.close();
  $("#import-file").click();
});
$("#settings-import-environment").addEventListener("click", () => {
  appSettingsDialog.close();
  $("#env-import-file").click();
});
$("#settings-export-collection").addEventListener("click", () => {
  const collection = locateCollection($("#settings-collection").value);
  if (collection) void exportCollection(collection);
});
$("#settings-export-environment").addEventListener("click", () => {
  const env = locateEnvironment($("#settings-environment").value);
  if (env) exportEnvironment(env);
});

/* Code drawer footer + cURL import dialog */
$("#code-target").addEventListener("change", renderCodeOutput);
$("#code-copy").addEventListener("click", async () => {
  const code = $("#code-output").textContent;
  notify(await copyText(code)
    ? `Snippet copied (${code.length} characters, variables resolved)`
    : "The clipboard is not available.");
});
$("#code-import").addEventListener("click", openCurlDialog);
$("#code-export").addEventListener("click", exportSnippet);
$("#curl-import").addEventListener("click", () => void importCurlCommand());
$("#curl-cancel").addEventListener("click", () => curlDialog.close());

/* full backup: everything the app stores, in one file */
$("#settings-export-backup").addEventListener("click", () => void exportBackup());
$("#settings-import-backup").addEventListener("click", () => {
  appSettingsDialog.close();
  $("#backup-file").click();
});
$("#backup-file").addEventListener("change", async (event) => {
  const file = event.target.files?.[0];
  if (!file) return;
  try {
    openRestoreDialog(await file.text());
  } finally {
    event.target.value = "";
  }
});
$("#restore-apply").addEventListener("click", () => void confirmRestore());
$("#restore-cancel").addEventListener("click", () => restoreDialog.close());

document.addEventListener("keydown", (event) => {
  if (document.body.inert) {
    // Behind a modal only the dialog's own Escape handling may run.
    if (event.key !== "Escape") event.preventDefault();
    return;
  }
  // A real modal (showModal) owns its keyboard: typing, Tab, and native Escape
  // must stay untouched; only the app shortcuts below are suppressed. The
  // non-modal drawer is deliberately excluded so it never blocks this path.
  if ($("dialog[open]:not(#drawer)")) return;
  if (event.key === "Escape" && !$("#var-overlay").hidden) closeVarOverlay();
  if (event.key === "Escape" && !$("#tab-menu").hidden) closeTabMenu();
  if (event.key === "Escape" && drawer.open) { event.preventDefault(); closeDrawer(); }
  if (event.key === "Escape" && !$("#ws-pop").hidden) closeWsPop();
  if (event.key === "Escape" && !$("#env-pop").hidden) closeEnvPop();
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.repeat) return;
  const key = event.key.toLowerCase();
  if (!["enter", "s", "n", "k", ",", "w", "pageup", "pagedown"].includes(key)) return;
  event.preventDefault();
  if (key === "enter") void send();
  else if (key === "s") openSaveDialog();
  else if (key === "n" && event.shiftKey) openCreateDialog("collection");
  else if (key === "n") addTab();
  else if (key === "k") { $("#tree-search").focus(); $("#tree-search").select(); }
  else if (key === ",") openAppSettings();
  else if (key === "pageup") cycleTab(-1);
  else if (key === "pagedown") cycleTab(1);
  else if (key === "w" && state.activeTabId) closeTab(state.activeTabId);
});

await loadWorkspace();
workspaceReady = true;
applyAppSettings();

try {
  const version = await window.__TAURI__.app.getVersion();
  for (const label of $$("#app-version, .app-version")) label.textContent = version;
} catch { $("#app-version").textContent = "Unavailable"; }

try {
  const appWindow = window.__TAURI__.window.getCurrentWindow();
  let closing = false;
  await appWindow.onCloseRequested(async (event) => {
    event.preventDefault();
    if (closing) return;
    closing = true;
    try {
      document.activeElement?.blur();
      document.body.inert = true;
      do { await flushWorkspace(); } while (savedRevision !== revision);
      await appWindow.destroy();
    } catch (err) {
      document.body.inert = false;
      closing = false;
      showError(`Could not close safely: ${String(err)}`);
    }
  });
} catch (err) { showError(`Close-time saving unavailable: ${String(err)}`); }
