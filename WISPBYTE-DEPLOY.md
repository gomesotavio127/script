# Migrar o Deobfuscator Lab para a WispByte

Esta configuração é isolada na branch \`wispbyte-deployment\`; não substitui a publicação atual no Vercel.

## Criar o servidor

1. Abra https://wispbyte.com/client/create e crie um servidor chamado \`Deobfuscator Rust\`.
2. Selecione o plano gratuito, se estiver disponível na conta, e a imagem **Node.js 20 ou superior**. O Node serve o site e a API; o script de inicialização instala Rust e compila o WebAssembly.
3. Importe/clone o repositório \`gomesotavio127/script\` usando a branch \`wispbyte-deployment\` (ou envie o conteúdo dessa branch para **Files**). O arquivo \`package.json\` deve ficar na raiz do servidor.
4. Em **Startup**, use o comando \`npm start\`.
5. Em **Console**, inicie o servidor e acompanhe a primeira compilação. No primeiro boot, Rust e o target \`wasm32-unknown-unknown\` serão instalados; isso pode demorar e pode exceder a memória de um plano pequeno.
6. Quando iniciar, teste \`/healthz\` no endereço público do servidor. A resposta esperada é um JSON com \`"ok": true\`.

## Como funciona

- \`wispbyte-start.sh\` compila o motor Rust para WebAssembly se \`deobfuscator-engine/public/engine.wasm\` ainda não existir e reutiliza o arquivo nos reinícios seguintes.
- \`wispbyte-server.js\` serve os arquivos estáticos e implementa \`POST /api/fetch-loader\`, que a interface já utiliza.
- A API aceita URLs HTTP/HTTPS públicas de diferentes domínios que retornem texto, bloqueia IPs privados/reservados e redirecionamentos, fixa o IP validado para reduzir risco de DNS rebinding, limita a resposta a 4 MiB e não executa o código remoto.
- O servidor escuta em \`0.0.0.0\` e usa a porta indicada por \`PORT\`.

## Limitação conhecida

O primeiro build compila dependências Rust no servidor. A imagem Node.js precisa ter Bash e curl, acesso de rede para baixar o toolchain e memória suficiente para compilar. O script limita o paralelismo e desativa LTO para reduzir a memória; se o build terminar por falta de RAM, será necessário compilar o WASM fora do servidor e enviar o arquivo compilado para \`deobfuscator-engine/public/engine.wasm\`.
