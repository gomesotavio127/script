const $ = (id) => document.getElementById(id);
const input = $("source");
const mode = $("mode");
const fileInput = $("file");
const runButton = $("run");
const statusEl = $("status");
const results = $("results");
const output = $("output");
const copyButton = $("copy");
const downloadButton = $("download");
const loaderActions = $("loaderActions");
const loaderUrlInput = $("loaderUrl");
const fetchPayloadButton = $("fetchPayload");
const loaderHelp = $("loaderHelp");
const compatPanel = $("compatPanel");
const compatSummary = $("compatSummary");
const compatReport = $("compatReport");
const downloadCompatButton = $("downloadCompat");

let fileData = null;
let fileName = "";
let worker = null;
let initialized = false;
let lastResult = null;
let currentLoaderUrl = "";

function extractCompatibleLoaderUrl(source) {
  const normalized = String(source || "")
    .replaceAll("\\/", "/")
    .replaceAll("\\u002F", "/")
    .replaceAll("\\x2f", "/")
    .replaceAll("\\x3a", ":");
  const candidates = [];
  const trimmed = normalized.trim();
  if (/^https?:\/\//i.test(trimmed)) candidates.push(trimmed);
  candidates.push(...(normalized.match(/https?:\/\/[^\s"'<>]+/gi) || []));
  for (const candidate of candidates) {
    const cleaned = candidate.replace(/[)\]},;]+$/g, "");
    try {
      const parsed = new URL(cleaned);
      if (
        (parsed.protocol === "https:" || parsed.protocol === "http:") &&
        parsed.hostname &&
        !parsed.username &&
        !parsed.password &&
        !parsed.hash
      ) return parsed.href;
    } catch {
      // Keep looking for another public URL in the loader text.
    }
  }
  return "";
}

function setStatus(message, kind = "") {
  statusEl.textContent = message;
  statusEl.dataset.kind = kind;
}

function prettyBytes(n) {
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KiB";
  return (n / 1048576).toFixed(2) + " MiB";
}

function getWorker() {
  if (!worker) {
    worker = new Worker("./engine-worker.js", { type: "module" });
    worker.onmessage = ({ data }) => {
      if (!initialized) {
        initialized = true;
        runButton.disabled = false;
        runButton.textContent = "ANALISAR COM MOTOR RUST";
        if (data?.ok) setStatus("Motor Rust carregado e pronto.", "success");
        else setStatus(data?.error || "Motor inicializado com aviso.", "warning");
        return;
      }
      runButton.disabled = false;
      runButton.textContent = "ANALISAR COM MOTOR RUST";
      if (!data?.ok) {
        setStatus(data?.error || "Falha ao executar o motor.", "error");
        return;
      }
      renderResult(data.result);
    };
    worker.onerror = (event) => {
      initialized = true;
      runButton.disabled = false;
      runButton.textContent = "ANALISAR COM MOTOR RUST";
      setStatus(event.message || "Falha no worker WebAssembly.", "error");
    };
  }
  return worker;
}

const COMPAT_API_GROUPS = [
  {
    title: "Loader e rede",
    names: ["loadstring", "game:HttpGet", "HttpGet", "request", "http_request", "syn.request", "http.request"],
    guidance: "Registrar a dependência. Para análise offline, usar uma resposta gravada e fictícia; não fazer chamadas de rede automaticamente."
  },
  {
    title: "Identidade do executor",
    names: ["identifyexecutor", "getexecutorname", "getexecutorversion", "isexecutorclosure"],
    guidance: "Não falsificar a identidade de um executor real. Em código próprio, substituir a checagem por um perfil MOCK declarado."
  },
  {
    title: "Ambiente e registro",
    names: ["getgenv", "getrenv", "getreg", "getgc"],
    guidance: "Mapear o escopo e as dependências necessárias. Não expor o ambiente real do Roblox em testes locais."
  },
  {
    title: "Hooks e introspecção",
    names: ["hookfunction", "hookmetamethod", "newcclosure", "checkcaller", "getnamecallmethod", "getupvalue", "setupvalue", "getconstants", "getprotos", "getstack", "getconnections", "firesignal", "cloneref", "compareinstances"],
    guidance: "Não há equivalente fiel fora do runtime. Marcar como não suportado em vez de instalar hooks ou alterar o comportamento do jogo."
  },
  {
    title: "Arquivos locais",
    names: ["readfile", "writefile", "appendfile", "isfile", "isfolder", "makefolder", "listfiles", "delfile", "delfolder", "loadfile", "dofile"],
    guidance: "Usar fixtures locais controladas e leitura limitada; não permitir gravação ou exclusão arbitrária."
  },
  {
    title: "Criptografia e codificação",
    names: ["crypt.", "syn.crypt", "base64", "lz4", "zlib", "bit32."],
    guidance: "Identificar o algoritmo e os dados de entrada. Implementar apenas transformação pura conhecida e validar com fixtures."
  },
  {
    title: "Interface, desenho e conexão",
    names: ["gethui", "get_hidden_gui", "Drawing.new", "WebSocket.connect", "queue_on_teleport", "setclipboard", "getclipboard"],
    guidance: "Registrar como dependência de plataforma. Em testes, usar um adaptador transparente sem interface real, sockets ou efeitos no jogo."
  }
];

function buildCompatibilityReport(source) {
  const text = String(source || "");
  const lines = [
    "FUSCATOR LAB — MAPA ESTÁTICO DE COMPATIBILIDADE",
    "Perfil: MOCK TRANSPARENTE / análise sem execução",
    "Este relatório é heurístico. Nenhuma API foi chamada e nenhum script foi executado.",
    ""
  ];
  let total = 0;

  for (const group of COMPAT_API_GROUPS) {
    const names = group.names.filter((name) => {
      const escaped = name.replaceAll(".", "\\.");
      return new RegExp(escaped, "i").test(text);
    });
    if (!names.length) continue;
    total += names.length;
    lines.push("[" + group.title + "]");
    for (const name of names) lines.push("  - " + name);
    lines.push("  Tratamento sugerido: " + group.guidance, "");
  }

  lines.push("PRÓXIMAS ETAPAS");
  if (total === 0) {
    lines.push("- Nenhuma referência reconhecida foi encontrada no texto analisado.");
    lines.push("- Código compactado, strings codificadas ou VM própria podem esconder as referências.");
  } else {
    lines.push("1. Separar cada API detectada das regras centrais do script.");
    lines.push("2. Definir mocks explícitos apenas para dependências que você controla.");
    lines.push("3. Deixar identidade real, hooks, rede e operações destrutivas indisponíveis por padrão.");
    lines.push("4. Comparar cada saída com fixtures conhecidas e registrar chamadas não suportadas.");
    lines.push("5. Se as APIs aparecerem somente depois de uma etapa de decodificação, analisar essa saída separadamente.");
  }
  lines.push("", "Total de nomes de API reconhecidos: " + total);
  return { text: lines.join("\n"), total };
}

function renderCompatibilityReport(source) {
  const report = buildCompatibilityReport(source);
  compatPanel.hidden = false;
  compatReport.value = report.text;
  compatSummary.textContent = report.total
    ? report.total + " referência(s) de API identificada(s). O mapa orienta mocks explícitos; não finge ser um executor real."
    : "Nenhuma referência conhecida detectada. A análise é estática e pode não enxergar nomes escondidos em strings codificadas ou VM.";
}

function renderResult(result) {
  lastResult = result;
  results.hidden = false;
  renderCompatibilityReport(input.value);
  $("summary").textContent = result.summary || result.error || "Análise concluída.";
  $("engineTag").textContent = (result.engine || "motor") + " v" + (result.version || "?") + " · " + (result.mode || "");
  $("format").textContent = result.format || (result.mode === "source" ? "Lua/Luau — análise estática" : "Formato não identificado");
  $("metrics").replaceChildren();

  for (const [key, value] of Object.entries(result.metrics || {})) {
    const item = document.createElement("div");
    item.className = "metric";
    const label = document.createElement("span");
    label.textContent = key.replaceAll("_", " ");
    const val = document.createElement("strong");
    val.textContent = typeof value === "number" && ["bytes", "characters"].includes(key)
      ? value.toLocaleString("pt-BR")
      : String(value);
    item.append(label, val);
    $("metrics").append(item);
  }

  const findings = [...(result.findings || []), ...(result.warnings || [])];
  $("findings").replaceChildren();
  for (const finding of findings) {
    const li = document.createElement("li");
    li.textContent = finding;
    $("findings").append(li);
  }

  const isLoader = result.format === "Lua/Luau dynamic loader";
  currentLoaderUrl = isLoader ? extractCompatibleLoaderUrl(input.value) : "";
  loaderUrlInput.value = currentLoaderUrl;
  loaderActions.hidden = false;
  fetchPayloadButton.disabled = !currentLoaderUrl;
  loaderHelp.textContent = currentLoaderUrl
    ? "URL compatível detectada. A ferramenta obterá somente o texto, sem executar o conteúdo."
    : "URL não detectada automaticamente. Você pode colar a URL pública Jnkie compatível neste campo.";
  const hasOutput = typeof result.output === "string" && result.output.length > 0;
  const transformations = Number(result.metrics?.transformations_total || 0);
  output.value = hasOutput ? result.output : "";
  $("outputLabel").textContent = isLoader
    ? "Nenhum código protegido recuperado"
    : result.mode === "bytecode"
      ? "Código Luau recuperado por descompilação"
      : transformations > 0
        ? "Código recuperado (transformações estáticas; pode ser parcial)"
        : "Código de entrada normalizado (nenhuma desofuscação confirmada)";
  output.hidden = !hasOutput;
  copyButton.disabled = !hasOutput;
  downloadButton.disabled = !hasOutput;
  $("noSourceOutput").textContent = isLoader
    ? "Foi detectado apenas um loader (carregador). Use “Obter texto do loader” para tentar importar a fonte compatível; o texto recebido será analisado sem executar o script."
    : "Este arquivo não foi descompilado. Confira o diagnóstico e a versão de bytecode suportada; formatos desconhecidos não produzem código fictício.";
  $("noSourceOutput").hidden = hasOutput;
  const statusMessage = isLoader
    ? `Entrada identificada como loader de ${Number(result.metrics?.bytes || 0).toLocaleString("pt-BR")} bytes; nenhum payload foi analisado ainda. Use “Obter texto do loader” ou selecione a captura local.`
    : result.mode === "bytecode"
      ? (hasOutput
        ? "Bytecode descompilado pelo motor Rust. Revise os avisos; comentários e nomes originais podem não existir mais."
        : "O decodificador não recuperou código deste arquivo. Nenhum resultado foi inventado.")
      : transformations > 0
        ? `${transformations.toLocaleString("pt-BR")} transformação(ões) estática(s) aplicada(s). Trechos não compreendidos foram preservados; confira os avisos.`
        : "Nenhuma transformação estática compatível foi aplicada. Isto não significa que o código esteja desofuscado.";
  setStatus(
    statusMessage,
    isLoader || result.warnings?.length || (result.mode === "source" && transformations === 0) ? "warning" : "success"
  );
}

function runAnalysis(bytes, selectedMode) {
  if (!bytes?.byteLength) {
    setStatus("Entrada vazia.", "error");
    return;
  }
  if (bytes.byteLength > 32 * 1024 * 1024) {
    setStatus("O limite da versão 1 é 32 MiB.", "error");
    return;
  }

  runButton.disabled = true;
  runButton.textContent = "ANALISANDO…";
  setStatus("Analisando " + prettyBytes(bytes.byteLength) + " com o motor Rust…");
  results.hidden = true;
  const transfer = bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength
    ? bytes.buffer
    : bytes.slice().buffer;
  getWorker().postMessage({ bytes: transfer, mode: selectedMode }, [transfer]);
}

$("form").addEventListener("submit", (event) => {
  event.preventDefault();
  const selectedMode = mode.value === "bytecode" ? 1 : 0;
  let bytes;
  if (selectedMode === 1 && fileData) {
    bytes = new Uint8Array(fileData).slice();
  } else {
    const value = input.value;
    if (!value.trim()) {
      setStatus("Cole o código ou selecione um arquivo primeiro.", "error");
      return;
    }
    bytes = new TextEncoder().encode(value);
  }
  runAnalysis(bytes, selectedMode);
});

loaderUrlInput.addEventListener("input", () => {
  currentLoaderUrl = extractCompatibleLoaderUrl(loaderUrlInput.value);
  fetchPayloadButton.disabled = !currentLoaderUrl;
  loaderHelp.textContent = currentLoaderUrl
    ? "URL compatível validada. A ferramenta obterá somente o texto, sem executar o conteúdo."
    : "Cole uma URL pública Jnkie compatível com o formato aceito.";
});

fetchPayloadButton.addEventListener("click", async () => {
  const url = extractCompatibleLoaderUrl(loaderUrlInput.value) || currentLoaderUrl;
  if (!url) {
    setStatus("Cole uma URL pública Jnkie compatível no campo.", "error");
    return;
  }

  fetchPayloadButton.disabled = true;
  setStatus("Obtendo somente o texto do payload remoto — o script não será executado…", "warning");
  try {
    const response = await fetch("./api/fetch-loader", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Accept": "text/plain, application/json" },
      body: JSON.stringify({ url })
    });

    if (!response.ok) {
      let message = "Falha ao obter o payload (HTTP " + response.status + ").";
      try {
        const details = await response.json();
        if (details?.error) message = details.error;
      } catch {}
      throw new Error(message);
    }

    const text = await response.text();
    const bytes = new TextEncoder().encode(text);
    if (!text.trim() || bytes.byteLength === 0) {
      throw new Error("A fonte remota não retornou código-fonte textual.");
    }
    if (bytes.byteLength > 32 * 1024 * 1024) {
      throw new Error("O payload ultrapassa o limite de 32 MiB do motor.");
    }

    input.value = text;
    mode.value = "source";
    fileData = null;
    fileName = "loader-payload.lua";
    currentLoaderUrl = "";
    runAnalysis(bytes, 0);
  } catch (error) {
    fetchPayloadButton.disabled = false;
    setStatus((error?.message || "Falha ao obter o payload.") + " Você também pode baixar o arquivo manualmente e selecioná-lo acima.", "error");
  }
});

fileInput.addEventListener("change", async () => {
  const file = fileInput.files?.[0];
  if (!file) return;
  fileName = file.name;
  if (file.size > 32 * 1024 * 1024) {
    fileInput.value = "";
    fileData = null;
    setStatus("Arquivo maior que 32 MiB; escolha um arquivo menor.", "error");
    return;
  }
  fileData = await file.arrayBuffer();
  const looksBinary = /\.(luac|bytecode|bc)$/i.test(file.name);
  if (looksBinary) {
    mode.value = "bytecode";
    input.value = "";
  } else {
    mode.value = "source";
    input.value = new TextDecoder("utf-8", { fatal: false }).decode(fileData);
  }
  setStatus("Arquivo carregado: " + fileName + " (" + prettyBytes(file.size) + ").");
});

mode.addEventListener("change", () => {
  if (mode.value === "bytecode" && !fileData) {
    setStatus("Para bytecode binário, selecione um arquivo .luac ou arquivo binário.");
  }
});

copyButton.addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(output.value);
    setStatus("Código normalizado copiado.", "success");
  } catch {
    output.focus();
    output.select();
    setStatus("Selecione o texto e copie manualmente.", "warning");
  }
});

downloadButton.addEventListener("click", () => {
  const blob = new Blob([output.value], { type: "text/plain;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  const baseName = fileName.replace(/\.[^.]+$/, "") || "source";
  const recovered = lastResult?.mode === "bytecode"
    || Number(lastResult?.metrics?.transformations_total || 0) > 0;
  anchor.download = baseName + (recovered ? ".recovered.lua" : ".analysis.lua");
  anchor.click();
  URL.revokeObjectURL(url);
});

downloadCompatButton.addEventListener("click", () => {
  const blob = new Blob([compatReport.value], { type: "text/plain;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = "fuscator-compatibility-report.txt";
  anchor.click();
  URL.revokeObjectURL(url);
});

$("clear").addEventListener("click", () => {
  input.value = "";
  fileInput.value = "";
  fileData = null;
  fileName = "";
  lastResult = null;
  results.hidden = true;
  output.value = "";
  setStatus("Pronto para analisar.");
});

setStatus("Carregando motor Rust…");
const warmup = new TextEncoder().encode("return 1");
getWorker().postMessage({ bytes: warmup.buffer, mode: 0 }, [warmup.buffer]);
