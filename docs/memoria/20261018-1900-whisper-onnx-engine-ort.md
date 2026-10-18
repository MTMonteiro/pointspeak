# Engine Whisper ONNX no ort 2.0.0-rc.14: armadilhas

tipo: armadilha
origem: task #794 · claude · 2026-10-18

- O padrão de otimização do `ort` é `ORT_ENABLE_ALL`, e com ele o ONNX Runtime 1.24.4 (CPU EP) **não abre** o `encoder_model_fp16.onnx` do `onnx-community`: `SimplifiedLayerNormFusion` procura um `InsertedPrecisionFreeCast_…` que não existe. `GraphOptimizationLevel::Level3` (`ORT_ENABLE_LAYOUT`; o `All` do `ort` é outro nível) roda e foi o mais rápido. No Python 1.31 o mesmo modelo abria: conferir no runtime que o app carrega, não em outro.
- `with_execution_providers` registra em silêncio (EP que falha vira CPU sem erro). Para saber que a EP não iniciou, use `ExecutionProvider::register(&mut builder)`, que devolve o erro.
- `DirectML::default()` filtra só GPU (`DeviceFilter::Gpu`): NPU fica de fora. O app usa `DeviceFilter::Any` + `HighPerformance` (`onnx_whisper::directml()`), também na detecção.
- `SessionOutputs::remove(nome)` devolve um `DynValue` dono (sem lifetime): dá para guardar o `present.*` e passar como `past_key_values.*` no passo seguinte sem copiar. `DynValue` não é `Clone`.
- Tensor vazio para o 1º passo do decoder merged: `Tensor::from_array(([1, heads, 0, head_dim], Vec::<f32>::new()))` funciona. `heads`/`head_dim` vêm do `session.inputs()` (`ValueType::Tensor { shape }`, -1 nas dimensões simbólicas).
- Os métodos do `SessionBuilder` devolvem `Error<SessionBuilder>` (144 bytes): `.and_then(|b| b.with_…)` cai no `clippy::result_large_err`; encadeie com `?`/`map_err` direto.
- Baixar para validar: `./scripts/fetch-onnxruntime.sh linux` + os 4 arquivos do tiny do `ONNX_CATALOG` em `target/models/onnx-tiny/` + `jfk.wav` do whisper.cpp; `cargo run -p pointspeak --release --example transcribe -- --backend cpu target/models/onnx-tiny target/models/jfk.wav en`.
