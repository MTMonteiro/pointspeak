# "Salada de tokens" no Whisper ONNX: como separar bug do pipeline de dispositivo errado

tipo: armadilha
origem: task #827 · claude · 2026-10-19

- Sintoma: texto com tokens de várias línguas, palavras repetidas e `�`. É o decoder guloso recebendo logits sem relação com o áudio, não áudio ruim.
- Antes de mexer no pipeline, reproduza na CPU: `./scripts/fetch-onnxruntime.sh linux`, baixe `onnx-tiny`/`onnx-base` pelas URLs do `ONNX_CATALOG` em `target/models/`, `jfk.wav` do whisper.cpp e rode o exemplo `transcribe --backend cpu` (en/auto/pt). Sem `ffmpeg` no runner: um WAV 48 kHz estéreo sai com `python3 -I` + `wave` (o exemplo passa pelo mesmo downmix/resample da captura). Na task #827 tudo saiu certo na CPU: mel, prompt, KV cache, logits da última posição, resample e BPE estavam bons, e a causa ficou no DirectML (NPU/GPU), que só o Windows reproduz.
- Mitigação: `whisper_onnx::sanity` compara os logits do dispositivo com os da CPU numa entrada fixa (Pearson ≥ 0,95). Calibração no tiny real: fp32 × fp16 = 0,999999; outro modelo (base no lugar do tiny) = 0,21. Cosseno sem centralizar engana (os logits têm deslocamento comum); use correlação.
- Para simular um "dispositivo errado" no Linux, o teste real ignorado aceita `POINTSPEAK_TEST_ONNX_OTHER=<pasta onnx-base>`; para simular arredondamento, a pasta com o fp32 do tiny (`onnx/encoder_model.onnx` e `onnx/decoder_model_merged.onnx` renomeados para os nomes fp16).
- `docs/decisoes/` não existe neste repositório: decisões de task ficam no HANDOFF/ficha e no `docs/tecnologia.md`.
