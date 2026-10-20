#!/bin/sh
# Depois de instalar o .deb: aplica a regra do /dev/uinput (70-pointspeak-uinput.rules) agora,
# sem reiniciar. Falha aqui não impede a instalação (a regra vale no próximo boot).
set -e
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules || true
  udevadm trigger --subsystem-match=misc --sysname-match=uinput || true
fi
exit 0
