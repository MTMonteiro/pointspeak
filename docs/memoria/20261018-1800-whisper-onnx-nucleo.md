# Whisper ONNX (onnx-community): o que conferir antes de mexer

tipo: armadilha
origem: task #793 · claude · 2026-10-18

- Os exports `onnx-community/whisper-*` em fp16 (`encoder_model_fp16.onnx`, `decoder_model_merged_fp16.onnx`) têm entradas e saídas **f32** (pesos fp16 por dentro): a engine não precisa converter para `f16`. O decoder merged pede `use_cache_branch` (bool) e os `past_key_values.*` vazios (dimensão 0) no primeiro passo.
- `vocab.json`/`added_tokens.json` não ficam no LFS: o tree da API do Hugging Face não traz `sha256` deles, só o `oid` git (SHA-1). Baixe e rode `sha256sum`. Use sempre o commit (`/resolve/<sha>/`) na URL, nunca `main`, senão o hash quebra quando o repositório muda.
- IDs especiais mudam entre o multilíngue (99 idiomas) e o large-v3 (100, com `<|yue|>`): `<|transcribe|>` 50359 vs 50360, `<|notimestamps|>` 50363 vs 50364. No tiny o `<|endoftext|>` está no `vocab.json`; no large-v3, no `added_tokens.json`. Leia os dois arquivos juntos.
- Referência do log-mel sem baixar modelo: `uv venv` + `uv pip install openai-whisper torch numpy` e `whisper.log_mel_spectrogram(torch.from_numpy(whisper.pad_or_trim(x)), n_mels)`. Com `--extra-index-url` do PyPI junto, o uv escolheu o torch com CUDA (vários GB); para o torch, deixe só o índice `https://download.pytorch.org/whl/cpu`. Guarde só somatórios e alguns pontos no teste.
- O `realfft`/`rustfft` já está no `Cargo.lock` por causa do `rubato`: usar no núcleo não trouxe crate nova.
