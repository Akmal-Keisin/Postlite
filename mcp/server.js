#!/usr/bin/env node
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import {
  activeWorkspace, collectionItems, collectVars, defaultWorkspaceFile, environmentOf,
  findRequest, loadDocument, prepareRequest, requestSettings, resolveAuth, workspacesOf,
} from "./postlite.js";
import { executeRequest } from "./http.js";

const workspaceFile = defaultWorkspaceFile();
const server = new McpServer({ name: "postlite", version: "0.1.0" });

const json = (data) => ({ content: [{ type: "text", text: JSON.stringify(data, null, 2) }] });
const fail = (message) => ({ content: [{ type: "text", text: message }], isError: true });
const guarded = (handler) => async (args) => {
  try {
    return await handler(args);
  } catch (err) {
    return fail(err?.message ?? String(err));
  }
};

function pickWorkspace(doc, workspaceId) {
  if (workspaceId) {
    const ws = workspacesOf(doc).find((w) => w.id === workspaceId);
    if (!ws) throw new Error(`workspace not found: ${workspaceId}`);
    return ws;
  }
  const ws = activeWorkspace(doc);
  if (!ws) throw new Error("the workspace file has no workspaces");
  return ws;
}

const treeNode = (node) => node.kind === "folder"
  ? { kind: "folder", id: node.id, name: node.name, items: (node.items ?? []).map(treeNode) }
  : { kind: "request", id: node.id, name: node.name, method: node.method, url: node.url };

server.registerTool("list_workspaces", {
  description: "List postlite workspaces with collection and environment counts. Reads workspace.json read-only.",
  inputSchema: {},
}, guarded(async () => {
  const doc = loadDocument(workspaceFile);
  return json(workspacesOf(doc).map((ws) => ({
    id: ws.id,
    name: ws.name,
    active: ws.id === (doc.activeWorkspaceId ?? activeWorkspace(doc)?.id),
    collections: (ws.collections ?? []).length,
    environments: (ws.environments ?? []).length,
  })));
}));

server.registerTool("list_collections", {
  description: "List collections in a workspace as a tree of folders and requests (id, name, method, url). Defaults to the active workspace.",
  inputSchema: { workspaceId: z.string().optional().describe("Workspace id; defaults to the active workspace") },
}, guarded(async ({ workspaceId }) => {
  const doc = loadDocument(workspaceFile);
  const ws = pickWorkspace(doc, workspaceId);
  return json({
    workspace: { id: ws.id, name: ws.name },
    collections: (ws.collections ?? []).map((collection) => ({
      id: collection.id,
      name: collection.name,
      environmentId: collection.environmentId ?? null,
      items: collectionItems(collection).map(treeNode),
    })),
  });
}));

server.registerTool("get_request", {
  description: "Get a saved request by id with its folder/collection chain, the effective variables (collection < environment < folder precedence) and the resolved auth. Secret variable values are included as stored.",
  inputSchema: {
    requestId: z.string().describe("Request id from list_collections"),
    workspaceId: z.string().optional(),
  },
}, guarded(async ({ requestId, workspaceId }) => {
  const doc = loadDocument(workspaceFile);
  const found = findRequest(doc, requestId, workspaceId);
  if (!found) throw new Error(`request not found: ${requestId}`);
  const { workspace, collection, parents, request } = found;
  const env = environmentOf(workspace, collection);
  return json({
    workspace: { id: workspace.id, name: workspace.name },
    collection: { id: collection.id, name: collection.name },
    path: parents.map((node) => node.name),
    environment: env ? { id: env.id, name: env.name } : null,
    variables: Object.fromEntries(collectVars(workspace, collection, parents)),
    resolvedAuth: resolveAuth(request, parents),
    request,
  });
}));

server.registerTool("list_environments", {
  description: "List environments and their variables in a workspace. Defaults to the active workspace. Secret variable values are included as stored.",
  inputSchema: { workspaceId: z.string().optional() },
}, guarded(async ({ workspaceId }) => {
  const doc = loadDocument(workspaceFile);
  const ws = pickWorkspace(doc, workspaceId);
  return json({
    workspace: { id: ws.id, name: ws.name },
    environments: (ws.environments ?? []).map((env) => ({ id: env.id, name: env.name, variables: env.variables ?? [] })),
  });
}));

server.registerTool("send_request", {
  description: "Send a saved postlite request by id, resolving variables, path variables, query params and inherited auth exactly like the app, using the app's saved HTTP settings. Pre-request/test scripts are NOT run and the encrypted cookie jar is NOT attached (set a Cookie header manually if needed). Does not modify workspace.json.",
  inputSchema: {
    requestId: z.string().describe("Request id from list_collections"),
    workspaceId: z.string().optional(),
    variables: z.record(z.string()).optional().describe("Variable overrides, applied with the highest precedence"),
  },
}, guarded(async ({ requestId, workspaceId, variables }) => {
  const doc = loadDocument(workspaceFile);
  const found = findRequest(doc, requestId, workspaceId);
  if (!found) throw new Error(`request not found: ${requestId}`);
  const { workspace, collection, parents, request } = found;
  const prepared = prepareRequest(workspace, collection, parents, request, variables ?? {});
  const response = await executeRequest(prepared, requestSettings(doc));
  return json({
    request: { method: prepared.method, url: prepared.url, headers: prepared.headers },
    response,
    notes: [
      "pre-request and test scripts were not run",
      "no cookie jar attached; cookies neither replay nor persist",
    ],
  });
}));

server.registerTool("send_http_request", {
  description: "Send an ad-hoc HTTP request (not saved in postlite) using the app's saved HTTP settings. No variables, scripts or cookies are applied.",
  inputSchema: {
    method: z.string().describe("HTTP method, e.g. GET, POST"),
    url: z.string().describe("Absolute http/https URL"),
    headers: z.record(z.string()).optional().describe("Header name/value pairs"),
    body: z.string().optional().describe("Raw request body"),
  },
}, guarded(async ({ method, url, headers, body }) => {
  let settings;
  try {
    settings = requestSettings(loadDocument(workspaceFile));
  } catch {
    settings = requestSettings({});
  }
  const prepared = {
    method,
    url,
    headers: Object.entries(headers ?? {}).map(([key, value]) => ({ key, value })),
    body: body ?? "",
    form: [],
  };
  return json({ response: await executeRequest(prepared, settings) });
}));

await server.connect(new StdioServerTransport());
console.error(`postlite-mcp listening on stdio (workspace file: ${workspaceFile})`);