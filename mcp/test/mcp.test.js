import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const serverPath = path.join(here, "..", "server.js");

function captureServer() {
  const received = [];
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => {
      const body = Buffer.concat(chunks);
      received.push({ method: req.method, url: req.url, headers: req.headers, body });
      const route = req.url.split("?")[0];
      if (route === "/redirect") {
        res.writeHead(302, { Location: "/final" });
        res.end();
      } else if (route === "/big") {
        res.writeHead(200, { "Content-Type": "text/plain" });
        res.end("x".repeat(3000));
      } else {
        res.writeHead(200, { "Content-Type": "text/plain" });
        res.end("ok");
      }
    });
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({ server, received, base: `http://127.0.0.1:${server.address().port}` });
    });
  });
}

function fixtureDoc(base, extra = {}) {
  const uploadFile = path.join(os.tmpdir(), `postlite-mcp-test-${process.pid}.png`);
  fs.writeFileSync(uploadFile, Buffer.from("fake-png-bytes"));
  const doc = {
    version: 2,
    settings: {
      request: { requestTimeoutMs: 10000, followRedirects: true, maxRedirects: 10, sslVerify: true, disableCache: false, proxy: { mode: "direct", url: "", bypass: "" } },
      response: { maxResponseSizeKB: 2000 },
    },
    activeWorkspaceId: "ws1",
    workspaces: [{
      id: "ws1",
      name: "Test WS",
      collections: [{
        id: "col1",
        name: "API",
        environmentId: "env1",
        auth: { type: "bearer", bearer: { token: "{{token}}" } },
        variables: [
          { key: "host", value: base, enabled: true },
          { key: "token", value: "col-token", enabled: true },
          { key: "shared", value: "collection", enabled: true },
        ],
        items: [
          {
            kind: "folder", id: "f1", name: "Users",
            auth: { type: "inherit" },
            variables: [{ key: "shared", value: "folder", enabled: true }],
            items: [{
              kind: "request", id: "r1", name: "Get user", method: "GET",
              url: "{{host}}/users/:id",
              pathVars: [{ key: "id", value: "{{userId}}" }],
              params: [{ key: "verbose", value: "true", enabled: true }],
              headers: [{ key: "Accept", value: "application/json", enabled: true }],
              body: { mode: "none" },
              auth: { type: "inherit" },
            }],
          },
          {
            kind: "request", id: "r2", name: "Create", method: "POST",
            url: "{{host}}/things",
            headers: [],
            body: { mode: "raw", raw: "{\"name\":\"{{shared}}\"}", contentType: "application/json" },
            auth: { type: "noauth" },
          },
          {
            kind: "request", id: "r3", name: "Upload", method: "POST",
            url: "{{host}}/upload",
            headers: [],
            body: {
              mode: "form-data",
              form: [
                { key: "note", value: "hello", enabled: true, type: "text" },
                { key: "upload", value: uploadFile, enabled: true, type: "file" },
              ],
            },
            auth: { type: "noauth" },
          },
        ],
      }],
      environments: [{
        id: "env1", name: "Local",
        variables: [
          { key: "token", value: "env-token", enabled: true },
          { key: "userId", value: "42", enabled: true },
          { key: "shared", value: "environment", enabled: true },
        ],
      }],
    }],
    ...extra,
  };
  return { doc, uploadFile };
}

