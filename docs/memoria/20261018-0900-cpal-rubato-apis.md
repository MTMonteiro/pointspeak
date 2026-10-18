# cpal 0.18 e rubato 5: APIs que mudaram

tipo: armadilha
origem: task #763 · claude · 2026-10-18

- `cpal` 0.18: o nome do dispositivo vem de `device.description()?.name()` (não há mais `name()`); `id()` é um id estável de host. `SampleRate` é `u32`. Erros são `cpal::Error` com `kind()` (`Xrun`, `DeviceNotAvailable`, `StreamInvalidated`...).
- Converter amostra: `s.to_sample::<f32>()` (o trait vem pelo bound `SizedSample`); `f32::from_sample` não existe.
- `rubato` 5 usa adaptadores (`rubato::audioadapter_buffers::direct::InterleavedSlice`) e `process_into_buffer` com `Indexing { partial_len, .. }`; não há mais `FftFixedIn`/`process(&[Vec])` do 0.16. Para streaming, descarte `output_delay()` amostras no início e bombeie blocos vazios (`partial_len: Some(0)`) no fim até `ceil(n × razão)`; veja `MonoResampler` em `pointspeak-core/src/audio.rs`.
- Comando Tauri com o mesmo nome da função pública: o erro precisa de `Serialize` (implementamos serializando `to_string()`), senão o `generate_handler!` não aceita `Result<_, AudioError>`.
