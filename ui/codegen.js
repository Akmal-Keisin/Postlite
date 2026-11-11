// Request <-> text conversions: cURL commands and runnable code snippets.
// Pure module: no DOM and no app state, so the harness can require it in node.

const postliteText = (() => {
  /* ------------------------------------------------------------------- shell */

  const shellQuote = (value) => `'${String(value).split("'").join("'\\''")}'`;

  function toCurl(request) {
    const flags = [`--request ${request.method}`];
    if (request.sslVerify === false) flags.push("--insecure");
    flags.push(`--url ${shellQuote(request.url)}`);
    for (const header of request.headers ?? [])
      flags.push(`--header ${shellQuote(`${header.key}: ${header.value}`)}`);
    if (request.form?.length) {
      for (const field of request.form)
        flags.push(`--form ${shellQuote(`${field.key}=${field.value}`)}`);
    } else if (request.body) {
      flags.push(`--data-raw ${shellQuote(request.body)}`);
    }
    return [`curl ${flags[0]}`, ...flags.slice(1).map((flag) => `  ${flag}`)].join(" \\\n");
  }

  // role "ignore" means the flag is understood but belongs to curl itself.
  const FLAGS = new Map([
    ["-X", { arity: 1, role: "method" }], ["--request", { arity: 1, role: "method" }],
    ["--url", { arity: 1, role: "url" }],
    ["-H", { arity: 1, role: "header" }], ["--header", { arity: 1, role: "header" }],
    ["-d", { arity: 1, role: "data" }], ["--data", { arity: 1, role: "data" }],
    ["--data-raw", { arity: 1, role: "data" }], ["--data-binary", { arity: 1, role: "data" }],
    ["--data-ascii", { arity: 1, role: "data" }],
    ["--data-urlencode", { arity: 1, role: "data", note: "--data-urlencode sent as typed" }],
    ["-F", { arity: 1, role: "form" }], ["--form", { arity: 1, role: "form" }],
    ["-u", { arity: 1, role: "user" }], ["--user", { arity: 1, role: "user" }],
    ["-b", { arity: 1, role: "cookie" }], ["--cookie", { arity: 1, role: "cookie" }],
    ["-A", { arity: 1, role: "userAgent" }], ["--user-agent", { arity: 1, role: "userAgent" }],
    ["-e", { arity: 1, role: "referer" }], ["--referer", { arity: 1, role: "referer" }],
    ["-G", { arity: 0, role: "get" }], ["--get", { arity: 0, role: "get" }],
    ["-g", { arity: 0, role: "ignore" }], ["--globoff", { arity: 0, role: "ignore" }],
    ["-s", { arity: 0, role: "ignore" }], ["--silent", { arity: 0, role: "ignore" }],
    ["-S", { arity: 0, role: "ignore" }], ["--show-error", { arity: 0, role: "ignore" }],
    ["-L", { arity: 0, role: "ignore" }], ["--location", { arity: 0, role: "ignore" }],
    ["-k", { arity: 0, role: "ignore" }], ["--insecure", { arity: 0, role: "ignore" }],
    ["-i", { arity: 0, role: "ignore" }], ["--include", { arity: 0, role: "ignore" }],
    ["-I", { arity: 0, role: "ignore" }], ["--head", { arity: 0, role: "ignore" }],
    ["-v", { arity: 0, role: "ignore" }], ["--verbose", { arity: 0, role: "ignore" }],
    ["-f", { arity: 0, role: "ignore" }], ["--fail", { arity: 0, role: "ignore" }],
    ["--compressed", { arity: 0, role: "ignore" }],
    ["--location-trusted", { arity: 0, role: "ignore" }],
    ["-o", { arity: 1, role: "ignore" }], ["--output", { arity: 1, role: "ignore" }],
    ["-m", { arity: 1, role: "ignore" }], ["--max-time", { arity: 1, role: "ignore" }],
    ["--connect-timeout", { arity: 1, role: "ignore" }],
    ["-c", { arity: 1, role: "ignore", note: "cookie-jar flags are not applied" }], ["--cookie-jar", { arity: 1, role: "ignore", note: "cookie-jar flags are not applied" }],
    ["-x", { arity: 1, role: "ignore", note: "proxy flags are not applied" }], ["--proxy", { arity: 1, role: "ignore", note: "proxy flags are not applied" }],
    ["--cacert", { arity: 1, role: "ignore" }], ["--cert", { arity: 1, role: "ignore" }],
    ["--key", { arity: 1, role: "ignore" }], ["--resolve", { arity: 1, role: "ignore" }],
    ["--retry", { arity: 1, role: "ignore" }],
  ]);

  const tokenize = (text) => {
    const tokens = [];
    let token = "";
    let started = false;
    let index = 0;
    const flush = () => {
      if (started) tokens.push(token);
      token = "";
      started = false;
    };
    while (index < text.length) {
      const char = text[index];
      if (char === " " || char === "\t" || char === "\n" || char === "\r") {
        flush();
        index += 1;
        continue;
      }
      // An unquoted backslash before a newline is a shell line continuation.
      const continuation = /^\\\s*\n/.exec(text.slice(index));
      if (continuation) {
        flush();
        index += continuation[0].length;
        continue;
      }
      started = true;
      if (char === "'") {
        const end = text.indexOf("'", index + 1);
        if (end === -1) throw new Error("Unclosed single quote in the cURL command.");
        token += text.slice(index + 1, end);
        index = end + 1;
        continue;
      }
      if (char === '"') {
        index += 1;
        while (index < text.length && text[index] !== '"') {
          if (text[index] === "\\" && '"\\$`'.includes(text[index + 1])) {
            token += text[index + 1];
            index += 2;
            continue;
          }
          token += text[index];
          index += 1;
        }
        if (text[index] !== '"') throw new Error("Unclosed double quote in the cURL command.");
        index += 1;
        continue;
      }
      if (char === "\\" && index + 1 < text.length) {
        token += text[index + 1];
        index += 2;
        continue;
      }
      token += char;
      index += 1;
    }
    flush();
    return tokens;
  };

  // `-XPOST`, `-d{"a":1}`, `--header=X`, `--url X`, bundled `-sL`.
  function options(token) {
    if (token.startsWith("--")) {
      const split = token.search(/[=:]/);
      return split === -1 ? [[token, null]] : [[token.slice(0, split), token.slice(split + 1)]];
    }
    const name = token.slice(0, 2);
    if (!FLAGS.has(name)) return [[token, null]];
    if (FLAGS.get(name).arity === 1) return [[name, token.length > 2 ? token.slice(2) : null]];
    const bundled = [...token.slice(1)].map((char) => `-${char}`);
    if (bundled.some((flag) => FLAGS.get(flag)?.arity !== 0)) return [[token, null]];
    return bundled.map((flag) => [flag, null]);
  }

  function parseCurl(source) {
    const text = String(source)
      .replace(/\r\n/g, "\n")
      .split("\n")
      .filter((line) => !line.trim().startsWith("#"))
      .join("\n");
    const tokens = tokenize(text);
    if (!/curl(\.exe)?$/i.test(tokens[0] ?? "")) throw new Error("The command has to start with curl.");

    const found = {
      method: null, url: "", headers: [], data: [], form: [],
      user: "", cookie: "", userAgent: "", referer: "", get: false,
    };
    const ignored = [];
    const notes = [];
    const skip = new Set();
    const remember = (list, value) => { if (value && !list.includes(value)) list.push(value); };

    for (let index = 1; index < tokens.length; index += 1) {
      if (skip.has(index)) continue;
      const token = tokens[index];
      if (!token) continue;
      if (!token.startsWith("-") || token === "-") {
        if (!found.url) found.url = token;
        else remember(ignored, token);
        continue;
      }
      for (const [name, inline] of options(token)) {
        const spec = FLAGS.get(name);
        if (!spec) {
          remember(ignored, name);
          continue;
        }
        let value = "";
        if (spec.arity === 1) {
          value = inline ?? "";
          if (inline === null) {
            value = tokens[index + 1] ?? "";
            skip.add(index + 1);
            index += 1;
          }
        } else if (inline !== null) {
          remember(ignored, name);
          continue;
        }
        if (spec.note) remember(notes, spec.note);
        switch (spec.role) {
          case "method": found.method = value.toUpperCase().replace(/^-X/, ""); break;
          case "url": found.url = value; break;
          case "header": found.headers.push(value); break;
          case "data": found.data.push(value); break;
          case "form": found.form.push(value); break;
          case "user": found.user = value; break;
          case "cookie":
            // A value with "=" is a cookie string (becomes a manual Cookie header); anything else
            // is a filename, and reading a cookie file must not inject cookies into the workspace.
            if (value.includes("=")) found.cookie = value;
            else remember(notes, "cookie files are not imported");
            break;
          case "userAgent": found.userAgent = value; break;
          case "referer": found.referer = value; break;
          case "get": found.get = true; break;
          default: remember(ignored, name);
        }
      }
    }

    if (!found.url) throw new Error("The cURL command has no URL.");

    const headerValue = (name) =>
      found.headers.find((line) => line.split(":")[0].trim().toLowerCase() === name);
    const extra = [
      [found.cookie, "cookie", "Cookie"],
      [found.userAgent, "user-agent", "User-Agent"],
      [found.referer, "referer", "Referer"],
    ];
    for (const [value, lower, name] of extra)
      if (value && headerValue(lower) === undefined) found.headers.push(`${name}: ${value}`);

    const body = found.data.join("&");
    let url = found.url;
    if (found.get && body) url += (url.includes("?") ? "&" : "?") + body;
    if (found.get && !found.method) found.method = "GET";

    const hasContentType = headerValue("content-type") !== undefined;
    const [username, ...password] = found.user.split(":");

    const fileFields = found.form.filter((field) => field.slice(field.indexOf("=") + 1).startsWith("@"));
    if (fileFields.length) notes.push("file uploads are not imported");

    return {
      method: found.method ?? (body || found.form.length ? "POST" : "GET"),
      url,
      headers: found.headers.map((line) => {
        const colon = line.indexOf(":");
        return colon === -1
          ? { key: line.trim(), value: "" }
          : { key: line.slice(0, colon).trim(), value: line.slice(colon + 1).trim() };
      }),
      body: found.get ? "" : body,
      form: found.form
        .filter((field) => !field.slice(field.indexOf("=") + 1).startsWith("@"))
        .map((field) => {
          const equals = field.indexOf("=");
          return equals === -1
            ? { key: field, value: "" }
            : { key: field.slice(0, equals), value: field.slice(equals + 1) };
        }),
      contentType: body && !found.get && !hasContentType ? "application/x-www-form-urlencoded" : "",
      auth: found.user
        ? { type: "basic", basic: { username, password: password.join(":") } }
        : { type: "noauth" },
      ignored,
      notes,
    };
  }

  /* ---------------------------------------------------------------- languages */

  const json = (value) => JSON.stringify(String(value));
  const phpString = (value) => json(value).replace(/\$/g, "\\$");
  const phpPairs = (list) => list.map((h) => `${phpString(h.key)} => ${phpString(h.value)}`).join(", ");
  const goString = (value) => {
    const text = String(value);
    if (!text.includes("`") && !text.includes("\r")) return `\`${text}\``;
    const escaped = text
      .replace(/\\/g, "\\\\")
      .replace(/"/g, '\\"')
      .replace(/\n/g, "\\n")
      .replace(/\r/g, "\\r")
      .replace(/\t/g, "\\t");
    return `"${escaped}"`;
  };
  const rows = (list) => (list ?? []).map((item) => ({ ...item }));
  const withoutContentType = (list) => rows(list).filter((h) => h.key.toLowerCase() !== "content-type");
  const pairs = (list, quote) => list.map((h) => `${quote(h.key)}: ${quote(h.value)}`).join(", ");

  const CODE_TARGETS = [
    { id: "curl", label: "Shell / cURL" },
    { id: "js-fetch", label: "JavaScript / Fetch" },
    { id: "js-axios", label: "JavaScript / Axios" },
    { id: "php-curl", label: "PHP / cURL extension" },
    { id: "php-guzzle", label: "PHP / Guzzle" },
    { id: "py-requests", label: "Python / requests" },
    { id: "py-urllib", label: "Python / urllib (stdlib)" },
    { id: "go-http", label: "Go / net/http" },
  ];

  const join = (lines) => lines.join("\n").replace(/\n{3,}/g, "\n\n").trim();

  function jsFetch(request) {
    const multipart = request.form?.length;
    const headers = multipart ? withoutContentType(request.headers) : rows(request.headers);
    return join([
      `// ${request.method} ${request.url}`,
      multipart ? "// The client sets the multipart boundary." : "",
      `const headers = { ${pairs(rows(headers), json)} };`,
      multipart
        ? ["const form = new FormData();", ...request.form.map((f) => `form.append(${json(f.key)}, ${json(f.value)});`)].join("\n")
        : request.body ? `const body = ${json(request.body)};` : "",
      "const response = await fetch(",
      `  ${json(request.url)},`,
      "  {",
      `    method: ${json(request.method)},`,
      "    headers,",
      multipart ? "    body: form," : request.body ? "    body," : "",
      "  },",
      ");",
      "",
      "console.log(response.status);",
      "console.log(await response.text());",
    ]);
  }

  function jsAxios(request) {
    const multipart = request.form?.length;
    const headers = multipart ? withoutContentType(request.headers) : rows(request.headers);
    return join([
      `// ${request.method} ${request.url}`,
      "// Requires: npm install axios",
      "import axios from \"axios\";",
      "",
      `const headers = { ${pairs(rows(headers), json)} };`,
      multipart
        ? ["const form = new FormData();", ...request.form.map((f) => `form.append(${json(f.key)}, ${json(f.value)});`)].join("\n")
        : request.body ? `const body = ${json(request.body)};` : "",
      "const response = await axios({",
      `  method: ${json(request.method.toLowerCase())},`,
      `  url: ${json(request.url)},`,
      "  headers,",
      multipart ? "  data: form," : request.body ? "  data: body," : "",
      "});",
      "",
      "console.log(response.status);",
      "console.log(response.data);",
    ]);
  }

  function phpCurl(request) {
    const multipart = request.form?.length;
    const headers = multipart ? withoutContentType(request.headers) : rows(request.headers);
    return join([
      "<?php",
      `// ${request.method} ${request.url}`,
      `$headers = [${headers.map((h) => phpString(`${h.key}: ${h.value}`)).join(", ")}];`,
      multipart
        ? `$payload = [${request.form.map((f) => `${phpString(f.key)} => ${phpString(f.value)}`).join(", ")}];`
        : request.body ? `$payload = ${phpString(request.body)};` : "",
      "$handle = curl_init();",
      "curl_setopt_array($handle, [",
      `  CURLOPT_URL => ${phpString(request.url)},`,
      "  CURLOPT_RETURNTRANSFER => true,",
      `  CURLOPT_CUSTOMREQUEST => ${phpString(request.method)},`,
      "  CURLOPT_HTTPHEADER => $headers,",
      multipart || request.body ? "  CURLOPT_POSTFIELDS => $payload," : "",
      "]);",
      "$body = curl_exec($handle);",
      "$status = curl_getinfo($handle, CURLINFO_HTTP_CODE);",
      "if ($body === false) {",
      "  fwrite(STDERR, curl_error($handle) . PHP_EOL);",
      "  exit(1);",
      "}",
      "curl_close($handle);",
      "echo $status . PHP_EOL;",
      "echo $body;",
    ]);
  }

  function phpGuzzle(request) {
    const multipart = request.form?.length;
    const headers = multipart ? withoutContentType(request.headers) : rows(request.headers);
    return join([
      "<?php",
      `// ${request.method} ${request.url}`,
      "// Requires: composer require guzzlehttp/guzzle",
      "require __DIR__ . '/vendor/autoload.php';",
      "",
      "$client = new \\GuzzleHttp\\Client();",
      "$response = $client->request(",
      `  ${phpString(request.method)},`,
      `  ${phpString(request.url)},`,
      "  [",
      `    'headers' => [${phpPairs(headers)}],`,
      multipart
        ? `    'multipart' => [${request.form.map((f) => `['name' => ${phpString(f.key)}, 'contents' => ${phpString(f.value)}]`).join(", ")}],`
        : request.body ? `    'body' => ${phpString(request.body)},` : "",
      "  ]",
      ");",
      "echo $response->getStatusCode() . PHP_EOL;",
      "echo $response->getBody();",
    ]);
  }

  function pyRequests(request) {
    const multipart = request.form?.length;
    const headers = multipart ? withoutContentType(request.headers) : rows(request.headers);
    return join([
      `# ${request.method} ${request.url}`,
      "# Requires: pip install requests",
      "import requests",
      "",
      `url = ${json(request.url)}`,
      `headers = { ${pairs(rows(headers), json)} }`,
      multipart
        ? `files = { ${request.form.map((f) => `${json(f.key)}: (None, ${json(f.value)})`).join(", ")} }`
        : request.body ? `body = ${json(request.body)}` : "",
      "",
      "response = requests.request(",
      `  ${json(request.method)},`,
      "  url,",
      "  headers=headers,",
      multipart ? "  files=files," : request.body ? "  data=body," : "",
      ")",
      "",
      "print(response.status_code)",
      "print(response.text)",
    ]);
  }

  function pyUrllib(request) {
    const multipart = request.form?.length;
    const headers = rows(request.headers);
    if (multipart) {
      const existing = headers.find((h) => h.key.toLowerCase() === "content-type");
      if (existing) existing.value = "application/x-www-form-urlencoded";
      else headers.push({ key: "Content-Type", value: "application/x-www-form-urlencoded" });
    }
    return join([
      `# ${request.method} ${request.url}`,
      multipart ? "# urllib cannot build a multipart body, so these fields go out url-encoded." : "",
      "import urllib.parse",
      "import urllib.request",
      "",
      `url = ${json(request.url)}`,
      `headers = { ${pairs(headers, json)} }`,
      multipart
        ? `body = urllib.parse.urlencode(${json(Object.fromEntries(request.form.map((f) => [f.key, f.value])))}).encode()`
        : request.body ? `body = ${json(request.body)}.encode()` : "",
      "",
      `request = urllib.request.Request(url, method=${json(request.method)}${multipart || request.body ? ", data=body" : ""})`,
      "for name, value in headers.items():",
      "    request.add_header(name, value)",
      "",
      "with urllib.request.urlopen(request) as response:",
      "    print(response.status)",
      "    print(response.read().decode())",
    ]);
  }

  function goHttp(request) {
    const multipart = request.form?.length;
    const headers = multipart ? withoutContentType(request.headers) : rows(request.headers);
    const withBody = multipart || Boolean(request.body);
    const imports = ["fmt", "io", "net/http", "time"];
    if (withBody) imports.push("bytes");
    if (multipart) imports.push("mime/multipart");
    return join([
      `// ${request.method} ${request.url}`,
      "package main",
      "",
      "import (",
      ...imports.sort().map((name) => JSON.stringify(name)),
      ")",
      "",
      "func main() {",
      multipart
        ? ["  body := &bytes.Buffer{}",
            "  writer := multipart.NewWriter(body)",
            ...request.form.map((f) => `  writer.WriteField(${goString(f.key)}, ${goString(f.value)})`),
            "  writer.Close()"].join("\n")
        : request.body ? `  body := bytes.NewBufferString(${goString(request.body)})` : "",
      "",
      `  request, err := http.NewRequest(${goString(request.method)}, ${goString(request.url)}, ${withBody ? "body" : "nil"})`,
      "  if err != nil {",
      "    panic(err)",
      "  }",
      ...headers.map((h) => `  request.Header.Set(${goString(h.key)}, ${goString(h.value)})`),
      multipart ? "  request.Header.Set(\"Content-Type\", writer.FormDataContentType())" : "",
      "",
      "  client := &http.Client{Timeout: 60 * time.Second}",
      "  response, err := client.Do(request)",
      "  if err != nil {",
      "    panic(err)",
      "  }",
      "  defer response.Body.Close()",
      "",
      "  payload, _ := io.ReadAll(response.Body)",
      "  fmt.Println(response.Status)",
      "  fmt.Println(string(payload))",
      "}",
    ]);
  }

  const EMITTERS = {
    "js-fetch": jsFetch,
    "js-axios": jsAxios,
    "php-curl": phpCurl,
    "php-guzzle": phpGuzzle,
    "py-requests": pyRequests,
    "py-urllib": pyUrllib,
    "go-http": goHttp,
  };

  function toCode(request, target) {
    if (target === "curl") return toCurl(request);
    const emit = EMITTERS[target];
    if (!emit) throw new Error(`Unknown code target: ${target}`);
    return emit(request);
  }

  return { toCurl, parseCurl, toCode, CODE_TARGETS };
})();

if (typeof window !== "undefined") window.postliteText = postliteText;
if (typeof module !== "undefined") module.exports = postliteText;
