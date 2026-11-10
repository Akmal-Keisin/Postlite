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

const state = {
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
    method: fields.method ?? "GET",
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

// workspace.json written before folders existed kept requests in a flat field.
const migrateCollection = (stored) =>
  newCollection(stored.name ?? "Collection", { ...stored, items: stored.items ?? stored.requests ?? [] });

const activeTab = () => state.tabs.find((t) => t.id === state.activeTabId) ?? null;
const activeReq = () => activeTab()?.req ?? null;

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

const debounce = (fn, ms) => {
  let timer;
  return (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
};

const persist = debounce(() => {
  renderTabbar();
  const payload = {
    collections: state.collections,
    environments: state.environments,
    tabs: state.tabs.map((t) => t.req),
    activeTabId: state.activeTabId,
  };
  $(".statusbar").classList.add("saving");
  $("#save-state").textContent = "Saving…";
  invoke("save_workspace", { state: JSON.stringify(payload) })
    .then(() => {
      $(".statusbar").classList.remove("saving");
      $("#save-state").textContent = `Saved ${new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}`;
    })
    .catch((err) => {
      $(".statusbar").classList.remove("saving");
      $("#save-state").textContent = "Save failed";
      showError(String(err));
    });
}, 350);

function showError(message) {
  $("#error").textContent = message;
}

async function loadWorkspace() {
  const raw = await invoke("load_workspace");
  if (raw) {
    try {
      const saved = JSON.parse(raw);
      state.collections = (saved.collections ?? []).map(migrateCollection);
      state.environments = (saved.environments ?? []).map(newEnvironment);
      state.tabs = (saved.tabs ?? []).map((req) => ({ id: uid(), req: newRequest(req), response: null }));
      state.activeTabId = state.tabs.some((t) => t.id === saved.activeTabId)
        ? saved.activeTabId
        : state.tabs[0]?.id ?? null;
    } catch {
      showError("saved workspace was unreadable, starting fresh");
    }
  }
  if (!state.tabs.length) addTab();
}

function addTab(req = newRequest()) {
  const tab = { id: uid(), req, response: null };
  state.tabs.push(tab);
  state.activeTabId = tab.id;
  state.selected = null;
  render();
  return tab;
}

function closeTab(id) {
  state.tabs = state.tabs.filter((t) => t.id !== id);
  if (state.activeTabId === id) state.activeTabId = state.tabs.at(-1)?.id ?? null;
  if (!state.tabs.length) addTab();
  render();
  persist();
}

function closeOtherTabs(id) {
  state.tabs = state.tabs.filter((t) => t.id === id);
  state.activeTabId = id;
  state.selected = null;
  render();
  persist();
}

function closeAllTabs() {
  state.tabs = [];
  state.activeTabId = null;
  addTab();
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
  (rows ?? []).map((row) => ({ key: row.key ?? "", value: String(row.value ?? ""), enabled: true }));

function importPostmanItems(items) {
  const out = [];
  for (const item of items ?? []) {
    if (Array.isArray(item.item)) {
      out.push(
        newFolder({
          name: item.name ?? "Folder",
          items: importPostmanItems(item.item),
          auth: fromPostmanAuth(item.auth),
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
    auth: doc.auth,
    variables: doc.variable,
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
    body.contentType = req.body.options?.raw?.language ?? "";
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

  const contentType =
    body.contentType || headers.find((h) => h.key.toLowerCase() === "content-type")?.value || "application/json";

  return newRequest({
    name: item.name ?? "Request",
    method: (req.method ?? "GET").toUpperCase(),
    url: base,
    params: params.length ? params : fromRaw,
    headers: headers.length ? headers : undefined,
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

function exportEnvironment(env) {
  const file = `${String(env.name || "environment").replace(/[^\w.-]+/g, "_")}.postman_environment.json`;
  const url = URL.createObjectURL(
    new Blob([JSON.stringify(toPostmanEnvironment(env), null, 2)], { type: "application/json" })
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = file;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
}

/* ---------------------------------------------------------------------- render */

function render() {
  renderRail();
  renderSidebar();
  renderTabbar();
  renderEnvBar();
  renderRequest();
  renderSettings();
  renderResponse();
  $("#stat-collections").textContent = state.collections.length;
  $("#stat-tabs").textContent = state.tabs.length;
}

// The collection that owns the active tab's request; null for an unsaved tab.
function activeCollection() {
  const tab = activeTab();
  return tab ? parentChain(tab.req.id)[0] ?? null : null;
}

// The topbar picker binds the active collection, so it only appears when the
// open request actually belongs to one.
function renderEnvBar() {
  const bar = $("#env-bar");
  const collection = activeCollection();
  bar.hidden = !collection;
  if (!collection) return;
  const select = $("#env-select");
  select.innerHTML = envSelectMarkup();
  select.value = envForCollection(collection) ? collection.environmentId : "";
}

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
        .map((row) => `${esc(row.key)}=${row.secret ? SECRET_MASK : esc(row.value)}`)
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
      const stored = locate(tab.req.id);
      const dirty = stored && JSON.stringify(stored) !== JSON.stringify(tab.req);
      return `
      <div class="tab-item ${tab.id === state.activeTabId ? "is-active" : ""}" data-id="${tab.id}" title="${esc(tab.req.name)}">
        <span class="method ${esc(tab.req.method)}">${esc(tab.req.method)}</span>
        <span class="tab-name">${esc(tab.req.name)}</span>
        ${dirty ? `<span class="tab-dirty" title="Unsaved changes"></span>` : ""}
        <button class="close" data-act="close-tab" title="Close" aria-label="Close tab">${icon("x")}</button>
      </div>`;
    })
    .join("");
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
  renderRows($("#headers-table").tBodies[0], req.headers);
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
  const { check = true, secret = false } = options;
  tbody.innerHTML = rows
    .map(
      (row, index) => `
      <tr data-index="${index}">
        ${check ? `<td class="c-check"><input type="checkbox" data-field="enabled" ${row.enabled ? "checked" : ""} /></td>` : ""}
        <td><input type="text" data-field="key" value="${esc(row.key)}" spellcheck="false" /></td>
        <td><input type="${secret && row.secret ? "password" : "text"}" data-field="value" value="${esc(row.value)}" spellcheck="false" /></td>
        ${secret ? `<td class="c-secret"><input type="checkbox" data-field="secret" ${row.secret ? "checked" : ""} aria-label="Secret" title="Mask this value in the UI" /></td>` : ""}
        <td class="c-del"><button class="del" data-act="drop-row" title="Remove" aria-label="Remove row">${icon("x")}</button></td>
      </tr>`
    )
    .join("");
}

const authOptions = (includeInherit) =>
  (includeInherit ? ["inherit", "noauth", "apikey", "bearer", "basic"] : ["noauth", "apikey", "bearer", "basic"])
    .map((type) => `<option value="${type}">${AUTH_LABELS[type]}</option>`)
    .join("");

function authFieldsMarkup(auth) {
  if (auth.type === "bearer")
    return `<label class="field"><span>Token</span><input type="text" data-a="bearer" data-k="token" value="${esc(auth.bearer.token)}" spellcheck="false" /></label>`;
  if (auth.type === "basic")
    return `
      <label class="field"><span>Username</span><input type="text" data-a="basic" data-k="username" value="${esc(auth.basic.username)}" spellcheck="false" /></label>
      <label class="field"><span>Password</span><input type="text" data-a="basic" data-k="password" value="${esc(auth.basic.password)}" spellcheck="false" /></label>`;
  if (auth.type === "apikey")
    return `
      <label class="field"><span>Key</span><input type="text" data-a="apikey" data-k="key" value="${esc(auth.apikey.key)}" spellcheck="false" /></label>
      <label class="field"><span>Value</span><input type="text" data-a="apikey" data-k="value" value="${esc(auth.apikey.value)}" spellcheck="false" /></label>
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

function settingsNode() {
  if (!state.selected) return null;
  if (state.selected.kind === "environment") return locateEnvironment(state.selected.id);
  const node = nodeFor(state.selected);
  return node && node.kind !== "request" ? node : null;
}

const envSelectMarkup = () =>
  `<option value="">No environment</option>${state.environments
    .map((env) => `<option value="${env.id}">${esc(env.name)}</option>`)
    .join("")}`;

function renderSettings() {
  const node = settingsNode();
  const kind = state.selected?.kind;
  const isEnv = kind === "environment";
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
    renderAuth($("#node-auth-type"), $("#node-auth-fields"), node.auth, false);
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

let lastRenderedResponse;

function renderResponse() {
  const response = activeTab()?.response;
  const pill = $("#status");
  $("#error").textContent = "";
  if (response !== lastRenderedResponse) {
    lastRenderedResponse = response;
    const res = $("#res-pane");
    res.classList.remove("flash");
    void res.offsetWidth;
    res.classList.add("flash");
  }
  if (!response) {
    pill.className = "pill";
    pill.textContent = "—";
    $("#time").textContent = "";
    $("#size").textContent = "";
    $("#pretty").textContent = "";
    $("#raw").textContent = "";
    $("#res-headers").tBodies[0].innerHTML = "";
    $("#res-headers-count").textContent = "";
    $("#copy-res").hidden = true;
    renderTests([]);
    return;
  }
  renderTests(response.results ?? []);
  if (response.error) {
    pill.className = "pill failed";
    pill.textContent = "failed";
    $("#time").textContent = `${response.elapsed_ms} ms`;
    $("#size").textContent = "";
    $("#copy-res").hidden = true;
    showError(response.error);
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
  try {
    $("#pretty").innerHTML = highlightJson(JSON.stringify(JSON.parse(response.body), null, 2));
  } catch {
    $("#pretty").textContent = response.body;
  }
  $("#res-headers-count").textContent = response.headers.length;
  $("#res-headers").tBodies[0].innerHTML = response.headers
    .map((h) => `<tr><td>${esc(h.key)}</td><td>${esc(h.value)}</td></tr>`)
    .join("");
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

async function send() {
  const req = activeReq();
  if (!req || !req.url.trim()) {
    $("#url").focus();
    return;
  }
  const parents = parentChain(req.id);
  const vars = collectVars(parents);
  const auth = resolveAuth(req, parents);
  const results = [];

  const prepared = {
    method: req.method,
    url: req.url,
    params: req.params.filter((p) => p.enabled && p.key.trim()).map((p) => ({ key: p.key, value: p.value })),
    headers: enabledHeaders(req),
    body: req.body.mode === "raw" ? req.body.raw : "",
    form:
      req.body.mode === "form-data"
        ? req.body.form.filter((f) => f.enabled !== false && f.key.trim()).map((f) => ({ key: f.key, value: f.value }))
        : [],
  };

  // ancestors run first so a folder can prepare what its requests consume
  const prePm = () => buildPm({ vars, request: prepared, results });
  for (const node of parents) runScript(node.scripts?.pre, prePm(), node.name, results);
  runScript(req.scripts.pre, prePm(), req.name, results);

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

  const button = $("#send");
  button.disabled = true;
  button.textContent = "Sending";
  let response;
  try {
    response = await invoke("http_send", {
      request: {
        method: prepared.method,
        url: joinQuery(prepared.url, [...prepared.params, ...authQuery(auth, vars)]),
        headers: prepared.headers,
        body: prepared.body,
        form: prepared.form,
      },
    });
  } finally {
    button.disabled = false;
    button.textContent = "Send";
  }

  if (!response.error) {
    for (const node of [req, ...[...parents].reverse()])
      runScript(node.scripts?.test, buildPm({ vars, response, results }), node.name, results);
  }

  activeTab().response = { ...response, results };
  renderResponse();
}

/* --------------------------------------------------------------------- dialogs */

const saveDialog = $("#save-dialog");
const collectionDialog = $("#collection-dialog");
let renameTarget = null;

function openSaveDialog() {
  const req = activeReq();
  if (!req) return;
  if (!state.collections.length) state.collections.push(newCollection("My Collection"));
  $("#save-name").value = req.name;
  const options = containerOptions();
  $("#save-collection").innerHTML = options.map((o) => `<option value="${o.id}">${esc(o.label)}</option>`).join("");
  $("#save-collection").value = homeOf(req.id) ?? options[0].id;
  saveDialog.showModal();
}

const DIALOG_SUB = {
  environment: "Environments hold a set of variables a collection can bind to.",
  default: "Collections and folders can hold requests and subfolders.",
};

function openRenameDialog(target) {
  renameTarget = target;
  $("#collection-name").value = target.name;
  $("#collection-dialog .dialog-sub").textContent =
    target.kind === "environment" ? DIALOG_SUB.environment : DIALOG_SUB.default;
  collectionDialog.showModal();
}

function openCreateDialog(kind) {
  renameTarget = { kind, id: null };
  $("#collection-name").value = "";
  $("#collection-dialog .dialog-sub").textContent = kind === "environment" ? DIALOG_SUB.environment : DIALOG_SUB.default;
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
    const target = nodeFor(renameTarget);
    if (target) target.name = name;
  } else if (renameTarget.kind === "collection") {
    state.collections.push(newCollection(name));
  } else if (renameTarget.kind === "environment") {
    const env = newEnvironment({ name });
    state.environments.push(env);
    state.selected = { id: env.id, kind: "environment" };
  }
  persist();
  render();
});

/* ---------------------------------------------------------------------- events */

$("#collections").addEventListener("click", (event) => {
  const action = event.target.closest("[data-act]")?.dataset.act;
  const node = event.target.closest(".node");
  if (!node) return;
  const { id, cid, kind } = node.dataset;

  if (action === "delete") {
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
    if (stored) addTab(structuredClone(stored));
    return;
  }

  // the caret folds the node; clicking its name opens the node's settings
  if (event.target.closest(".caret")) {
    state.expanded.has(id) ? state.expanded.delete(id) : state.expanded.add(id);
    renderSidebar();
    return;
  }
  state.selected = { id, kind };
  render();
});

$("#collections").addEventListener("dblclick", (event) => {
  const head = event.target.closest(".node-head");
  if (!head) return;
  const id = head.closest(".node").dataset.id;
  state.expanded.has(id) ? state.expanded.delete(id) : state.expanded.add(id);
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
    state.environments = state.environments.filter((e) => e.id !== id);
    for (const collection of state.collections) {
      if (collection.environmentId === id) collection.environmentId = null;
    }
    if (state.selected?.id === id) state.selected = null;
    persist();
    render();
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
  state.selected = { id, kind: "environment" };
  render();
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
  const body = activeTab()?.response?.body ?? "";
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
    button.title = "Copy response body";
    button.querySelector("use").setAttribute("href", "#i-copy");
  }, 1400);
});

$("#tabbar").addEventListener("click", (event) => {
  const item = event.target.closest(".tab-item");
  if (!item) return;
  if (event.target.closest('[data-act="close-tab"]')) closeTab(item.dataset.id);
  else {
    state.activeTabId = item.dataset.id;
    state.selected = null;
    render();
  }
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
document.addEventListener("click", (event) => {
  if (!$("#tab-menu").hidden && !event.target.closest("#tab-menu")) closeTabMenu();
});
window.addEventListener("blur", closeTabMenu);

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
  if (tab) activateTab("#res-tabs .tab", "#pretty, #raw, #res-headers, .tests", tab);
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
    persist();
  });
  $(sel).addEventListener("click", (event) => {
    if (event.target.closest("[data-act]")?.dataset.act !== "drop-row") return;
    const rows = rowTarget(key);
    if (!rows) return;
    rows.splice(Number(event.target.closest("tr").dataset.index), 1);
    renderRows($(sel).tBodies[0], rows, { check: key !== "form" });
    persist();
  });
}

$$("[data-add]").forEach((button) =>
  button.addEventListener("click", () => {
    const rows = rowTarget(button.dataset.add);
    if (!rows) return;
    rows.push({ key: "", value: "", enabled: true });
    renderRows($(`#${button.dataset.add}-table`).tBodies[0], rows, { check: button.dataset.add !== "form" });
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
  persist();
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

$("#env-select").addEventListener("change", (event) => {
  const collection = activeCollection();
  if (!collection) return;
  collection.environmentId = event.target.value || null;
  render();
  persist();
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
    persist();
  };
  host.addEventListener("input", write);
  host.addEventListener("change", write);
}

wireAuthControls("#req-auth-type", "#req-auth-fields", () => activeReq()?.auth);
wireAuthControls("#node-auth-type", "#node-auth-fields", () => settingsNode()?.auth);

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
    persist();
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
  persist();
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
  persist();
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
      return { value: sources[i].vars.get(name), scope: sources[i].label, secret: sources[i].secret.has(name) };
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

$("#url").addEventListener("input", (event) => renderVarPeek(event.target.value));

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
    try {
      localStorage.setItem(SIDEBAR_KEY, String(w));
    } catch {}
  }
  return w;
}

function resetSidebarWidth() {
  shell.style.removeProperty("--sidebar-w");
  try {
    localStorage.removeItem(SIDEBAR_KEY);
  } catch {}
  const w = Math.round($("#sidebar").getBoundingClientRect().width);
  resizer.setAttribute("aria-valuenow", String(w));
}

try {
  const saved = Number(localStorage.getItem(SIDEBAR_KEY));
  if (Number.isFinite(saved) && saved > 0) applySidebarWidth(saved);
} catch {}
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
  try {
    localStorage.setItem(SIDEBAR_KEY, String(w));
  } catch {}
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
    if (!doc?.item) throw new Error("not a Postman collection export");
    const collection = fromPostman(doc);
    if (!collection.items.length) throw new Error("collection has no requests");
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

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !$("#var-overlay").hidden) closeVarOverlay();
  if (event.key === "Escape" && !$("#tab-menu").hidden) closeTabMenu();
  if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
    event.preventDefault();
    send();
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    openSaveDialog();
  }
});

await loadWorkspace();
render();
