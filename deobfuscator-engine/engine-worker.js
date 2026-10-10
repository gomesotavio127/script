let instancePromise;

async function getEngine() {
  if (!instancePromise) {
    instancePromise = (async () => {
      const response = await fetch(new URL("./engine.wasm?v=20261010-3", self.location.href));
      if (!response.ok) throw new Error("Não foi possível carregar o motor Rust.");
      const bytes = await response.arrayBuffer();
      const { instance } = await WebAssembly.instantiate(bytes, {});
      return instance;
    })();
  }
  return instancePromise;
}

self.onmessage = async (event) => {
  const { bytes, mode } = event.data || {};
  try {
    if (!(bytes instanceof ArrayBuffer) || bytes.byteLength === 0) {
      throw new Error("Entrada vazia.");
    }
    if (bytes.byteLength > 32 * 1024 * 1024) {
      throw new Error("O motor v1 aceita arquivos de até 32 MiB.");
    }

    const instance = await getEngine();
    const api = instance.exports;
    const memory = api.memory;
    if (!memory || typeof api.alloc !== "function" || typeof api.engine_run !== "function") {
      throw new Error("O WebAssembly publicado não possui a ABI esperada.");
    }

    const input = new Uint8Array(bytes);
    const inputPtr = api.alloc(input.byteLength);
    new Uint8Array(memory.buffer, inputPtr, input.byteLength).set(input);

    let outputPtr = 0;
    let outputLen = 0;
    try {
      outputPtr = api.engine_run(inputPtr, input.byteLength, mode === 1 ? 1 : 0);
      outputLen = api.result_len();
      const outputBytes = new Uint8Array(memory.buffer, outputPtr, outputLen).slice();
      const result = JSON.parse(new TextDecoder().decode(outputBytes));
      self.postMessage({ ok: true, result });
    } finally {
      if (outputPtr && outputLen && typeof api.free_result === "function") {
        api.free_result(outputPtr, outputLen);
      }
      if (typeof api.dealloc === "function") api.dealloc(inputPtr, input.byteLength);
    }
  } catch (error) {
    self.postMessage({ ok: false, error: error?.message || String(error) });
  }
};
