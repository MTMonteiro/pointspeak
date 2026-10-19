# Relatório da verificação de sanidade: onde mexer e como ler

tipo: armadilha
origem: task #831 · claude · 2026-10-19

- Correlação **0 exato** não é "modelo parecido": o `correlation` devolve 0 com tamanho diferente, NaN/±inf ou vetor constante. O `ProbeReport` (`whisper_onnx::sanity::probe_report`) guarda isso por passo; o log tem `passo 0 (encoder)` e `passo 1…3 (decoder, sem/com KV cache)`.
- A saída do encoder sai do `DecoderStep::encoder_output` (método com padrão `None`): fakes de teste continuam valendo sem implementar; na engine `ort` é o `hidden` do `OrtDecoderStep` copiado (`try_extract_tensor::<f32>`), 1500×384 no tiny.
- Referência no runner (CPU × CPU, tiny, `onnx_whisper_real`): passo 0 min −17,4 máx 17,0 média 0,03; passos do decoder min ≈ −16 máx 4–8 média −1 a −10. Valores do DirectML muito fora disso (ou `±inf`) apontam overflow do fp16.
- Leitura: passo 0 ruim = encoder; passo 0 ≈ 1 e passo 1 ruim = decoder sem cache (nó `If`/`past_key_values` vazios); só passos 2–3 = ramo `use_cache_branch`.
- Rodar o teste real leva ~1 min contando o download (77 MB do tiny + ORT): vale para conferir qualquer mudança no caminho `ort` da sanidade.
