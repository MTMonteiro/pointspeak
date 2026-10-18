# cargo fora do PATH dos checks do runner

tipo: armadilha
origem: task #753 · claude · 2026-10-18

- Os checks do runner rodam em `/bin/sh` sem `~/.cargo/bin` no PATH: a 1ª tentativa falhou com `cargo: not found` mesmo com tudo passando no terminal do agente.
- `./scripts/setup-linux.sh` agora cria links dos proxies do rustup (`cargo`, `rustc`, `rustfmt`, `cargo-fmt`, `cargo-clippy`, `clippy-driver`, `rustup`) em `/usr/local/bin`. Já rodado neste runner.
- Para simular o ambiente dos checks: `env -i HOME=$HOME /bin/sh -c 'cargo --version'`.
