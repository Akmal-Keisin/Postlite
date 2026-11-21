import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const APP_IDENTIFIER = "app.postlite.desktop";

export function defaultWorkspaceFile() {
  if (process.env.POSTLITE_WORKSPACE_FILE) return process.env.POSTLITE_WORKSPACE_FILE;
  let dir;
  if (process.platform === "win32") {
    dir = path.join(process.env.APPDATA ?? path.join(os.homedir(), "AppData", "Roaming"), APP_IDENTIFIER);
  } else if (process.platform === "darwin") {
    dir = path.join(os.homedir(), "Library", "Application Support", APP_IDENTIFIER);
  } else {
    dir = path.join(process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local", "share"), APP_IDENTIFIER);
  }
  return path.join(dir, "workspace.json");
}

export function loadDocument(file) {
  let raw;
  try {
    raw = fs.readFileSync(file, "utf8");
  } catch (err) {
    if (err.code === "ENOENT") {
      throw new Error(`no workspace file at ${file}; open postlite once or set POSTLITE_WORKSPACE_FILE`);
    }
    throw new Error(`could not read ${file}: ${err.message}`);
  }
  try {
    return JSON.parse(raw);
  } catch {
    throw new Error(`workspace file at ${file} is not valid JSON`);
  }
}

export function workspacesOf(doc) {
  if (Array.isArray(doc.workspaces) && doc.workspaces.length) return doc.workspaces;
  // v1 files carry one implicit workspace.
  if (doc.collections || doc.environments) {
    return [{ id: "default", name: "My Workspace", collections: doc.collections ?? [], environments: doc.environments ?? [] }];
  }
  return [];
}

export const collectionItems = (collection) => collection.items ?? collection.requests ?? [];

export function activeWorkspace(doc) {
  const all = workspacesOf(doc);
  return all.find((ws) => ws.id === doc.activeWorkspaceId) ?? all[0] ?? null;
}

function walk(node, acc, visit) {
  const chain = [...acc, node];
  if (visit(node, chain)) return chain;
  for (const child of node.items ?? []) {
    const found = walk(child, chain, visit);
    if (found) return found;
  }
  return null;
}

// The request plus its collection and folder ancestors, mirroring parentChain() in the UI.
export function findRequest(doc, requestId, workspaceId) {
  const all = workspacesOf(doc);
  const ordered = workspaceId
    ? all.filter((ws) => ws.id === workspaceId)
    : [...all.filter((ws) => ws.id === doc.activeWorkspaceId), ...all.filter((ws) => ws.id !== doc.activeWorkspaceId)];
  for (const ws of ordered) {
    for (const collection of ws.collections ?? []) {
      const chain = walk(collection, [], (node) => node.kind === "request" && node.id === requestId);
      if (chain) {
        return { workspace: ws, collection, parents: chain.slice(0, -1), request: chain[chain.length - 1] };
      }
    }
  }
  return null;
}

export function environmentOf(ws, collection) {
  return (ws.environments ?? []).find((env) => env.id === collection?.environmentId) ?? null;
}

// Precedence: collection < environment < folder, same slot order as collectVars() in the UI.
export function collectVars(ws, collection, parents) {
  const env = environmentOf(ws, collection);
  const chain = env ? [collection, env, ...parents.slice(1)] : parents;
  const vars = new Map();
  for (const node of chain) {
    for (const row of node.variables ?? []) {
      if (row.enabled !== false && String(row.key).trim()) vars.set(row.key.trim(), row.value);
    }
  }
  return vars;
}

const NOAUTH = { type: "noauth", bearer: { token: "" }, basic: { username: "", password: "" }, apikey: { key: "", value: "", addTo: "header" } };

export function resolveAuth(req, parents) {
  if (req.auth?.type && req.auth.type !== "inherit") return req.auth;
  for (const node of [...parents].reverse()) {
    if (node.auth?.type && node.auth.type !== "inherit") return node.auth;
  }
  return NOAUTH;
}

export function interpolate(text, vars) {
  return String(text).replace(/\{\{\s*([^{}]+?)\s*\}\}/g, (whole, name) =>
    vars.has(name) ? String(vars.get(name)) : whole);
}

export function normalizeUrlPath(url) {
  const m = /^([a-z][a-z0-9+.-]*:\/\/)?([^/?#]*)([^?#]*)([?#].*)?$/i.exec(url);
  if (!m) return url;
  const [, scheme = "", authority = "", pathPart = "", tail = ""] = m;
  return scheme + authority + pathPart.replace(/\/{2,}/g, "/") + tail;
}

export function applyPathVars(url, pathVars, vars) {
  const values = new Map(
    (pathVars ?? [])
      .filter((v) => v.key && String(v.value).trim())
      .map((v) => [v.key, interpolate(String(v.value), vars)]),
  );
  if (!values.size) return url;
  const m = /^([a-z][a-z0-9+.-]*:\/\/)?([^/?#]*)([^?#]*)([?#].*)?$/i.exec(url);
  if (!m) return url;
  const [, scheme = "", authority = "", pathPart = "", tail = ""] = m;
  const replaced = pathPart.replace(/:([A-Za-z0-9_]+)/g, (whole, key) =>
    values.has(key) ? encodeURIComponent(values.get(key)) : whole);
  return scheme + authority + replaced + tail;
}

export function joinQuery(base, pairs) {
  const query = pairs
    .filter((p) => String(p.key).trim())
    .map((p) => `${encodeURIComponent(p.key)}=${encodeURIComponent(p.value)}`)
    .join("&");
  if (!query) return base;
  return `${base}${base.includes("?") ? "&" : "?"}${query}`;
}

function enabledHeaders(req) {
  const headers = (req.headers ?? [])
    .filter((h) => h.enabled !== false && String(h.key).trim())
    .map((h) => ({ key: h.key.trim(), value: h.value ?? "" }));
  const body = req.body ?? {};
  if (body.mode === "raw" && body.contentType) {
    if (!headers.some((h) => h.key.toLowerCase() === "content-type")) {
      headers.push({ key: "Content-Type", value: body.contentType });
    }
  }
  return headers;
}

function authHeaders(auth, vars) {
  const value = (text) => interpolate(text ?? "", vars);
  if (auth.type === "bearer" && value(auth.bearer?.token)) {
    return [{ key: "Authorization", value: `Bearer ${value(auth.bearer.token)}` }];
  }
  if (auth.type === "basic") {
    const credentials = `${value(auth.basic?.username)}:${value(auth.basic?.password)}`;
    return [{ key: "Authorization", value: `Basic ${Buffer.from(credentials, "utf8").toString("base64")}` }];
  }
  if (auth.type === "apikey" && auth.apikey?.addTo === "header" && value(auth.apikey?.key)) {
    return [{ key: value(auth.apikey.key), value: value(auth.apikey.value) }];
  }
  return [];
}

function authQuery(auth, vars) {
  if (auth.type !== "apikey" || auth.apikey?.addTo !== "query") return [];
  const key = interpolate(auth.apikey.key ?? "", vars);
  return key ? [{ key, value: interpolate(auth.apikey.value ?? "", vars) }] : [];
}

// prepareRequest + finalizePrepared from the UI, minus pre-request scripts (not run here).
export function prepareRequest(ws, collection, parents, req, overrides = {}) {
  const vars = collectVars(ws, collection, parents);
  for (const [key, value] of Object.entries(overrides)) vars.set(key, String(value));
  const auth = resolveAuth(req, parents);

  const prepared = {
    method: req.method ?? "GET",
    url: req.url ?? "",
    params: (req.params ?? []).filter((p) => p.enabled !== false && String(p.key).trim()).map((p) => ({ key: p.key, value: p.value ?? "" })),
    pathVars: (req.pathVars ?? []).map((v) => ({ key: v.key, value: v.value ?? "" })),
    headers: enabledHeaders(req),
    body: req.body?.mode === "raw" ? req.body.raw ?? "" : "",
    form: req.body?.mode === "form-data"
      ? (req.body.form ?? [])
          .filter((f) => f.enabled !== false && String(f.key).trim() && (f.type !== "file" || String(f.value).trim()))
          .map((f) => ({ key: f.key, value: f.value ?? "", type: f.type === "file" ? "file" : "text" }))
      : [],
  };

  const userContentType = prepared.headers.some((h) => h.key.toLowerCase() === "content-type");
  for (const header of authHeaders(auth, vars)) {
    if (userContentType && header.key.toLowerCase() === "content-type") continue;
    prepared.headers.push(header);
  }
  prepared.url = applyPathVars(normalizeUrlPath(interpolate(prepared.url, vars)), prepared.pathVars, vars);
  prepared.body = interpolate(prepared.body, vars);
  prepared.params = prepared.params.map((p) => ({ key: interpolate(p.key, vars), value: interpolate(p.value, vars) }));
  prepared.headers = prepared.headers.map((h) => ({ key: interpolate(h.key, vars), value: interpolate(h.value, vars) }));
  prepared.form = prepared.form.map((f) => ({ key: interpolate(f.key, vars), value: interpolate(f.value, vars), type: f.type }));
  prepared.url = joinQuery(prepared.url, [...prepared.params, ...authQuery(auth, vars)]);
  return prepared;
}

export const DEFAULT_REQUEST_SETTINGS = {
  requestTimeoutMs: 60000,
  followRedirects: true,
  maxRedirects: 10,
  sslVerify: true,
  disableCache: false,
  proxy: { mode: "direct", url: "", bypass: "" },
};

export function requestSettings(doc) {
  const raw = doc.settings?.request ?? {};
  const s = { ...DEFAULT_REQUEST_SETTINGS, ...raw, proxy: { ...DEFAULT_REQUEST_SETTINGS.proxy, ...(raw.proxy ?? {}) } };
  const limitKB = doc.settings?.response?.maxResponseSizeKB;
  s.maxResponseSizeKB = Number.isInteger(limitKB) && limitKB >= 1 && limitKB <= 102400 ? limitKB : 2000;
  return s;
}