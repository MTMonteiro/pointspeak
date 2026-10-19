# Fallback para a engine ONNX na CPU: como ler o erro "O modelo não carregou"

tipo: decisão
origem: task #829 · claude · 2026-10-19

- Ordem do `engine::load_with_fallback` quando a engine ONNX falha num backend acelerado: whisper.cpp na CPU (se o `.bin` está no disco, continua o preferido) → `load_onnx(Backend::Cpu, None)` (a própria variante ONNX na CPU EP) → erro. Variante ONNX ausente (`OnnxNotDownloaded`) não tenta a CPU EP: sem o `.bin` continua `ModelNotFound`.
- O erro final é sempre o motivo do backend acelerado (ex.: `DirectML reprovou na verificação: …`), nunca o da CPU EP. Então, depois desta task, ver `O modelo não carregou: DirectML …` no app significa que **a CPU EP também falhou**: procure no stderr a linha `[pointspeak] engine ONNX (CPU): …` com o erro real.
- No `src-tauri`, o mesmo closure ONNX serve à CPU: `sanity_checked(Cpu)` é false e o `open_session` só aplica `with_memory_pattern(false)`/`with_parallel_execution(false)` no DirectML, então a CPU abre como na referência da verificação.
- Testes do núcleo: o helper `Calls::onnx_seq` entra em pânico (`tentativa a mais`) se o carregador for chamado mais vezes que os resultados dados; ao mudar a ordem do fallback, os vetores dos testes precisam do resultado da tentativa na CPU (registrada como `("onnx", Backend::Cpu)`).
