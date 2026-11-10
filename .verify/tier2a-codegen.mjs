// Unit checks for ui/codegen.js: cURL quoting, parsing, and round-trip fidelity.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
const codegen = createRequire(import.meta.url)("../ui/codegen.js");

let count = 0;
const check = (name, fn) => {
  fn();
  count += 1;
  console.log(`PASS ${name}`);
};

const request = {
  method: "POST",
  url: "https://api.test/users?page=2",
  headers: [
    { key: "Content-Type", value: "application/json" },
    { key: "Authorization", value: "Bearer tok" },
  ],
  body: '{"name":"rina"}',
  form: [],
  sslVerify: true,
};

check("curl export emits method, url, headers and raw body", () => {
  assert.equal(codegen.toCurl(request), [
    "curl --request POST \\",
    "  --url 'https://api.test/users?page=2' \\",
    "  --header 'Content-Type: application/json' \\",
    "  --header 'Authorization: Bearer tok' \\",
    "  --data-raw '{\"name\":\"rina\"}'",
  ].join("\n"));
});

check("curl export escapes a single quote in a value", () => {
  const text = codegen.toCurl({ ...request, body: "it's" });
  assert.match(text, /--data-raw 'it'\\''s'/);
});

check("curl export adds --insecure only when ssl verification is off", () => {
  assert.ok(codegen.toCurl({ ...request, sslVerify: false }).includes("--insecure"));
  assert.ok(!codegen.toCurl({ ...request, sslVerify: true }).includes("--insecure"));
});

check("curl export uses --form for form fields and no body otherwise", () => {
  const text = codegen.toCurl({ ...request, form: [{ key: "a", value: "b c" }], body: "" });
  assert.ok(text.includes("--form 'a=b c'"));
  assert.ok(!text.includes("--data-raw"));
  assert.ok(!codegen.toCurl({ ...request, body: "" }).includes("--data"));
});

check("curl parse reads a command exported by postlite", () => {
  const parsed = codegen.parseCurl(codegen.toCurl(request));
  assert.equal(parsed.method, "POST");
  assert.equal(parsed.url, request.url);
  assert.deepEqual(parsed.headers, request.headers);
  assert.equal(parsed.body, request.body);
  assert.equal(parsed.contentType, "");
  assert.deepEqual(parsed.ignored, []);
});

check("curl parse handles continuations, comments and double quotes", () => {
  const parsed = codegen.parseCurl(`# fetch users
curl -X POST \\
  'https://api.test/users' \\
  -H "Content-Type: application/json" \\
  --header='X-Test: one' \\
  -d '{"a": 1}'`);
  assert.equal(parsed.method, "POST");
  assert.equal(parsed.url, "https://api.test/users");
  assert.deepEqual(parsed.headers, [
    { key: "Content-Type", value: "application/json" },
    { key: "X-Test", value: "one" },
  ]);
  assert.equal(parsed.body, '{"a": 1}');
});

check("curl parse infers POST for data and GET without it", () => {
  assert.equal(codegen.parseCurl("curl https://a.test -d 'x=1'").method, "POST");
  assert.equal(codegen.parseCurl("curl https://a.test").method, "GET");
  assert.equal(codegen.parseCurl("curl -I https://a.test").method, "GET");
});

check("curl parse joins repeated data as a urlencoded body", () => {
  const parsed = codegen.parseCurl("curl https://a.test -d 'a=1' -d 'b=2'");
  assert.equal(parsed.body, "a=1&b=2");
  assert.equal(parsed.contentType, "application/x-www-form-urlencoded");
  assert.equal(
    codegen.parseCurl("curl https://a.test -d 'a=1' -H 'Content-Type: text/plain'").contentType,
    "");
});

check("curl parse maps auth, cookie, agent and referer flags", () => {
  const parsed = codegen.parseCurl(
    "curl https://a.test -u 'user:pass:word' -b 'sid=7' -A 'postlite/1' -e 'https://ref.test'");
  assert.deepEqual(parsed.auth, { type: "basic", basic: { username: "user", password: "pass:word" } });
  assert.deepEqual(parsed.headers, [
    { key: "Cookie", value: "sid=7" },
    { key: "User-Agent", value: "postlite/1" },
    { key: "Referer", value: "https://ref.test" },
  ]);
});

check("curl parse does not duplicate a header the command already sets", () => {
  const parsed = codegen.parseCurl("curl https://a.test -H 'Cookie: a=b' -b 'c=d'");
  assert.deepEqual(parsed.headers, [{ key: "Cookie", value: "a=b" }]);
});

check("curl parse turns form fields into a postlite form body", () => {
  const parsed = codegen.parseCurl("curl https://a.test -F 'a=b' --form 'c=d=e'");
  assert.deepEqual(parsed.form, [{ key: "a", value: "b" }, { key: "c", value: "d=e" }]);
  assert.equal(parsed.body, "");
});

check("curl parse reports file uploads instead of inventing content", () => {
  const parsed = codegen.parseCurl("curl https://a.test -F 'doc=@/tmp/report.pdf' -F 'a=b'");
  assert.deepEqual(parsed.form, [{ key: "a", value: "b" }]);
  assert.deepEqual(parsed.notes, ["file uploads are not imported"]);
});

check("curl parse moves data into the query for --get", () => {
  const parsed = codegen.parseCurl("curl 'https://a.test?x=1' -G -d 'a=2'");
  assert.equal(parsed.method, "GET");
  assert.equal(parsed.url, "https://a.test?x=1&a=2");
  assert.equal(parsed.body, "");
});

check("curl parse lists ignored and unknown options", () => {
  const parsed = codegen.parseCurl("curl -sL --compressed --resolve a.test:443:1.2.3.4 -Z --max-time 5 https://a.test");
  assert.equal(parsed.url, "https://a.test");
  assert.deepEqual(parsed.ignored, ["-s", "-L", "--compressed", "--resolve", "-Z", "--max-time"]);
});

check("curl parse rejects input that is not a curl command or has no url", () => {
  assert.throws(() => codegen.parseCurl("wget https://a.test"), /start with curl/);
  assert.throws(() => codegen.parseCurl("curl -H 'a: b'"), /no URL/);
  assert.throws(() => codegen.parseCurl("curl 'unbalanced"), /Unclosed single quote/);
});

check("curl round-trip keeps params, headers and body for every method", () => {
  for (const method of ["GET", "PUT", "PATCH", "DELETE", "QUERY"]) {
    const source = { ...request, method, body: method === "GET" ? "" : "x" };
    const parsed = codegen.parseCurl(codegen.toCurl(source));
    assert.equal(parsed.method, method);
    assert.equal(parsed.url, source.url);
    assert.equal(parsed.body, source.body);
    assert.deepEqual(parsed.headers, source.headers);
  }
});

const targets = {
  "js-fetch": ["const response = await fetch(", "method: \"POST\""],
  "js-axios": ["import axios from \"axios\"", "method: \"post\""],
  "php-curl": ["<?php", "CURLOPT_CUSTOMREQUEST => \"POST\"", "curl_setopt_array"],
  "php-guzzle": ["new \\GuzzleHttp\\Client()", "'headers' => ["],
  "py-requests": ["import requests", "requests.request("],
  "py-urllib": ["import urllib.request", "urllib.request.Request(url, method=\"POST\""],
  "go-http": ["package main", "net/http", "http.NewRequest(`POST`"],
};

check("every code target names the url, headers and body", () => {
  for (const [target, needles] of Object.entries(targets)) {
    const code = codegen.toCode(request, target);
    for (const needle of needles)
      assert.ok(code.includes(needle), `${target} missing ${needle}`);
    assert.ok(code.includes("Bearer tok"), `${target} lost the auth header`);
    assert.ok(code.includes("rina"), `${target} lost the body`);
  }
});

check("code targets keep an embedded quote in the body", () => {
  const awkward = { ...request, body: "it\"s a \\ test\nend" };
  for (const target of Object.keys(targets)) {
    const code = codegen.toCode(awkward, target);
    assert.ok(code.includes("it") && code.includes("end"), `${target} dropped body text`);
  }
});

check("multipart targets replace the content-type header", () => {
  const formRequest = { ...request, body: "", form: [{ key: "a", value: "b" }] };
  for (const target of Object.keys(targets)) {
    const code = codegen.toCode(formRequest, target);
    const lines = code.split("\n").filter((line) => /application\/json/.test(line));
    assert.deepEqual(lines, [], `${target} kept a stale content-type`);
  }
});

check("an unknown target is an error, not an empty file", () => {
  assert.throws(() => codegen.toCode(request, "ruby-http"), /Unknown code target/);
});

console.log(`PASS ${count} codegen checks`);
