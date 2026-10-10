"use strict";

const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");

const ROOT = __dirname;
const PUBLIC_DIR = path.join(ROOT, "deobfuscator-engine", "public");
const PORT = Number(process.env.PORT || 3000);
const MAX_LOADER_BYTES = 4 * 1024 * 1024;
const MAX_JSON_BYTES = 16 * 1024;
const LOADER_URL = /^https:\/\/api\.jnkie\.com\/api\/v1\/luascripts\/public\/[a-z0-9_-]{12,128}\/download$/i;

const MIME_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".wasm": "application/wasm",
  ".txt": "text/plain; charset=utf-8",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon"
};

function sendJson(res, statusCode, body) {
  const bytes = Buffer.from(JSON.stringify(body));
  res.writeHead(statusCode, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": String(bytes.length),
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff"
  });
  res.end(bytes);
}

function readJsonBody(req) {
  return new Promise((resolve, reject) => {
    let size = 0;
    const chunks = [];
    req.on("data", (chunk) => {
      size += chunk.length;
      if (size > MAX_JSON_BYTES) {
        reject(Object.assign(new Error("Corpo da solicitação muito grande."), { statusCode: 413 }));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => {
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}"));
      } catch {
        reject(Object.assign(new Error("JSON inválido."), { statusCode: 400 }));
      }
    });
    req.on("error", reject);
  });
}

async function getLoaderText(rawUrl) {
  let target;
  try {
    target = new URL(rawUrl);
  } catch {
    throw Object.assign(new Error("URL inválida."), { statusCode: 400 });
  }

  if (
    typeof rawUrl !== "string" ||
    target.protocol !== "https:" ||
    target.hostname.toLowerCase() !== "api.jnkie.com" ||
    target.port ||
    target.username ||
    target.password ||
    target.search ||
    target.hash ||
    !LOADER_URL.test(target.href)
  ) {
    throw Object.assign(new Error("Só são permitidas URLs públicas Jnkie no formato compatível."), { statusCode: 400 });
  }

  let remote;
  try {
    remote = await fetch(target.href, {
      method: "GET",
      redirect: "manual",
      headers: { Accept: "text/plain, application/octet-stream" },
      signal: AbortSignal.timeout(12000)
    });
  } catch (error) {
    const message = error && error.name === "TimeoutError"
      ? "A fonte remota demorou demais para responder."
      : "Não foi possível obter o texto da fonte remota.";
    throw Object.assign(new Error(message), { statusCode: 502 });
  }

  if (remote.status >= 300 && remote.status < 400) {
    await remote.body?.cancel();
    throw Object.assign(new Error("A fonte redirecionou para outro endereço; o redirecionamento foi bloqueado por segurança."), { statusCode: 502 });
  }
  if (!remote.ok) {
    await remote.body?.cancel();
    throw Object.assign(new Error("A fonte remota respondeu HTTP " + remote.status + "."), { statusCode: 502 });
  }

  const declaredSize = Number(remote.headers.get("content-length") || 0);
  if (declaredSize > MAX_LOADER_BYTES) {
    await remote.body?.cancel();
    throw Object.assign(new Error("O payload ultrapassa o limite de 4 MiB desta importação."), { statusCode: 413 });
  }

  const contentType = (remote.headers.get("content-type") || "").toLowerCase();
  if (contentType.includes("text/html")) {
    await remote.body?.cancel();
    throw Object.assign(new Error("A fonte respondeu com HTML, não com código-fonte."), { statusCode: 502 });
  }

  const reader = remote.body && remote.body.getReader();
  if (!reader) {
    throw Object.assign(new Error("A fonte remota não retornou um corpo legível."), { statusCode: 502 });
  }

  const chunks = [];
  let total = 0;
  try {
    while (true) {
      const part = await reader.read();
      if (part.done) break;
      total += part.value.byteLength;
      if (total > MAX_LOADER_BYTES) {
        await reader.cancel();
        throw Object.assign(new Error("O payload ultrapassa o limite de 4 MiB desta importação."), { statusCode: 413 });
      }
      chunks.push(Buffer.from(part.value));
    }
  } catch (error) {
    if (error && error.statusCode) throw error;
    throw Object.assign(new Error("Falha ao ler a resposta remota."), { statusCode: 502 });
  }

  if (total === 0) {
    throw Object.assign(new Error("A fonte remota retornou um arquivo vazio."), { statusCode: 502 });
  }

  let source;
  try {
    source = new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks, total));
  } catch {
    throw Object.assign(new Error("A resposta não é texto UTF-8 reconhecível."), { statusCode: 415 });
  }

  const prefix = source.trimStart().slice(0, 512).toLowerCase();
  if (!source.trim() || source.includes("\0") || prefix.startsWith("<!doctype html") || /<html[\s>]/i.test(prefix)) {
    throw Object.assign(new Error("A fonte devolveu HTML, conteúdo vazio ou texto inválido em vez de código Lua."), { statusCode: 502 });
  }
  return Buffer.from(source, "utf8");
}

