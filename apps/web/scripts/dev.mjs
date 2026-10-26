// Dev server: one origin for UI and API, like Caddy in production.
// `next dev` runs on PORT+1; this proxy serves PORT, sending /api to the api and the rest to Next.
// Next's own rewrites cannot do this: they replace Host, and the api rejects mutations whose
// Origin differs from Host. Dev-only; static export never sees it.
import { spawn } from "node:child_process";
import http from "node:http";
import net from "node:net";

const port = Number(process.env.PORT ?? 3000);
const nextPort = port + 1;
const api = new URL(process.env.BF_API_URL ?? "http://localhost:8080");

const next = spawn("pnpm", ["exec", "next", "dev", "-p", String(nextPort)], { stdio: "inherit" });
next.on("exit", (code) => process.exit(code ?? 0));
process.on("SIGINT", () => next.kill("SIGINT"));
process.on("SIGTERM", () => next.kill("SIGTERM"));

const server = http.createServer((req, res) => {
  const target = req.url?.startsWith("/api/") ? { host: api.hostname, port: api.port } : { host: "localhost", port: nextPort };
  const upstream = http.request({ ...target, method: req.method, path: req.url, headers: req.headers }, (up) => {
    res.writeHead(up.statusCode ?? 502, up.headers);
    up.pipe(res);
  });
  upstream.on("error", () => res.writeHead(502).end("upstream unavailable"));
  req.pipe(upstream);
});

// HMR websocket for Next.
server.on("upgrade", (req, socket, head) => {
  const upstream = net.connect(nextPort, "localhost", () => {
    const headers = req.rawHeaders.map((h, i) => (i % 2 ? `${h}\r\n` : `${h}: `)).join("");
    upstream.write(`${req.method} ${req.url} HTTP/1.1\r\n${headers}\r\n`);
    upstream.write(head);
    socket.pipe(upstream).pipe(socket);
  });
  upstream.on("error", () => socket.destroy());
  socket.on("error", () => upstream.destroy());
});

server.listen(port, () => console.log(`bunfiiliate web: http://localhost:${port} (api: ${api.origin})`));
