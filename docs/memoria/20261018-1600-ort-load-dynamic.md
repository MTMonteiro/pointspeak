# ort 2.0.0-rc.14 com load-dynamic: armadilhas

tipo: armadilha
origem: task #791 · claude · 2026-10-18

- `default-features = false` é obrigatório: o padrão do `ort` liga `download-binaries` + `copy-dylibs` (baixa o ONNX Runtime no build) e `tls-native` (OpenSSL). Com `load-dynamic` o `ort-sys` fica com `disable-linking` e o build não toca a rede.
- A feature `api-N` fixa a versão **mínima** do runtime: `init_from` recusa biblioteca com minor < N (`BadVersion`), e biblioteca mais nova só gera um `info!`. O pacote NuGet `Microsoft.ML.OnnxRuntime.DirectML` parou na 1.24.4 (o ONNX Runtime seguiu até a 1.31): por isso `api-24`. Subir a `api-*` sem DirectML novo quebra o Windows.
- Sem `init_from` antes, qualquer API do `ort` chama o `setup_api`, que procura `ORT_DYLIB_PATH`/nome padrão e dá **panic** se não achar. O release usa `panic = "abort"`: nunca chame `ort::*` (nem `is_available`) sem um `init_from` que deu `Ok`.
- `Environment::devices()` (api-22+) só expõe o fabricante (`HardwareDevice::vendor`), não o nome da placa.
- O pacote `Microsoft.AI.DirectML` tem 200 MB (todas as arquiteturas + Xbox); o fetch guarda em `target/onnxruntime/downloads/` e só baixa de novo se o SHA-256 mudar.
- Overlay do Tauri para recursos do .exe: não use o nome `tauri.windows.conf.json` (o Tauri o mescla sozinho em todo build Windows e o `generate_context!` exigiria as DLLs). Use outro nome e `--config` só no `build-windows.sh`. Recursos em mapa `{"origem": "destino"}` com destino na raiz ficam ao lado do `pointspeak.exe`.