async function handle(req, res) {
  res.setHeader("X-Content-Type-Options", "nosniff");
  res.setHeader("Referrer-Policy", "no-referrer");

  let pathname;
  try {
    pathname = new URL(req.url, "http://localhost").pathname;
  } catch {
    return sendJson(res, 400, { ok: false, error: "Caminho inválido." });
  }

  if (pathname === "/healthz") {
    if (req.method !== "GET" && req.method !== "HEAD") {
      res.setHeader("Allow", "GET, HEAD");
      return sendJson(res, 405, { ok: false, error: "Método não permitido." });
    }
    return sendJson(res, 200, { ok: true, service: "deobfuscator-rust-wasm" });
  }

  if (pathname === "/api/fetch-loader") {
    res.setHeader("Cache-Control", "no-store");
    if (req.method !== "POST") {
      res.setHeader("Allow", "POST");
      return sendJson(res, 405, { ok: false, error: "Use POST para solicitar o texto do loader." });
    }
    let body;
    try {
      body = await readJsonBody(req);
    } catch (error) {
      if (!res.headersSent && !res.writableEnded) {
        return sendJson(res, error.statusCode || 400, { ok: false, error: error.message || "Solicitação inválida." });
      }
      return;
    }

    try {
      const content = await getLoaderText(body.url);
      res.writeHead(200, {
        "Content-Type": "text/plain; charset=utf-8",
        "Content-Length": String(content.length),
        "Cache-Control": "no-store",
        "X-Content-Type-Options": "nosniff"
      });
      return res.end(content);
    } catch (error) {
      return sendJson(res, error.statusCode || 502, { ok: false, error: error.message || "Falha ao importar texto." });
    }
  }

  if (req.method !== "GET" && req.method !== "HEAD") {
    res.setHeader("Allow", "GET, HEAD");
    return sendJson(res, 405, { ok: false, error: "Método não permitido." });
  }

  let decodedPath;
  try {
    decodedPath = decodeURIComponent(pathname);
  } catch {
    return sendJson(res, 400, { ok: false, error: "Caminho inválido." });
  }
  if (decodedPath === "/") decodedPath = "/index.html";

  const filePath = path.resolve(PUBLIC_DIR, "." + decodedPath);
  if (filePath !== PUBLIC_DIR && !filePath.startsWith(PUBLIC_DIR + path.sep)) {
    return sendJson(res, 403, { ok: false, error: "Acesso negado." });
  }

  try {
    const stat = await fs.promises.stat(filePath);
    if (!stat.isFile()) return sendJson(res, 404, { ok: false, error: "Arquivo não encontrado." });
    const ext = path.extname(filePath).toLowerCase();
    res.writeHead(200, {
      "Content-Type": MIME_TYPES[ext] || "application/octet-stream",
      "Content-Length": String(stat.size),
      "X-Content-Type-Options": "nosniff",
      "Cache-Control": ext === ".wasm" ? "public, max-age=300" : "no-cache"
    });
    if (req.method === "HEAD") return res.end();
    return fs.createReadStream(filePath).pipe(res);
  } catch (error) {
    if (error.code === "ENOENT") {
      return sendJson(res, 503, { ok: false, error: "Arquivos do motor ainda não foram compilados. Reinicie o servidor e confira o log do build." });
    }
    return sendJson(res, 500, { ok: false, error: "Falha ao abrir o arquivo solicitado." });
  }
}

const server = http.createServer((req, res) => {
  handle(req, res).catch(() => {
    if (!res.headersSent && !res.writableEnded) sendJson(res, 500, { ok: false, error: "Erro interno do servidor." });
    else if (!res.writableEnded) res.end();
  });
});

server.listen(PORT, "0.0.0.0", () => {
  console.log("[WispByte] Deobfuscator Lab disponível na porta " + PORT);
});
