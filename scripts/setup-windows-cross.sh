#!/usr/bin/env bash
# Prepara este Linux (Debian/Ubuntu) para gerar o instalador Windows (.exe NSIS)
# por compilação cruzada, como documentado pelo Tauri v2: cargo-xwin + target
# x86_64-pc-windows-msvc + NSIS + lld/llvm.
# Idempotente: só instala o que falta. Rode antes uma vez o ./scripts/setup-linux.sh.
set -euo pipefail

TARGET=x86_64-pc-windows-msvc
PACKAGES=(nsis lld llvm clang)

if ! command -v apt-get >/dev/null 2>&1; then
  echo "setup-windows-cross.sh: apt-get não encontrado; este script cobre só Debian/Ubuntu." >&2
  exit 1
fi

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  SUDO="sudo"
fi

missing=()
for pkg in "${PACKAGES[@]}"; do
  if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "install ok installed"; then
    missing+=("$pkg")
  fi
done

if [ ${#missing[@]} -eq 0 ]; then
  echo "setup-windows-cross.sh: pacotes apt já instalados."
else
  echo "setup-windows-cross.sh: instalando ${missing[*]}"
  $SUDO apt-get update
  DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y "${missing[@]}"
fi

CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
export PATH="$CARGO_BIN:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
  echo "setup-windows-cross.sh: rustup não encontrado; instale em https://rustup.rs" >&2
  exit 1
fi

# `rustup target add` já é idempotente; o `cd` garante o toolchain do rust-toolchain.toml.
cd "$(dirname "$0")/.."
rustup target add "$TARGET"

if command -v cargo-xwin >/dev/null 2>&1; then
  echo "setup-windows-cross.sh: cargo-xwin já instalado ($(cargo-xwin --version 2>/dev/null || echo '?'))."
else
  cargo install --locked cargo-xwin
fi

echo "setup-windows-cross.sh: pronto. Gere o instalador com ./scripts/build-windows.sh"
