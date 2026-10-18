# DirectML na NPU e `cargo xwin check` sem o script

tipo: armadilha
origem: task #819 · claude · 2026-10-18

- `ep::DirectML` com `DeviceFilter::Any` + `HighPerformance` faz o DXCore escolher a GPU antes da NPU: a NPU nunca roda. A NPU só é usada com `DeviceFilter::Npu`; a ordem NPU → GPU fica em `engine::DmlDevice::ORDER` e o `load_with_fallback` tenta cada dispositivo (encoder e decoder juntos) antes da CPU.
- O `ep::DirectML` e o `DeviceFilter` do `ort` compilam no Linux (o registro só falha em tempo de execução, `DirectML is not supported in this build`), então `onnx_whisper::directml` não precisa de `cfg(windows)`.
- `cargo xwin check -p pointspeak --target x86_64-pc-windows-msvc` direto falha no `whisper-rs-sys` com `STL1000: expected Clang 19.0.0 or newer`: o clang do PATH é antigo. Exporte o mesmo ambiente do `build-windows.sh` antes (`PATH=/usr/lib/llvm-19/bin:$PATH`, `GGML_NATIVE=OFF`, `GGML_OPENMP=OFF`, `GGML_AVX/AVX2/FMA/F16C=ON`, `CMAKE_{C,CXX}_FLAGS_RELEASE="/MT /O2 /Ob2 /DNDEBUG"`); com ele o check incremental leva ~30 s.
