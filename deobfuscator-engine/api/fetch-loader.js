const MAX_BYTES = 4 * 1024 * 1024;
const LOADER_URL = /^https:\/\/api\.jnkie\.com\/api\/v1\/luascripts\/public\/[a-f0-9]{64}\/download$/i;

module.exports = async function fetchLoader(req, res) {
  res.setHeader("Cache-Control", "no-store");
  res.setHeader("X-Content-Type-Options", "nosniff");

  if (req.method !== "POST") {
    res.setHeader("Allow", "POST");
    return res.status(405).json({ ok: false, error: "Use POST para solicitar o texto do loader." });
  }

  const rawUrl = typeof req.body?.url === "string" ? req.body.url : "";
  let target;
  try {
    target = new URL(rawUrl);
  } catch {
    return res.status(400).json({ ok: false, error: "URL inválida." });
  }

  // Strict allowlist: only public Jnkie script-download URLs, never arbitrary
  // hosts, ports, query strings, credentials, or redirect targets.
  if (
    target.protocol !== "https:" ||
    target.hostname !== "api.jnkie.com" ||
    target.port ||
    target.username ||
    target.password ||
    target.search ||
    target.hash ||
    !LOADER_URL.test(target.href)
  ) {
    return res.status(400).json({
      ok: false,
      error: "Só é permitido obter texto de URLs públicas Jnkie no formato compatível."
    });
  }

  try {
    const remote = await fetch(target.href, {
      method: "GET",
      redirect: "manual",
      headers: { Accept: "text/plain, application/octet-stream" },
      signal: AbortSignal.timeout(12000)
    });

    if (remote.status >= 300 && remote.status < 400) {
      return res.status(502).json({
        ok: false,
        error: "A fonte redirecionou para outro endereço; o motor bloqueou o redirecionamento por segurança."
      });
    }
    if (!remote.ok) {
      return res.status(502).json({
        ok: false,
        error: "A fonte remota respondeu HTTP " + remote.status + "."
      });
    }

    const declaredSize = Number(remote.headers.get("content-length") || 0);
    if (declaredSize > MAX_BYTES) {
      await remote.body?.cancel();
      return res.status(413).json({ ok: false, error: "O payload ultrapassa o limite de 4 MiB desta importação." });
    }

    const contentType = (remote.headers.get("content-type") || "").toLowerCase();
    if (contentType.includes("text/html")) {
      await remote.body?.cancel();
      return res.status(502).json({
        ok: false,
        error: "A fonte respondeu com uma página HTML, não com código-fonte. O conteúdo não foi enviado ao analisador."
      });
    }

    const bytes = new Uint8Array(await remote.arrayBuffer());
    if (bytes.byteLength === 0) {
      return res.status(502).json({ ok: false, error: "A fonte remota retornou um arquivo vazio." });
    }
    if (bytes.byteLength > MAX_BYTES) {
      return res.status(413).json({ ok: false, error: "O payload ultrapassa o limite de 4 MiB desta importação." });
    }

    let source;
    try {
      source = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      return res.status(415).json({ ok: false, error: "A resposta não é texto UTF-8 reconhecível." });
    }
    const prefix = source.trimStart().slice(0, 512).toLowerCase();
    if (prefix.startsWith("<!doctype html") || /<html[\s>]/i.test(prefix)) {
      return res.status(502).json({ ok: false, error: "A fonte devolveu uma página HTML/bloqueio em vez do payload Lua." });
    }
    if (!source.trim() || source.includes("\0")) {
      return res.status(415).json({ ok: false, error: "A resposta não parece ser código-fonte textual." });
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
        : "Não foi possível obter o texto da fonte remota."
    });
  }
};
