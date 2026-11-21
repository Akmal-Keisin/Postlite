import fs from "node:fs/promises";
import path from "node:path";
import { STATUS_CODES } from "node:http";
import { Agent, ProxyAgent, interceptors, request as undiciRequest } from "undici";

const MIME_BY_EXT = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif",
  webp: "image/webp", svg: "image/svg+xml", pdf: "application/pdf", zip: "application/zip",
  json: "application/json", csv: "text/csv", txt: "text/plain", html: "text/html",
  htm: "text/html", xml: "application/xml", mp3: "audio/mpeg", mp4: "video/mp4",
};

function filePartMeta(filePath) {
  const name = path.basename(filePath) || "file";
  const ext = path.extname(filePath).slice(1).toLowerCase();
  return { name, mime: MIME_BY_EXT[ext] ?? "application/octet-stream" };
}

function bypassed(hostname, bypassList) {
  for (const raw of String(bypassList ?? "").split(",")) {
    const entry = raw.trim().toLowerCase().replace(/^\./, "");
    if (!entry) continue;
    if (entry === "*") return true;
    const host = hostname.toLowerCase();
    if (host === entry || host.endsWith(`.${entry}`)) return true;
  }
  return false;
}

// Same lookup order as resolve_proxy() in the Rust backend: uppercase then lowercase,
// scheme-specific first, ALL_PROXY as the fallback.
function resolveProxy(proxy, targetUrl) {
  if (!proxy || proxy.mode === "direct") return { proxy: null, note: null };
  if (proxy.mode === "manual") {
    if (!String(proxy.url ?? "").trim()) throw new Error("proxy url is required in manual mode");
    return { proxy: { url: proxy.url, bypass: proxy.bypass ?? "" }, note: null };
  }
  if (proxy.mode === "environment") {
    const https = /^https:/i.test(targetUrl);
    const names = https
      ? ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
      : ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"];
    const found = names.map((name) => process.env[name]).find((value) => value && value.trim());
    if (!found) return { proxy: null, note: "no proxy in environment" };
    const bypass = ["NO_PROXY", "no_proxy"].map((name) => process.env[name]).find((value) => value && value.trim()) ?? "";
    return { proxy: { url: found, bypass }, note: null };
  }
  throw new Error(`unknown proxy mode: ${proxy.mode}`);
}

function dispatcherFor(proxy, tls) {
  if (!proxy) return new Agent({ connect: tls });
  const uri = new URL(proxy.url);
  const opts = { uri: `${uri.protocol}//${uri.host}`, connect: tls, requestTls: tls };
  if (uri.username) {
    const credentials = `${decodeURIComponent(uri.username)}:${decodeURIComponent(uri.password)}`;
    opts.token = `Basic ${Buffer.from(credentials, "utf8").toString("base64")}`;
  }
  return new ProxyAgent(opts);
}

const EMPTY = {
  status: 0, status_text: "", headers: [], body: "", truncated: false,
  elapsed_ms: 0, size_bytes: 0, error: null, note: null,
};

export async function executeRequest(prepared, settings) {
  const started = performance.now();
  const fail = (error) => ({ ...EMPTY, error: String(error), elapsed_ms: Math.round(performance.now() - started) });
  try {
    const timeoutMs = settings.requestTimeoutMs;
    const tls = { rejectUnauthorized: settings.sslVerify !== false };
    const { proxy, note } = resolveProxy(settings.proxy, prepared.url);
    let dispatcher = proxy && !bypassed(new URL(prepared.url).hostname, proxy.bypass)
      ? dispatcherFor(proxy, tls)
      : new Agent({ connect: tls });
    if (settings.followRedirects && settings.maxRedirects > 0) {
      dispatcher = dispatcher.compose(interceptors.redirect({ maxRedirections: settings.maxRedirects }));
    }

    let headers = prepared.headers.map((h) => [h.key, h.value]);
    if (settings.disableCache) {
      headers = headers.filter(([key]) => !["cache-control", "pragma"].includes(key.toLowerCase()));
      headers.push(["Cache-Control", "no-cache"], ["Pragma", "no-cache"]);
    }

    let body = null;
    if (prepared.form?.length) {
      // Built by hand because undici's extractBody rejects Node's global FormData (brand check).
      const boundary = `----postlite-mcp-${Math.random().toString(36).slice(2)}`;
      const escape = (text) => String(text).replace(/\n/g, "%0A").replace(/\r/g, "%0D").replace(/"/g, "%22");
      const parts = [];
      for (const field of prepared.form) {
        if (field.type === "file") {
          let bytes;
          try {
            bytes = await fs.readFile(field.value);
          } catch (err) {
            return fail(`could not read file for "${field.key}": ${err.message}`);
          }
          const { name, mime } = filePartMeta(field.value);
          parts.push(Buffer.from(
            `--${boundary}\r\nContent-Disposition: form-data; name="${escape(field.key)}"; filename="${escape(name)}"\r\nContent-Type: ${mime}\r\n\r\n`));
          parts.push(bytes, Buffer.from("\r\n"));
        } else {
          parts.push(Buffer.from(
            `--${boundary}\r\nContent-Disposition: form-data; name="${escape(field.key)}"\r\n\r\n${field.value}\r\n`));
        }
      }
      parts.push(Buffer.from(`--${boundary}--\r\n`));
      body = Buffer.concat(parts);
      headers = headers.filter(([key]) => key.toLowerCase() !== "content-type");
      headers.push(["Content-Type", `multipart/form-data; boundary=${boundary}`]);
    } else if (prepared.body) {
      body = prepared.body;
    }

    const response = await undiciRequest(prepared.url, {
      method: prepared.method,
      headers: headers.flat(),
      body,
      dispatcher,
      headersTimeout: timeoutMs,
      bodyTimeout: timeoutMs,
      signal: AbortSignal.timeout(timeoutMs),
    });

    // reqwest's limited(0) errors on any redirect; the undici interceptor would pass it through.
    if (settings.followRedirects && settings.maxRedirects === 0
      && [301, 302, 303, 307, 308].includes(response.statusCode) && response.headers.location) {
      return fail("redirect limit exceeded (max_redirects is 0)");
    }

    const cap = settings.maxResponseSizeKB * 1024;
    const kept = [];
    let keptBytes = 0;
    let sizeBytes = 0;
    for await (const chunk of response.body) {
      sizeBytes += chunk.length;
      if (keptBytes < cap) {
        const slice = chunk.subarray(0, cap - keptBytes);
        kept.push(slice);
        keptBytes += slice.length;
      }
    }

    const responseHeaders = [];
    for (const [key, value] of Object.entries(response.headers)) {
      for (const v of Array.isArray(value) ? value : [value]) {
        responseHeaders.push({ key, value: String(v) });
      }
    }

    return {
      status: response.statusCode,
      status_text: STATUS_CODES[response.statusCode] ?? "",
      headers: responseHeaders,
      body: Buffer.concat(kept).toString("utf8"),
      truncated: sizeBytes > cap,
      elapsed_ms: Math.round(performance.now() - started),
      size_bytes: sizeBytes,
      error: null,
      note,
    };
  } catch (err) {
    return fail(err?.message ?? err);
  }
}