async function withClient(doc, fn, extraEnv = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "postlite-mcp-"));
  const file = path.join(dir, "workspace.json");
  fs.writeFileSync(file, JSON.stringify(doc));
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [serverPath],
    env: { ...process.env, ...extraEnv, POSTLITE_WORKSPACE_FILE: file },
    stderr: "pipe",
  });
  const client = new Client({ name: "mcp-test", version: "0.0.0" });
  await client.connect(transport);
  const call = async (name, args = {}) => {
    const result = await client.callTool({ name, arguments: args });
    const text = result.content[0].text;
    return { isError: result.isError === true, text, data: result.isError ? null : JSON.parse(text) };
  };
  try {
    await fn(call);
  } finally {
    await client.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test("read tools expose workspaces, tree, variables and auth", async () => {
  const { server, base } = await captureServer();
  const { doc, uploadFile } = fixtureDoc(base);
  try {
    await withClient(doc, async (call) => {
      const workspaces = await call("list_workspaces");
      assert.deepEqual(workspaces.data, [{ id: "ws1", name: "Test WS", active: true, collections: 1, environments: 1 }]);

      const collections = await call("list_collections");
      const folder = collections.data.collections[0].items[0];
      assert.equal(folder.kind, "folder");
      assert.equal(folder.items[0].id, "r1");

      const req = await call("get_request", { requestId: "r1" });
      assert.equal(req.data.variables.shared, "folder", "folder beats environment beats collection");
      assert.equal(req.data.variables.token, "env-token", "environment beats collection");
      assert.equal(req.data.variables.host, base);
      assert.equal(req.data.resolvedAuth.type, "bearer");
      assert.deepEqual(req.data.path, ["API", "Users"]);

      const envs = await call("list_environments");
      assert.equal(envs.data.environments[0].variables.length, 3);

      const missing = await call("get_request", { requestId: "nope" });
      assert.equal(missing.isError, true);
    });
  } finally {
    server.close();
    fs.rmSync(uploadFile, { force: true });
  }
});

test("send_request resolves vars, path vars, query and inherited auth like the app", async () => {
  const { server, received, base } = await captureServer();
  const { doc, uploadFile } = fixtureDoc(base);
  try {
    await withClient(doc, async (call) => {
      const result = await call("send_request", { requestId: "r1" });
      assert.equal(result.data.response.error, null);
      assert.equal(result.data.response.status, 200);
      assert.equal(result.data.response.body, "ok");
      assert.equal(result.data.response.status_text, "OK");
      assert.equal(received[0].url, "/users/42?verbose=true");
      assert.equal(received[0].headers.authorization, "Bearer env-token");
      assert.equal(received[0].headers.accept, "application/json");

      const overridden = await call("send_request", { requestId: "r2", variables: { shared: "override" } });
      assert.equal(overridden.data.response.status, 200);
      assert.equal(received[1].body.toString(), "{\"name\":\"override\"}");
      assert.equal(received[1].headers["content-type"], "application/json");
      assert.equal(received[1].headers.authorization, undefined, "noauth must not inherit");

      const plain = await call("send_request", { requestId: "r2" });
      assert.equal(plain.data.response.error, null);
      assert.equal(received[2].body.toString(), "{\"name\":\"environment\"}", "no folder: environment wins over collection");
    });
  } finally {
    server.close();
    fs.rmSync(uploadFile, { force: true });
  }
});

test("multipart upload sends text and file parts with mime", async () => {
  const { server, received, base } = await captureServer();
  const { doc, uploadFile } = fixtureDoc(base);
  try {
    await withClient(doc, async (call) => {
      const result = await call("send_request", { requestId: "r3" });
      assert.equal(result.data.response.error, null);
      const sent = received[0];
      assert.match(sent.headers["content-type"], /^multipart\/form-data; boundary=/);
      const body = sent.body.toString();
      assert.ok(body.includes("name=\"note\"\r\n\r\nhello"), body);
      assert.ok(body.includes("name=\"upload\"; filename="), body);
      assert.ok(body.toLowerCase().includes("content-type: image/png"), body);
      assert.ok(body.includes("fake-png-bytes"), body);
    });
  } finally {
    server.close();
    fs.rmSync(uploadFile, { force: true });
  }
});

test("response cap truncates but reports full size", async () => {
  const { server, base } = await captureServer();
  const { doc, uploadFile } = fixtureDoc(base);
  doc.settings.response.maxResponseSizeKB = 1;
  try {
    await withClient(doc, async (call) => {
      const result = await call("send_http_request", { method: "GET", url: `${base}/big` });
      const response = result.data.response;
      assert.equal(response.status, 200);
      assert.equal(response.size_bytes, 3000);
      assert.equal(response.body.length, 1024);
      assert.equal(response.truncated, true);
    });
  } finally {
    server.close();
    fs.rmSync(uploadFile, { force: true });
  }
});

test("redirects follow up to the limit and stop when disabled", async () => {
  const { server, received, base } = await captureServer();
  const { doc, uploadFile } = fixtureDoc(base);
  try {
    await withClient(doc, async (call) => {
      const followed = await call("send_http_request", { method: "GET", url: `${base}/redirect` });
      assert.equal(followed.data.response.status, 200);
      assert.equal(followed.data.response.body, "ok");
      assert.deepEqual(received.map((r) => r.url), ["/redirect", "/final"]);
    });
    doc.settings.request.followRedirects = false;
    await withClient(doc, async (call) => {
      const manual = await call("send_http_request", { method: "GET", url: `${base}/redirect` });
      assert.equal(manual.data.response.status, 302);
    });
    doc.settings.request.followRedirects = true;
    doc.settings.request.maxRedirects = 0;
    await withClient(doc, async (call) => {
      const limited = await call("send_http_request", { method: "GET", url: `${base}/redirect` });
      assert.ok(limited.data.response.error, "zero redirects must error");
    });
  } finally {
    server.close();
    fs.rmSync(uploadFile, { force: true });
  }
});

test("disable_cache replaces cache headers and adhoc send posts a raw body", async () => {
  const { server, received, base } = await captureServer();
  const { doc, uploadFile } = fixtureDoc(base);
  doc.settings.request.disableCache = true;
  try {
    await withClient(doc, async (call) => {
      const result = await call("send_http_request", {
        method: "POST",
        url: `${base}/echo`,
        headers: { "Cache-Control": "max-age=600", "X-Test": "one" },
        body: "payload",
      });
      assert.equal(result.data.response.status, 200);
      assert.equal(received[0].body.toString(), "payload");
      assert.equal(received[0].headers["cache-control"], "no-cache");
      assert.equal(received[0].headers.pragma, "no-cache");
      assert.equal(received[0].headers["x-test"], "one");
    });
  } finally {
    server.close();
    fs.rmSync(uploadFile, { force: true });
  }
});

// undici's ProxyAgent tunnels through CONNECT, so the fake proxy must answer CONNECT
// and then serve the inner request on the socket itself.
function connectProxy() {
  const received = [];
  const server = http.createServer((req, res) => res.end("ok"));
  server.on("connect", (req, socket) => {
    socket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
    const chunks = [];
    socket.on("data", (chunk) => {
      chunks.push(chunk);
      const text = Buffer.concat(chunks).toString();
      if (text.includes("\r\n\r\n")) {
        received.push({ connect: req.url, requestLine: text.split("\r\n")[0] });
        socket.end("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
      }
    });
    socket.on("error", () => {});
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({ server, received, base: `http://127.0.0.1:${server.address().port}` });
    });
  });
}

