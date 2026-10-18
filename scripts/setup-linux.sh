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

if [ ${#missing[@]} -eq 0 ]; then
  echo "setup-linux.sh: todos os pacotes já estão instalados."
  exit 0
fi

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  SUDO="sudo"
fi

echo "setup-linux.sh: instalando ${missing[*]}"
$SUDO apt-get update
DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y --no-install-recommends "${missing[@]}"
echo "setup-linux.sh: pronto."
