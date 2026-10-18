# whisper-rs 0.16: build e armadilhas

tipo: armadilha
origem: task #765 · claude · 2026-10-18

- O `whisper-rs-sys` repassa toda variável de ambiente `GGML_*`, `WHISPER_*` e `CMAKE_*` como `-D` do cmake. O `build-windows.sh` usa isso para `GGML_NATIVE=OFF`, `GGML_OPENMP=OFF` e AVX2/FMA/F16C fixos.
- O build.rs do `whisper-rs-sys` testa `cfg!(target_os = "windows")` (o **host**), então no build cruzado não aplica `/utf-8` nem `advapi32`: se um dia der erro de link/encoding no .exe, é por aí.
- O cargo-xwin força o gerador **Ninja** no cmake: sem `ninja-build` o build Windows falha com "unable to find a build program corresponding to Ninja" (agora no `setup-windows-cross.sh`).
- `FullParams::set_detect_language(true)` faz o whisper.cpp **só detectar** o idioma e voltar sem texto; para auto use `set_language(Some("auto"))`.
- Sem `install_logging_hooks()` o whisper.cpp imprime no stderr/stdout a cada load; com a feature `log_backend` vai para o crate `log`.
- Em debug o whisper.cpp já compila otimizado (`RelWithDebInfo`); o `cargo test` do workspace do zero ficou em ~1m40 com 8 núcleos (whisper incluso).
- `EngineSlot::engine()`: `Option<&mut dyn Trait>` a partir de `Option<Box<dyn Trait>>` precisa de `.map(|e| e.as_mut() as &mut dyn Trait)`; `as_deref_mut()` dá erro de lifetime `'static`.
- Build Windows: a STL do xwin exige Clang ≥ 19 (STL1000); o `build-windows.sh` usa `/usr/lib/llvm-19/bin`. Sem `/MT` no `CMAKE_*_FLAGS_RELEASE` o .exe importa `MSVCP140.dll`; `+crt-static` quebra o link (conflita com o CRT híbrido do `static_vcruntime` do Tauri).
- Sem nenhuma chamada à engine no app, o LTO descarta o whisper.cpp do .exe: confira `strings pointspeak.exe | grep whisper_` e não só o tamanho.
