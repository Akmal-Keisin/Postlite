// Executes the generated artifacts against a live loopback fixture using the
// real toolchains on this machine. A target whose dependency is missing is
// syntax-checked and reported as SYNTAX, never as executed.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";

const codegen = createRequire(import.meta.url)("../ui/codegen.js");
const BASH = "C:\\Program Files\\Git\\bin\\bash.exe";
const dir = mkdtempSync(join(tmpdir(), "postlite-tier2a-execute-"));

// Git bash prints POSIX paths; spawn on Windows needs the native form.
const which = (name) => {
  const found = spawnSync(BASH, ["-lc", `p=$(command -v ${name}) && cygpath -w "$p"`], { encoding: "utf8" });
  return found.status === 0 ? found.stdout.trim() : null;
};

const node = which("node") ?? process.execPath;
const run = (command, args, options = {}) =>
  spawnSync(command, args, { encoding: "utf8", cwd: dir, timeout: 240_000, ...options });

// The fixture has its own process because spawnSync blocks this event loop.
const fixture = spawn(node, [join(import.meta.dirname, "tier2a-fixture.mjs")], {
  stdio: ["ignore", "pipe", "inherit"],
});
const port = await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("fixture did not report a port")), 10_000);
  fixture.stdout.setEncoding("utf8");
  fixture.stdout.on("data", (chunk) => {
    const found = /PORT (\d+)/.exec(chunk);
    if (found) {
      clearTimeout(timer);
      resolve(Number(found[1]));
    }
  });
});
const base = `http://127.0.0.1:${port}`;

const payload = { name: "rina", quote: "o'neil", slash: "a\\b", newline: "line\nnext" };

const request = {
  method: "POST",
  url: `${base}/users?page=2`,
  headers: [
    { key: "Content-Type", value: "application/json" },
    { key: "X-Test", value: "one" },
    { key: "Authorization", value: "Bearer tok" },
  ],
  body: JSON.stringify(payload),
  form: [],
  sslVerify: true,
};

const formRequest = {
  method: "PUT",
  url: `${base}/upload?x=1`,
  headers: [{ key: "X-Form", value: "yes" }],
  body: "",
  form: [{ key: "title", value: "a b" }, { key: "note", value: "c=d" }],
  sslVerify: true,
};

const echoOf = (stdout, label) => {
  const start = stdout.indexOf("{");
  assert.notEqual(start, -1, `${label} printed no JSON: ${stdout}`);
  return JSON.parse(stdout.slice(start)).echo;
};

const expectedBody = (echo, url = "/users?page=2") => {
  assert.equal(echo.method, "POST");
  assert.equal(echo.url, url);
  assert.equal(echo.headers["x-test"], "one");
  assert.equal(echo.headers.authorization, "Bearer tok");
  assert.equal(echo.headers["content-type"], "application/json");
  // Byte fidelity: what arrived must equal what was generated.
  assert.equal(echo.body, request.body);
  assert.deepEqual(JSON.parse(echo.body), payload);
};

const expectedForm = (echo) => {
  assert.equal(echo.method, "PUT");
  assert.equal(echo.url, "/upload?x=1");
  assert.equal(echo.headers["x-form"], "yes");
  assert.match(echo.headers["content-type"], /^multipart\/form-data; boundary=/);
  for (const needle of ['name="title"', "a b", 'name="note"', "c=d"])
    assert.ok(echo.body.includes(needle), `multipart body missing ${needle}`);
};

const report = [];
function verify(target, label, tool) {
  const code = codegen.toCode(request, target);
  const outcome = tool(code);
  if (outcome.state === "RUN") {
    assert.equal(outcome.result.status, 0, `${label} exited ${outcome.result.status}: ${outcome.result.stderr}`);
    // The exported cURL command is a request, not a script, so it prints no
    // status line; every other target prints one.
    if (target !== "curl")
      assert.match(outcome.result.stdout, /200/, `${label} did not print the status:\n${outcome.result.stdout}`);
    expectedBody(echoOf(outcome.result.stdout, label));
    report.push(`RUN    ${label}`);
  } else {
    assert.equal(outcome.result.status, 0, `${label} syntax check failed: ${outcome.result.stderr}`);
    report.push(`SYNTAX ${label}`);
  }
  const formCode = codegen.toCode(formRequest, target);
  assert.ok(!formCode.includes("undefined"), `${label} emitted an undefined value`);
  assert.ok(!/application\/json/.test(formCode.split("\n").filter((line) => /content-type/i.test(line)).join("\n")),
    `${label} kept a stale content-type on a multipart request`);
}

const write = (name, text) => {
  writeFileSync(join(dir, name), text, "utf8");
  return name;
};

