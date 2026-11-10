// Loopback echo fixture for the Tier 2A execution harness. Runs as its own
// process so a blocking spawnSync in the parent cannot stall the server.
import { createServer } from "node:http";

const server = createServer((req, res) => {
  const chunks = [];
  req.on("data", (chunk) => chunks.push(chunk));
  req.on("end", () => {
    const hopByHop = new Set(["host", "connection", "content-length", "expect", "transfer-encoding"]);
    const echo = {
      method: req.method,
      url: req.url,
      headers: Object.fromEntries(Object.entries(req.headers).filter(([key]) => !hopByHop.has(key))),
      body: Buffer.concat(chunks).toString("utf8"),
    };
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ ok: true, echo }));
  });
});

server.listen(Number(process.env.PORT ?? 0), "127.0.0.1", () => {
  console.log(`PORT ${server.address().port}`);
});
