# reqwest e SHA-256 dos modelos: o que vale saber

tipo: armadilha
origem: task #764 · claude · 2026-10-18

- O Tauri 2 já puxa o `reqwest` 0.13 (que no 0.13 usa aws-lc-rs por padrão no `rustls`). O app usa o `reqwest` 0.12 com `default-features = false, features = ["blocking", "rustls-tls"]`, que leva o `ring` e nenhum OpenSSL/aws-lc. Confira com `cargo tree -i openssl-sys --target all` (deve dar "did not match").
- No cliente **blocking**, `.timeout(d)` vale para cada operação (o `send` e cada `read` do corpo), não para o download inteiro: 30 s detecta conexão parada sem matar um download de 1,6 GB. No cliente async seria o tempo total.
- `reqwest::blocking` entra em pânico se for criado dentro de um runtime tokio: crie o cliente numa `std::thread` própria (não em `tauri::async_runtime`).
- Tamanho e SHA-256 reais dos ggml: `curl -s https://huggingface.co/api/models/ggerganov/whisper.cpp/tree/main` (campo `lfs.oid` e `lfs.size`). Os tamanhos do `DESIGN.md` (75/142/466 MB, 1,6 GB) são os rótulos usuais e não batem exatamente com MB ou MiB: não derive um do outro.
