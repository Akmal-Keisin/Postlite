import { createServer } from "node:http";
import { createServer as createHttpsServer } from "node:https";
import { readFileSync, mkdtempSync, unlinkSync, rmdirSync } from "node:fs";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";

const root = resolve(import.meta.dirname, "../ui");
const exposed = "state,DEFAULT_SETTINGS,normalizeSettings,newRequest,newFolder,newCollection,newEnvironment,newWorkspace,toPostmanCollection,fromPostman,toPostmanEnvironment,fromPostmanEnvironment,exportSettings,importSettings,applyAppSettings,flushWorkspace,persist,workspacePayload,addTab,enterWorkspace,switchWorkspace,deleteEnvironment,deleteWorkspace,responseCopyText,render,renderResponse,renderVarOverlay,send,openAppSettings,exportTarget,openDrawer,closeDrawer,openCurlDialog,renderCodeOutput,importCurlCommand,exportBackup,openRestoreDialog,confirmRestore,applySavedPayload,validateBackup,describePayload,notify,loadCookies,renderCookies,refreshCookiesIfVisible,deleteCookieFilter,refreshProxyControls,loadProxyCredentials,syncCookiePersist";
const handler = (req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  if (url.pathname === "/api/redirect") { res.writeHead(302, { location: "/api/echo" }); res.end(); return; }
  if (url.pathname === "/api/loop") { res.writeHead(302, { location: "/api/loop" }); res.end(); return; }
  if (url.pathname === "/api/large") { res.end("x".repeat(4096)); return; }
  if (url.pathname === "/api/slow") { setTimeout(() => res.end("slow"), 250); return; }
  // Cookie jar fixtures (Tier 2B P7/P9). /set plants a basket exercising the session/persistent
  // split, an expired delete, Secure, HttpOnly and SameSite; ?stage=seed first plants a cookie the
  // default basket later deletes, so the logout path is provable. /echo reports what the jar sent.
  if (url.pathname === "/set" || url.pathname === "/api/set") {
    // seed plants one persistent cookie that the default basket later deletes with an expired
    // Set-Cookie, so the logout path is provable across two real requests.
    if (url.searchParams.get("stage") === "seed") {
      res.writeHead(200, { "content-type": "application/json", "set-cookie": ["gone=old; Path=/; Max-Age=3600"] });
      res.end(JSON.stringify({ set: 1, stage: "seed", host: req.headers.host }));
      return;
    }
    const cookies = [
      "sid=sess-7f3a; Path=/; HttpOnly; SameSite=Lax",
      "theme=dark; Path=/; Max-Age=3600; SameSite=Strict",
      "gone=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
      "secure_tok=sec-99; Path=/; Secure; SameSite=None; Max-Age=3600",
      "api_scope=1; Path=/api; Max-Age=3600",
    ];
    res.writeHead(200, { "content-type": "application/json", "set-cookie": cookies });
    res.end(JSON.stringify({ set: cookies.length, host: req.headers.host }));
    return;
  }
  if (url.pathname === "/echo" || url.pathname === "/api/echo") {
    const chunks = [];
    req.on("data", c => chunks.push(c));
    req.on("end", () => {
      const legacy = url.pathname === "/api/echo";
      const headers = { "content-type": "application/json", "x-verify": "tier1" };
      if (legacy) Object.assign(headers, { "authorization": "synthetic-auth", "set-cookie": "synthetic-cookie=1", "x-api-key": "synthetic-key" });
      res.writeHead(200, headers);
      res.end(JSON.stringify({ method: req.method, url: req.url, cookie: req.headers.cookie ?? "", headers: req.headers, body: Buffer.concat(chunks).toString() }));
    });
    return;
  }
  const file = { "/": "index.html", "/index.html": "index.html", "/app.js": "app.js", "/codegen.js": "codegen.js", "/styles.css": "styles.css" }[url.pathname];
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
const http = createServer(handler).listen(8100, "::", () => console.log("Tier 1 UI and HTTP fixtures: http://127.0.0.1:8100"));
const https = createHttpsServer({ key: readFileSync(key), cert: readFileSync(cert) }, handler).listen(8101, "::", () => console.log("Tier 1 self-signed TLS fixture: port 8101"));
for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => { http.close(); https.close(); });
process.once("exit", () => { unlinkSync(key); unlinkSync(cert); rmdirSync(temporary); });
