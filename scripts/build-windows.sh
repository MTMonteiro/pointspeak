#!/usr/bin/env bash
# Gera o instalador Windows (NSIS) do PointSpeak por compilação cruzada neste Linux,
# pelo caminho documentado do Tauri v2: cargo-xwin + x86_64-pc-windows-msvc + NSIS.
# Pré-requisito (uma vez por máquina): ./scripts/setup-windows-cross.sh
# Saída: target/x86_64-pc-windows-msvc/release/bundle/nsis/PointSpeak_<versão>_x64-setup.exe
# Sem assinatura de código: o Windows SmartScreen avisa ao abrir (esperado).
set -euo pipefail

TARGET=x86_64-pc-windows-msvc
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
for tool in cargo-xwin makensis lld-link llvm-rc; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "build-windows.sh: '$tool' não encontrado; rode ./scripts/setup-windows-cross.sh" >&2
    exit 1
  fi
done

if [ ! -d node_modules ]; then
  npm ci --no-audit --no-fund
fi

npx tauri build --runner cargo-xwin --target "$TARGET" --bundles nsis

VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
EXE="$ROOT/target/$TARGET/release/bundle/nsis/PointSpeak_${VERSION}_x64-setup.exe"
if [ ! -f "$EXE" ]; then
  echo "build-windows.sh: instalador não encontrado em $EXE" >&2
  exit 1
fi
echo "build-windows.sh: instalador gerado:"
echo "$EXE"
if command -v file >/dev/null 2>&1; then
  file "$EXE"
fi