const has = (command, args) => run(command, args, { timeout: 30_000 }).status === 0;

/* ----------------------------------------------------------------------- curl */

// Passing the command as an argument lets the Windows runtime rewrite its
// backslashes, so the exported command runs from a script file instead.
const runShell = (code) => run(BASH, [write("snippet_command.sh", code)]);

verify("curl", "Shell / cURL", (code) => ({
  state: "RUN",
  result: runShell(code),
}));

/* ----------------------------------------------------------------- javascript */

const axiosAvailable = has(node, ["-e", "import('axios').then(()=>process.exit(0),()=>process.exit(1))"]);
verify("js-fetch", "JavaScript / Fetch", (code) => ({
  state: "RUN",
  result: run(node, [write("snippet_fetch.mjs", code)]),
}));
verify("js-axios", `JavaScript / Axios${axiosAvailable ? "" : " (package not installed)"}`, (code) => {
  const file = write("snippet_axios.mjs", code);
  return axiosAvailable
    ? { state: "RUN", result: run(node, [file]) }
    : { state: "SYNTAX", result: run(node, ["--check", file]) };
});

/* ------------------------------------------------------------------------ php */

const php = which("php");
const guzzleAvailable = has(php, ["-r", "exit(class_exists('GuzzleHttp\\\\Client') ? 0 : 1);"]);
verify("php-curl", "PHP / cURL extension", (code) => ({
  state: "RUN",
  result: run(php, [write("snippet_curl.php", code)]),
}));
verify("php-guzzle", `PHP / Guzzle${guzzleAvailable ? "" : " (package not installed)"}`, (code) => {
  const file = write("snippet_guzzle.php", code);
  return guzzleAvailable
    ? { state: "RUN", result: run(php, [file]) }
    : { state: "SYNTAX", result: run(php, ["-l", file]) };
});

/* --------------------------------------------------------------------- python */

const python = which("python");
const requestsAvailable = has(python, ["-c", "import requests"]);
verify("py-requests", `Python / requests${requestsAvailable ? "" : " (package not installed)"}`, (code) => {
  const file = write("snippet_requests.py", code);
  return requestsAvailable
    ? { state: "RUN", result: run(python, [file]) }
    : { state: "SYNTAX", result: run(python, ["-m", "py_compile", file]) };
});
verify("py-urllib", "Python / urllib", (code) => ({
  state: "RUN",
  result: run(python, [write("snippet_urllib.py", code)]),
}));

/* ------------------------------------------------------------------------- go */

const go = which("go");
const goEnv = { ...process.env, GOPATH: join(dir, "gopath"), GOCACHE: join(dir, "gocache") };
const goRun = (name, code) => {
  const sub = join(dir, name);
  mkdirSync(sub, { recursive: true });
  writeFileSync(join(sub, "main.go"), code, "utf8");
  const mod = run(go, ["mod", "init", "postlitecheck"], { cwd: sub, env: goEnv });
  assert.equal(mod.status, 0, `go mod init: ${mod.stderr}`);
  return run(go, ["run", "."], { cwd: sub, env: goEnv });
};
verify("go-http", "Go / net/http", (code) => ({ state: "RUN", result: goRun("go-main", code) }));

/* ------------------------------------------------------------------ multipart */

const multipart = [
  ["curl", "bash", runShell],
  ["js-fetch", "node", (code) => run(node, [write("snippet_form.mjs", code)])],
  ["php-curl", "php", (code) => run(php, [write("snippet_form.php", code)])],
  ...(requestsAvailable ? [["py-requests", "python", (code) => run(python, [write("snippet_form.py", code)])]] : []),
  ["go-http", "go", (code) => goRun("go-form", code)],
];
for (const [target, name, execute] of multipart) {
  const result = execute(codegen.toCode(formRequest, target));
  assert.equal(result.status, 0, `${name} multipart failed: ${result.stderr}`);
  expectedForm(echoOf(result.stdout, name));
  console.log(`PASS ${name} multipart fields arrive as multipart/form-data`);
}

/* ----------------------------------------------------------------- self-hosted */

const insecure = { ...request, url: `${base}/insecure`, sslVerify: false };
const insecureCommand = codegen.toCurl(insecure);
assert.ok(insecureCommand.includes("--insecure"), "export lost --insecure");
assert.equal(runShell(insecureCommand).status, 0, "the exported command does not run");
console.log("PASS exported command carries --insecure only when ssl verification is off and still runs");

console.log(`\n${report.join("\n")}`);
const executed = report.filter((line) => line.startsWith("RUN")).length;
console.log(`PASS ${executed} targets executed live against the fixture, ${report.length - executed} syntax-checked because their package is missing`);
fixture.kill();
rmSync(dir, { recursive: true, force: true });
