#!/usr/bin/env bash
# Gera o instalador Windows (NSIS) do PointSpeak por compilação cruzada neste Linux,
# pelo caminho documentado do Tauri v2: cargo-xwin + x86_64-pc-windows-msvc + NSIS.
# Pré-requisito (uma vez por máquina): ./scripts/setup-windows-cross.sh
# Saída: target/x86_64-pc-windows-msvc/release/bundle/nsis/PointSpeak_<versão>_x64-setup.exe
# (com onnxruntime.dll + DirectML.dll do ./scripts/fetch-onnxruntime.sh ao lado do pointspeak.exe)
# Sem assinatura de código: o Windows SmartScreen avisa ao abrir (esperado).
set -euo pipefail

TARGET=x86_64-pc-windows-msvc
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
# O cargo-xwin cria o clang-cl a partir do primeiro `clang` do PATH; a STL do MSVC pede Clang >= 19.
LLVM_DIR="${LLVM_DIR:-/usr/lib/llvm-19/bin}"
if [ ! -x "$LLVM_DIR/clang" ]; then
  echo "build-windows.sh: clang 19 não encontrado em $LLVM_DIR; rode ./scripts/setup-windows-cross.sh" >&2
  exit 1
fi
export PATH="$LLVM_DIR:$PATH"
for tool in cargo-xwin makensis lld-link llvm-rc ninja; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "build-windows.sh: '$tool' não encontrado; rode ./scripts/setup-windows-cross.sh" >&2
    exit 1
  fi
done

# whisper.cpp (whisper-rs-sys repassa as variáveis GGML_* ao cmake): sem -march=native da máquina
# de build e sem OpenMP (evita a DLL do runtime OpenMP); CPU fixa em AVX2 + FMA + F16C. Máquinas
# sem AVX2 (anteriores a ~2013) não rodam o .exe: limitação documentada em docs/tecnologia.md.
export GGML_NATIVE="${GGML_NATIVE:-OFF}"
export GGML_OPENMP="${GGML_OPENMP:-OFF}"
export GGML_AVX="${GGML_AVX:-ON}"
export GGML_AVX2="${GGML_AVX2:-ON}"
export GGML_FMA="${GGML_FMA:-ON}"
export GGML_F16C="${GGML_F16C:-ON}"
# whisper.cpp com /MT (STL estática, libcpmt): com o /MD padrão o C++ importaria a STL do
# MSVCP140.dll (VC++ Redistributable), que o instalador não traz. O resto segue o "CRT híbrido"
# do Tauri (static_vcruntime: libcmt + libvcruntime estáticos, UCRT do sistema); não use
# `+crt-static`, que remove o UCRT dinâmico e quebra o link. O CMakeLists do whisper.cpp usa a
# política antiga (CMP0091 OLD), então o /MD de CMAKE_*_FLAGS_RELEASE precisa ser trocado aqui.
export CMAKE_C_FLAGS_RELEASE="${CMAKE_C_FLAGS_RELEASE:-/MT /O2 /Ob2 /DNDEBUG}"
export CMAKE_CXX_FLAGS_RELEASE="${CMAKE_CXX_FLAGS_RELEASE:-/MT /O2 /Ob2 /DNDEBUG}"

if [ ! -d node_modules ]; then
  npm ci --no-audit --no-fund
fi

# ONNX Runtime (DirectML) carregado em tempo de execução: as DLLs entram no instalador como
# recursos, ao lado do pointspeak.exe. A config de overlay só é usada aqui, para o `cargo build`
# do Linux e o `generate_context!` não exigirem os arquivos.
"$ROOT/scripts/fetch-onnxruntime.sh" windows
ORT_OVERLAY="$ROOT/src-tauri/tauri.onnxruntime-windows.conf.json"

npx tauri build --runner cargo-xwin --target "$TARGET" --bundles nsis --config "$ORT_OVERLAY"

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

# load-dynamic: o .exe não pode importar o onnxruntime.dll (senão nem abre sem ele).
APP="$ROOT/target/$TARGET/release/pointspeak.exe"
if llvm-objdump -p "$APP" | grep -i 'DLL Name: onnxruntime' >/dev/null; then
  echo "build-windows.sh: pointspeak.exe importa o onnxruntime.dll de forma estática" >&2
  exit 1
fi
NSI="$ROOT/target/$TARGET/release/nsis/x64/installer.nsi"
for dll in onnxruntime.dll DirectML.dll onnxruntime_providers_openvino.dll openvino.dll \
  openvino_intel_npu_plugin.dll 'resources\silero_vad.onnx'; do
  if ! grep -F "$dll" "$NSI" >/dev/null; then
    echo "build-windows.sh: $dll não está no instalador ($NSI)" >&2
    exit 1
  fi
done
echo "build-windows.sh: onnxruntime.dll, DirectML.dll, a EP OpenVINO e o Silero VAD no instalador; pointspeak.exe sem import estático do ONNX Runtime"
ls -l "$EXE"
