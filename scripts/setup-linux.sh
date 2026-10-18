#!/usr/bin/env bash
# Instala as bibliotecas de sistema que o Tauri v2 (e, nas próximas fases,
# cpal e whisper.cpp) precisam para compilar no Linux (Debian/Ubuntu).
# Idempotente: só chama o apt-get quando falta algum pacote.
set -euo pipefail

PACKAGES=(
  libwebkit2gtk-4.1-dev
  libgtk-3-dev
  libayatana-appindicator3-dev
  librsvg2-dev
  libasound2-dev
  libxdo-dev
  libxkbcommon-dev
  libssl-dev
  libx11-dev
  libxcb1-dev
  pkg-config
  build-essential
  cmake
  clang
  libclang-dev
)

if ! command -v apt-get >/dev/null 2>&1; then
  echo "setup-linux.sh: apt-get não encontrado; este script cobre só Debian/Ubuntu." >&2
  exit 1
fi

missing=()
for pkg in "${PACKAGES[@]}"; do
  if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "install ok installed"; then
    missing+=("$pkg")
  fi
done

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  SUDO="sudo"
fi

if [ ${#missing[@]} -eq 0 ]; then
  echo "setup-linux.sh: todos os pacotes já estão instalados."
else
  echo "setup-linux.sh: instalando ${missing[*]}"
  $SUDO apt-get update
  DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y --no-install-recommends "${missing[@]}"
fi

# O rustup instala em ~/.cargo/bin, que só entra no PATH de shells de login.
# Os checks do runner rodam em /bin/sh sem esse PATH ("cargo: not found"), então
# expõe os proxies do rustup em /usr/local/bin (só os que ainda não existem lá).
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
if [ -x "$CARGO_BIN/cargo" ]; then
  for tool in cargo rustc rustup rustfmt cargo-fmt cargo-clippy clippy-driver; do
    if [ -x "$CARGO_BIN/$tool" ] && [ ! -e "/usr/local/bin/$tool" ]; then
      $SUDO ln -s "$CARGO_BIN/$tool" "/usr/local/bin/$tool"
      echo "setup-linux.sh: /usr/local/bin/$tool -> $CARGO_BIN/$tool"
    fi
  done
else
  echo "setup-linux.sh: rustup não encontrado em $CARGO_BIN; instale em https://rustup.rs" >&2
fi
echo "setup-linux.sh: pronto."
