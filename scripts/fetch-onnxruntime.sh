#!/usr/bin/env bash
# Baixa o ONNX Runtime oficial que o PointSpeak carrega em tempo de execução (ort `load-dynamic`).
# Uso: ./scripts/fetch-onnxruntime.sh [windows|linux|all]   (padrão: all)
# Saída (fora do git, em target/):
#   target/onnxruntime/windows-x64/onnxruntime.dll + DirectML.dll
#     (NuGet Microsoft.ML.OnnxRuntime.DirectML + Microsoft.AI.DirectML)
#   target/onnxruntime/linux-x64/libonnxruntime.so   (release onnxruntime-linux-x64, só CPU)
# Idempotente: os pacotes ficam em target/onnxruntime/downloads/ e só são baixados de novo se o
# SHA-256 não conferir. Versões e hashes fixados aqui; troque os três juntos.
set -euo pipefail

ORT_VERSION=1.24.4
ORT_DIRECTML_SHA256=57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681
DIRECTML_VERSION=1.15.4
DIRECTML_SHA256=4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9
ORT_LINUX_SHA256=3a211fbea252c1e66290658f1b735b772056149f28321e71c308942cdb54b747

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/target/onnxruntime"
DOWNLOADS="$OUT/downloads"
WHAT="${1:-all}"

case "$WHAT" in
  windows|linux|all) ;;
  *)
    echo "uso: $0 [windows|linux|all]" >&2
    exit 2
    ;;
esac
for tool in curl sha256sum unzip tar; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "fetch-onnxruntime.sh: '$tool' não encontrado" >&2
    exit 1
  fi
done
mkdir -p "$DOWNLOADS"

# fetch <url> <arquivo> <sha256>: baixa para downloads/ (se faltar ou o hash não conferir) e confere.
fetch() {
  local url="$1" file="$DOWNLOADS/$2" sha="$3"
  if [ -f "$file" ] && echo "$sha  $file" | sha256sum --check --status; then
    return
  fi
  echo "fetch-onnxruntime.sh: baixando $url"
  curl --fail --location --silent --show-error --retry 3 -o "$file.part" "$url"
  if ! echo "$sha  $file.part" | sha256sum --check --status; then
    echo "fetch-onnxruntime.sh: SHA-256 de $2 não confere (esperado $sha)" >&2
    rm -f "$file.part"
    exit 1
  fi
  mv "$file.part" "$file"
}

# extract <arquivo.zip> <caminho dentro> <destino>
extract() {
  unzip -p "$DOWNLOADS/$1" "$2" >"$3.part"
  mv "$3.part" "$3"
}

windows() {
  local nuget=https://api.nuget.org/v3-flatcontainer
  local ort="microsoft.ml.onnxruntime.directml.$ORT_VERSION.nupkg"
  local dml="microsoft.ai.directml.$DIRECTML_VERSION.nupkg"
  fetch "$nuget/microsoft.ml.onnxruntime.directml/$ORT_VERSION/$ort" "$ort" "$ORT_DIRECTML_SHA256"
  fetch "$nuget/microsoft.ai.directml/$DIRECTML_VERSION/$dml" "$dml" "$DIRECTML_SHA256"
  local dir="$OUT/windows-x64"
  mkdir -p "$dir"
  extract "$ort" runtimes/win-x64/native/onnxruntime.dll "$dir/onnxruntime.dll"
  extract "$ort" LICENSE "$dir/onnxruntime-LICENSE.txt"
  extract "$dml" bin/x64-win/DirectML.dll "$dir/DirectML.dll"
  extract "$dml" LICENSE.txt "$dir/DirectML-LICENSE.txt"
  echo "fetch-onnxruntime.sh: $dir"
}

linux() {
  local tgz="onnxruntime-linux-x64-$ORT_VERSION.tgz"
  fetch "https://github.com/microsoft/onnxruntime/releases/download/v$ORT_VERSION/$tgz" "$tgz" "$ORT_LINUX_SHA256"
  local dir="$OUT/linux-x64"
  mkdir -p "$dir"
  tar -xzf "$DOWNLOADS/$tgz" -O "onnxruntime-linux-x64-$ORT_VERSION/lib/libonnxruntime.so.$ORT_VERSION" >"$dir/libonnxruntime.so.part"
  mv "$dir/libonnxruntime.so.part" "$dir/libonnxruntime.so"
  tar -xzf "$DOWNLOADS/$tgz" -O "onnxruntime-linux-x64-$ORT_VERSION/LICENSE" >"$dir/onnxruntime-LICENSE.txt"
  echo "fetch-onnxruntime.sh: $dir"
}

case "$WHAT" in
  windows) windows ;;
  linux) linux ;;
  all) windows; linux ;;
esac
