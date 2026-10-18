#!/usr/bin/env bash
# Gera os pacotes Linux (.deb e AppImage) do PointSpeak com o ONNX Runtime (CPU EP) e o Silero VAD.
# Pré-requisito (uma vez por máquina): ./scripts/setup-linux.sh
# Saída: target/release/bundle/deb/PointSpeak_<versão>_amd64.deb
#        target/release/bundle/appimage/PointSpeak_<versão>_amd64.AppImage
# O libonnxruntime.so do ./scripts/fetch-onnxruntime.sh vai para a pasta de recursos do app
# (/usr/lib/PointSpeak/ no .deb, $APPDIR/usr/lib/PointSpeak/ no AppImage), onde o
# pointspeak_core::onnx_runtime o procura. O AppImage baixa o linuxdeploy da rede no primeiro
# build; se falhar, o .deb continua sendo a entrega (POINTSPEAK_SKIP_APPIMAGE=1 pula o AppImage).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
for tool in cargo npx dpkg-deb readelf; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "build-linux.sh: '$tool' não encontrado; rode ./scripts/setup-linux.sh" >&2
    exit 1
  fi
done

# whisper.cpp portátil (mesmos valores do build-windows.sh): sem -march=native da máquina de build
# e sem OpenMP (evita depender do libgomp1); CPU fixa em AVX2 + FMA + F16C.
export GGML_NATIVE="${GGML_NATIVE:-OFF}"
export GGML_OPENMP="${GGML_OPENMP:-OFF}"
export GGML_AVX="${GGML_AVX:-ON}"
export GGML_AVX2="${GGML_AVX2:-ON}"
export GGML_FMA="${GGML_FMA:-ON}"
export GGML_F16C="${GGML_F16C:-ON}"

if [ ! -d node_modules ]; then
  npm ci --no-audit --no-fund
fi

# ONNX Runtime (CPU) carregado em tempo de execução: entra nos pacotes como recurso. A config de
# overlay só é usada aqui, para o `cargo build` e o `generate_context!` não exigirem o arquivo.
"$ROOT/scripts/fetch-onnxruntime.sh" linux
ORT_OVERLAY="$ROOT/src-tauri/tauri.onnxruntime-linux.conf.json"

VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
BUNDLE="$ROOT/target/release/bundle"
DEB="$BUNDLE/deb/PointSpeak_${VERSION}_amd64.deb"
APPIMAGE="$BUNDLE/appimage/PointSpeak_${VERSION}_amd64.AppImage"
# Pacotes de um build anterior não podem passar pela conferência abaixo.
rm -f "$DEB" "$APPIMAGE"

BUNDLES=deb,appimage
if [ "${POINTSPEAK_SKIP_APPIMAGE:-0}" = 1 ]; then
  BUNDLES=deb
fi
# O linuxdeploy é um AppImage: APPIMAGE_EXTRACT_AND_RUN o roda sem FUSE no host. O .deb sai antes
# do AppImage; uma falha do AppImage (rede, linuxdeploy) não impede a conferência do .deb abaixo.
if ! APPIMAGE_EXTRACT_AND_RUN=1 npx tauri build --bundles "$BUNDLES" --config "$ORT_OVERLAY"; then
  echo "build-linux.sh: tauri build falhou; conferindo o que foi gerado" >&2
fi

if [ ! -f "$DEB" ]; then
  echo "build-linux.sh: pacote não encontrado em $DEB" >&2
  exit 1
fi

# load-dynamic: o binário não pode ter o onnxruntime como dependência (senão nem abre sem ele).
APP="$ROOT/target/release/pointspeak"
if readelf -d "$APP" | grep -i 'NEEDED.*onnxruntime' >/dev/null; then
  echo "build-linux.sh: pointspeak linka o libonnxruntime.so" >&2
  exit 1
fi

# dpkg-deb -c lista "./usr/..." ou "usr/..." conforme a versão: normaliza para "usr/...".
CONTENTS="$(dpkg-deb -c "$DEB" | awk '{print $6}' | sed 's|^\./||')"
for file in usr/lib/PointSpeak/libonnxruntime.so usr/lib/PointSpeak/resources/silero_vad.onnx \
  usr/lib/PointSpeak/licenses/onnxruntime-LICENSE.txt usr/bin/pointspeak; do
  if ! grep -Fx "$file" <<<"$CONTENTS" >/dev/null; then
    echo "build-linux.sh: $file não está no .deb" >&2
    exit 1
  fi
done
echo "build-linux.sh: libonnxruntime.so e o Silero VAD no .deb; pointspeak sem dependência do ONNX Runtime"
echo "build-linux.sh: pacotes gerados:"
ls -l "$DEB"
if [ -f "$APPIMAGE" ]; then
  ls -l "$APPIMAGE"
  # O AppImage é um runtime ELF + squashfs: lista o squashfs a partir do offset que o runtime informa.
  if command -v unsquashfs >/dev/null 2>&1; then
    OFFSET="$("$APPIMAGE" --appimage-offset)"
    APPIMAGE_FILES="$(unsquashfs -o "$OFFSET" -l "$APPIMAGE" | sed 's|^squashfs-root/||')"
    for file in usr/lib/PointSpeak/libonnxruntime.so usr/lib/PointSpeak/resources/silero_vad.onnx; do
      if ! grep -Fx "$file" <<<"$APPIMAGE_FILES" >/dev/null; then
        echo "build-linux.sh: $file não está no AppImage" >&2
        exit 1
      fi
    done
    echo "build-linux.sh: libonnxruntime.so e o Silero VAD no AppImage"
  fi
else
  echo "build-linux.sh: AppImage não gerado (veja o log acima); o .deb é a entrega"
fi