test("manual proxy intercepts, bypass goes direct, environment mode reads HTTP_PROXY", async () => {
  const { server: target, received: targetReceived, base } = await captureServer();
  const { server: proxy, received: proxyReceived, base: proxyBase } = await connectProxy();
  const { doc, uploadFile } = fixtureDoc(base);
  try {
    doc.settings.request.proxy = { mode: "manual", url: proxyBase, bypass: "" };
    await withClient(doc, async (call) => {
      const result = await call("send_http_request", { method: "GET", url: "http://example.test/some/path" });
      assert.equal(result.data.response.error, null);
      assert.equal(result.data.response.status, 200);
      assert.equal(proxyReceived[0].connect, "example.test:80");
      assert.equal(proxyReceived[0].requestLine, "GET /some/path HTTP/1.1");
      assert.equal(targetReceived.length, 0);
    });
    doc.settings.request.proxy.bypass = "127.0.0.1";
    await withClient(doc, async (call) => {
      const result = await call("send_http_request", { method: "GET", url: `${base}/direct` });
      assert.equal(result.data.response.status, 200);
      assert.equal(targetReceived[0].url, "/direct");
      assert.equal(proxyReceived.length, 1, "bypassed host must not reach the proxy");
    });
    doc.settings.request.proxy = { mode: "environment", url: "", bypass: "" };
    await withClient(doc, async (call) => {
      const result = await call("send_http_request", { method: "GET", url: "http://example.test/env" });
      assert.equal(result.data.response.error, null);
      assert.equal(proxyReceived[1].connect, "example.test:80");
    }, { HTTP_PROXY: proxyBase, NO_PROXY: "" });
    await withClient(doc, async (call) => {
      const result = await call("send_http_request", { method: "GET", url: `${base}/noproxy` });
      assert.equal(result.data.response.note, "no proxy in environment");
      assert.equal(targetReceived[1].url, "/noproxy");
    }, { HTTP_PROXY: "", http_proxy: "", ALL_PROXY: "", all_proxy: "" });
  } finally {
    target.close();
    proxy.close();
    fs.rmSync(uploadFile, { force: true });
  }
});

test("missing workspace file is a clear tool error, adhoc send still works", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "postlite-mcp-"));
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [serverPath],
    env: { ...process.env, POSTLITE_WORKSPACE_FILE: path.join(dir, "absent.json") },
    stderr: "pipe",
  });
  const client = new Client({ name: "mcp-test", version: "0.0.0" });
  await client.connect(transport);
  try {
    const result = await client.callTool({ name: "list_workspaces", arguments: {} });
    assert.equal(result.isError, true);
    assert.match(result.content[0].text, /no workspace file/);
  } finally {
    await client.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});