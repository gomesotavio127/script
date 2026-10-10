const dns = require("node:dns").promises;
const net = require("node:net");

const MAX_BYTES = 4 * 1024 * 1024;
const TIMEOUT_MS = 12000;

function isPublicIPv4(address) {
  if (net.isIP(address) !== 4) return false;
  const [a, b, c, d] = address.split(".").map(Number);
  if (![a, b, c, d].every((x) => Number.isInteger(x) && x >= 0 && x <= 255)) return false;
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

async function requirePublicHost(hostname) {
  const ipType = net.isIP(hostname);
  if (ipType === 4) {
    if (!isPublicIPv4(hostname)) throw new Error("Endereços IP privados ou reservados não são permitidos.");
    return;
  }
  if (ipType === 6) {
    throw new Error("Informe o domínio público em vez de um endereço IPv6 literal.");
  }
  if (!hostname || hostname === "localhost" || hostname.endsWith(".localhost") ||
      hostname.endsWith(".local") || hostname.endsWith(".internal")) {
    throw new Error("Somente domínios públicos podem ser importados.");
  }
  let records;
  try {
    records = await dns.lookup(hostname, { all: true, verbatim: true, family: 4 });
  } catch {
    throw new Error("Não foi possível resolver o domínio público.");
  }
  if (!records.length || records.some((record) => !isPublicIPv4(record.address))) {
    throw new Error("O domínio não resolve exclusivamente para endereços IPv4 públicos.");
  }
}

module.exports = async function fetchLoader(req, res) {
  res.setHeader("Cache-Control", "no-store");
  res.setHeader("X-Content-Type-Options", "nosniff");

  if (req.method !== "POST") {
    res.setHeader("Allow", "POST");
    return res.status(405).json({ ok: false, error: "Use POST para importar o texto de uma URL pública." });
  }

  const rawUrl = typeof req.body?.url === "string" ? req.body.url.trim() : "";
  if (!rawUrl || rawUrl.length > 4096) {
    return res.status(400).json({ ok: false, error: "Informe uma URL HTTP/HTTPS válida." });
  }

  let target;
  try {
    target = new URL(rawUrl);
  } catch {
    return res.status(400).json({ ok: false, error: "URL inválida." });
  }

  if ((target.protocol !== "https:" && target.protocol !== "http:") ||
      target.username || target.password || target.hash ||
      (target.port && !["80", "443"].includes(target.port))) {
    return res.status(400).json({
      ok: false,
      error: "Use uma URL HTTP/HTTPS pública, sem credenciais, fragmento ou porta não padrão."
    });
  }

  try {
    let current = target;
    let remote;
    const visited = new Set();

    for (let hop = 0; hop <= 5; hop++) {
      if ((current.protocol !== "https:" && current.protocol !== "http:") ||
          current.username || current.password || current.hash ||
          (current.port && !["80", "443"].includes(current.port))) {
        throw new Error("O redirecionamento levou a uma URL não permitida.");
      }
      if (current.protocol === "http:" && target.protocol === "https:") {
        throw new Error("O redirecionamento tentou sair de HTTPS para HTTP.");
      }
      if (visited.has(current.href)) {
        throw new Error("A URL entrou em um ciclo de redirecionamento.");
      }
      visited.add(current.href);
      await requirePublicHost(current.hostname);

      remote = await fetch(current.href, {
        method: "GET",
        redirect: "manual",
        headers: {
          "User-Agent": "DeobfuscatorLab/1.0 (+https://github.com/gomesotavio127/script)",
          "Accept": "text/plain, application/javascript, application/x-lua, application/octet-stream;q=0.9, */*;q=0.1",
          "Accept-Language": "pt-BR,pt;q=0.9,en;q=0.8"
        },
        signal: AbortSignal.timeout(TIMEOUT_MS)
      });

      if (![301, 302, 303, 307, 308].includes(remote.status)) break;

      const location = remote.headers.get("location");
      await remote.body?.cancel();
      if (!location) {
        return res.status(502).json({ ok: false, error: "A fonte respondeu com redirecionamento sem destino." });
      }
      if (hop === 5) {
        return res.status(502).json({ ok: false, error: "A URL excedeu o limite de 5 redirecionamentos." });
      }

      try {
        current = new URL(location, current);
      } catch {
        return res.status(502).json({ ok: false, error: "A fonte respondeu com um redirecionamento inválido." });
      }
    }

    if (!remote) throw new Error("A fonte remota não retornou resposta.");
    if (remote.status === 403) {
      return res.status(502).json({
        ok: false,
        error: "O servidor da URL recusou o acesso (HTTP 403). O importador já enviou uma requisição identificada e de texto; essa fonte pode exigir login, token, cookies ou permitir acesso apenas pelo navegador. Use o link público direto/raw ou baixe o arquivo e selecione-o manualmente."
      });
    }
    if (!remote.ok) {
      return res.status(502).json({
        ok: false,
        error: "A fonte remota respondeu HTTP " + remote.status + ". Verifique se a URL aponta diretamente para o arquivo público."
      });
    }

    const contentType = (remote.headers.get("content-type") || "").toLowerCase();
    if (contentType.includes("text/html") || contentType.includes("application/xhtml+xml")) {
      await remote.body?.cancel();
      return res.status(415).json({
        ok: false,
        error: "A URL retornou HTML, não o código-fonte. Use o link direto/raw do arquivo."
      });
    }

    const declaredSize = Number(remote.headers.get("content-length") || 0);
    if (declaredSize > MAX_BYTES) {
      await remote.body?.cancel();
      return res.status(413).json({ ok: false, error: "O arquivo ultrapassa o limite de 4 MiB." });
    }

    if (!remote.body) {
      return res.status(502).json({ ok: false, error: "A fonte remota não retornou conteúdo." });
    }

    const reader = remote.body.getReader();
    const chunks = [];
    let total = 0;

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > MAX_BYTES) {
        await reader.cancel();
        return res.status(413).json({ ok: false, error: "O arquivo ultrapassa o limite de 4 MiB." });
      }
      chunks.push(value);
    }

    if (!total) {
      return res.status(502).json({ ok: false, error: "A fonte remota retornou um arquivo vazio." });
    }

    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.byteLength;
    }

    let source;
    try {
      source = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      return res.status(415).json({ ok: false, error: "A resposta não é texto UTF-8 reconhecível." });
    }

    const prefix = source.trimStart().slice(0, 512).toLowerCase();
    if (!source.trim() || source.includes("\0") ||
        prefix.startsWith("<!doctype html") || /<html[\s>]/i.test(prefix)) {
      return res.status(415).json({ ok: false, error: "A URL não retornou texto de código-fonte reconhecível." });
    }

    res.setHeader("Content-Type", "text/plain; charset=utf-8");
    res.setHeader("Content-Length", String(bytes.byteLength));
    res.setHeader("X-Payload-Bytes", String(bytes.byteLength));
    return res.status(200).send(source);
  } catch (error) {
    const isTimeout = error?.name === "TimeoutError" || error?.name === "AbortError";
    return res.status(502).json({
      ok: false,
      error: isTimeout
        ? "A fonte remota demorou demais para responder."
        : (error?.message || "Não foi possível obter o texto da fonte remota.")
    });
  }
};
