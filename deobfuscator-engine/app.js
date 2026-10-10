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

let fileData = null;
let fileName = "";
let worker = null;
let initialized = false;
let lastResult = null;

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

function renderResult(result) {
  lastResult = result;
  results.hidden = false;
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
    ? "Foi detectado apenas um loader (carregador). O arquivo não contém o payload protegido que ele tenta obter. O motor não baixa nem executa conteúdo remoto; selecione um arquivo local que contenha o código protegido."
    : "Este arquivo não foi descompilado. Confira o diagnóstico e a versão de bytecode suportada; formatos desconhecidos não produzem código fictício.";
  $("noSourceOutput").hidden = hasOutput;
  const statusMessage = isLoader
    ? `Entrada identificada como loader de ${Number(result.metrics?.bytes || 0).toLocaleString("pt-BR")} bytes; nenhum payload foi analisado. Selecione o arquivo local com o código protegido.`
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

  if (bytes.byteLength > 32 * 1024 * 1024) {
    setStatus("O limite da versão 1 é 32 MiB.", "error");
    return;
  }

  runButton.disabled = true;
  runButton.textContent = "ANALISANDO…";
  setStatus("Analisando " + prettyBytes(bytes.byteLength) + " com o motor Rust…");
  results.hidden = true;
  getWorker().postMessage({ bytes: bytes.buffer, mode: selectedMode }, [bytes.buffer]);
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
