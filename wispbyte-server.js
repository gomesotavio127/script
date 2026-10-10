"use strict";

const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const dns = require("node:dns").promises;
const net = require("node:net");
const http = require("node:http");
const https = require("node:https");

const ROOT = __dirname;
const PUBLIC_DIR = path.join(ROOT, "deobfuscator-engine", "public");
const PORT = Number(process.env.SERVER_PORT || process.env.PORT || 11675);
const MAX_LOADER_BYTES = 4 * 1024 * 1024;
const MAX_JSON_BYTES = 16 * 1024;

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

function isPublicIPv4(address) {
  if (net.isIP(address) !== 4) return false;
  const [a, b, c, d] = address.split(".").map(Number);
  if (![a, b, c, d].every((part) => Number.isInteger(part) && part >= 0 && part <= 255)) return false;
  if (a === 0 || a === 10 || a === 127 || a >= 224) return false;
  if (a === 100 && b >= 64 && b <= 127) return false;
  if (a === 169 && b === 254) return false;
  if (a === 172 && b >= 16 && b <= 31) return false;
  if (a === 192 && b === 168) return false;
  if (a === 192 && b === 0 && (c === 0 || c === 2)) return false;
  if (a === 192 && b === 88 && c === 99) return false;
  if (a === 198 && (b === 18 || b === 19 || (b === 51 && c === 100))) return false;
  if (a === 203 && b === 0 && c === 113) return false;
  if (a === 255 && b === 255 && c === 255 && d === 255) return false;
  return true;
}

async function resolvePublicIPv4(hostname) {
  const normalized = hostname.replace(/^\[|\]$/g, "");
  const ipType = net.isIP(normalized);
  if (ipType === 4) {
    if (!isPublicIPv4(normalized)) throw Object.assign(new Error("Endereços IP privados ou reservados não podem ser importados."), { statusCode: 403 });
    return normalized;
  }
  if (ipType === 6) throw Object.assign(new Error("URLs com IPv6 literal não são aceitas; use o domínio público correspondente."), { statusCode: 400 });
  let records;
  try { records = await dns.lookup(normalized, { all: true, verbatim: true, family: 4 }); }
  catch { throw Object.assign(new Error("Não foi possível resolver o domínio para um endereço IPv4 público."), { statusCode: 400 }); }
  if (!records.length || records.some((record) => !isPublicIPv4(record.address))) {
    throw Object.assign(new Error("O domínio não resolve exclusivamente para endereços IPv4 públicos."), { statusCode: 403 });
  }
  return records[0].address;
}

function requestPublicText(target, pinnedAddress) {
  return new Promise((resolve, reject) => {
    const transport = target.protocol === "https:" ? https : http;
    const hostname = target.hostname.replace(/^\[|\]$/g, "");
    const options = {
      protocol: target.protocol, hostname,
      port: target.port ? Number(target.port) : undefined,
      path: target.pathname + target.search, method: "GET",
      headers: { Accept: "text/plain, application/javascript, application/x-lua, application/octet-stream;q=0.9, */*;q=0.1", "Accept-Encoding": "identity", "User-Agent": "DeobfuscatorLab/1.0" },
      lookup: (_name, lookupOptions, callback) => {
        if (lookupOptions && lookupOptions.all) callback(null, [{ address: pinnedAddress, family: 4 }]);
        else callback(null, pinnedAddress, 4);
      }
    };
    if (target.protocol === "https:" && net.isIP(hostname) === 0) options.servername = hostname;
    const request = transport.request(options, (response) => {
      const status = response.statusCode || 0;
      if (status >= 300 && status < 400) {
        response.resume();
        reject(Object.assign(new Error("Redirecionamentos foram bloqueados por segurança. Use a URL final do arquivo."), { statusCode: 502 }));
        return;
      }
      if (status < 200 || status >= 300) {
        response.resume();
        reject(Object.assign(new Error("A fonte remota respondeu HTTP " + status + "."), { statusCode: 502 }));
        return;
      }
      const contentType = String(response.headers["content-type"] || "").toLowerCase();
      if (contentType.includes("text/html") || contentType.includes("application/xhtml+xml")) {
        response.resume();
        reject(Object.assign(new Error("A URL retornou uma página HTML, não o conteúdo textual do script. Use o link direto/raw do arquivo."), { statusCode: 415 }));
        return;
      }
      const declaredSize = Number(response.headers["content-length"] || 0);
      if (declaredSize > MAX_LOADER_BYTES) {
        response.resume();
        reject(Object.assign(new Error("O arquivo ultrapassa o limite de 4 MiB da importação por URL."), { statusCode: 413 }));
        return;
      }
      const chunks = []; let total = 0;
      response.on("data", (chunk) => {
        total += chunk.length;
        if (total > MAX_LOADER_BYTES) {
          response.destroy(); request.destroy();
          reject(Object.assign(new Error("O arquivo ultrapassa o limite de 4 MiB da importação por URL."), { statusCode: 413 }));
          return;
        }
        chunks.push(Buffer.from(chunk));
      });
      response.on("error", (error) => reject(error));
      response.on("end", () => {
        if (total === 0) return reject(Object.assign(new Error("A URL retornou um arquivo vazio."), { statusCode: 502 }));
        let source;
        try { source = new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks, total)); }
        catch { return reject(Object.assign(new Error("O arquivo não contém texto UTF-8 reconhecível."), { statusCode: 415 })); }
        const prefix = source.trimStart().slice(0, 512).toLowerCase();
        if (!source.trim() || source.includes("\0") || prefix.startsWith("<!doctype html") || /<html[\s>]/i.test(prefix)) {
          return reject(Object.assign(new Error("A URL retornou HTML, conteúdo vazio ou texto inválido em vez do código-fonte."), { statusCode: 415 }));
        }
        resolve(Buffer.from(source, "utf8"));
      });
    });
    request.setTimeout(12000, () => request.destroy(Object.assign(new Error("A fonte remota demorou demais para responder."), { statusCode: 504 })));
    request.on("error", (error) => reject(error.statusCode ? error : Object.assign(new Error("Não foi possível obter o texto da URL pública."), { statusCode: 502 })));
    request.end();
  });
}

async function getRemoteSourceText(rawUrl) {
  if (typeof rawUrl !== "string" || rawUrl.length > 4096) throw Object.assign(new Error("Informe uma URL HTTP ou HTTPS válida."), { statusCode: 400 });
  let target;
  try { target = new URL(rawUrl); } catch { throw Object.assign(new Error("URL inválida."), { statusCode: 400 }); }
  if ((target.protocol !== "https:" && target.protocol !== "http:") || target.username || target.password || target.hash) {
    throw Object.assign(new Error("Use uma URL HTTP ou HTTPS pública, sem credenciais nem fragmento."), { statusCode: 400 });
  }
  const pinnedAddress = await resolvePublicIPv4(target.hostname);
  return requestPublicText(target, pinnedAddress);
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

  if (pathname === "/api/fetch-source" || pathname === "/api/fetch-loader") {
    res.setHeader("Cache-Control", "no-store");
    if (req.method !== "POST") {
      res.setHeader("Allow", "POST");
      return sendJson(res, 405, { ok: false, error: "Use POST para importar o texto de uma URL pública." });
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
      const content = await getRemoteSourceText(body.url);
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
