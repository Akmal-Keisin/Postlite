import { createServer } from "node:http";

createServer((req, res) => {
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    res.writeHead(200, { "content-type": "application/json", "x-verify": "postlite" });
    res.end(
      JSON.stringify({
        path: req.url,
        method: req.method,
        headers: req.headers,
        body: Buffer.concat(chunks).toString(),
      })
    );
  });
}).listen(8099, "127.0.0.1", () => console.log("verify server on 8099"));
