// A static file server for the browser tests and benchmarks: serves the
// repository root (dist/, js/), plus extra routes (`mounts`: URL path ->
// file or directory) and a POST endpoint collecting results (`/result`).
import { createReadStream, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve, sep } from "node:path";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json",
  ".wasm": "application/wasm",
  ".pdf": "application/pdf",
};

/** Starts the server on a free port; `onResult(body)` receives POSTs to /result. */
export function startServer(root, { mounts = {}, onResult, port = 0 } = {}) {
  root = resolve(root);
  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://localhost");
    if (req.method === "POST" && url.pathname === "/result") {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        res.writeHead(204, { "access-control-allow-origin": "*" }).end();
        onResult?.(body);
      });
      return;
    }
    let path = decodeURIComponent(url.pathname);
    let file;
    const mount = Object.keys(mounts)
      .sort((a, b) => b.length - a.length)
      .find((m) => path === m || path.startsWith(m.endsWith("/") ? m : `${m}/`));
    if (mount) file = join(mounts[mount], path.slice(mount.length));
    else file = join(root, normalize(path));
    file = resolve(file);
    const allowed = [root, ...Object.values(mounts).map((m) => resolve(m))];
    if (!allowed.some((a) => file === a || file.startsWith(a + sep))) {
      res.writeHead(403).end();
      return;
    }
    let st;
    try {
      st = statSync(file);
    } catch {
      res.writeHead(404).end();
      return;
    }
    if (st.isDirectory()) {
      res.writeHead(404).end();
      return;
    }
    res.writeHead(200, {
      "content-type": TYPES[extname(file)] ?? "application/octet-stream",
      "content-length": st.size,
      "cache-control": "no-store",
    });
    createReadStream(file).pipe(res);
  });
  return new Promise((ok) =>
    server.listen(port, "127.0.0.1", () => ok({ server, url: `http://127.0.0.1:${server.address().port}` })),
  );
}
