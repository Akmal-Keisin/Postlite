import { createServer } from "node:http";
import { createServer as createHttpsServer } from "node:https";
import { readFileSync, mkdtempSync, unlinkSync, rmdirSync } from "node:fs";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";

const root = resolve(import.meta.dirname, "../ui");
const exposed = "state,DEFAULT_SETTINGS,normalizeSettings,newRequest,newFolder,newCollection,newEnvironment,toPostmanCollection,fromPostman,toPostmanEnvironment,fromPostmanEnvironment,exportSettings,importSettings,applyAppSettings,flushWorkspace,persist,workspacePayload,addTab,enterWorkspace,switchWorkspace,deleteEnvironment,responseCopyText,render,renderResponse,renderVarOverlay,send,openAppSettings";
const handler = (req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  if (url.pathname === "/api/redirect") { res.writeHead(302, { location: "/api/echo" }); res.end(); return; }
  if (url.pathname === "/api/loop") { res.writeHead(302, { location: "/api/loop" }); res.end(); return; }
  if (url.pathname === "/api/large") { res.end("x".repeat(4096)); return; }
  if (url.pathname === "/api/slow") { setTimeout(() => res.end("slow"), 250); return; }
  if (url.pathname === "/api/echo") {
    const chunks = [];
    req.on("data", c => chunks.push(c));
    req.on("end", () => {
      res.writeHead(200, { "content-type": "application/json", "x-verify": "tier1", "authorization": "synthetic-auth", "set-cookie": "synthetic-cookie=1", "x-api-key": "synthetic-key" });
      res.end(JSON.stringify({ method: req.method, headers: req.headers, body: Buffer.concat(chunks).toString() }));
    });
    return;
  }
  const file = { "/": "index.html", "/index.html": "index.html", "/app.js": "app.js", "/styles.css": "styles.css" }[url.pathname];
  if (!file) { res.writeHead(404); res.end(); return; }
  let source = readFileSync(resolve(root, file), "utf8");
  if (file === "app.js") source += `\nwindow.__verify = {${exposed}};\n`;
  res.writeHead(200, { "content-type": file.endsWith("js") ? "text/javascript" : file.endsWith("css") ? "text/css" : "text/html", "cache-control": "no-store" });
  res.end(source);
};
const temporary = mkdtempSync(join(tmpdir(), "postlite-tier1-tls-"));
const key = join(temporary, "key.pem");
const cert = join(temporary, "cert.pem");
const generated = spawnSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", key, "-out", cert, "-days", "1", "-subj", "/CN=localhost"], { encoding: "utf8" });
if (generated.status !== 0) throw new Error(`OpenSSL is required for local TLS verification: ${generated.error?.message ?? generated.stderr}`);
const http = createServer(handler).listen(8100, "127.0.0.1", () => console.log("Tier 1 UI and HTTP fixtures: http://127.0.0.1:8100"));
const https = createHttpsServer({ key: readFileSync(key), cert: readFileSync(cert) }, handler).listen(8101, "127.0.0.1", () => console.log("Tier 1 self-signed TLS fixture: port 8101"));
for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => { http.close(); https.close(); });
process.once("exit", () => { unlinkSync(key); unlinkSync(cert); rmdirSync(temporary); });
