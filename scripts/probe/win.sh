#!/bin/bash
# Команда cmd на Windows-машине с пробой через реверс-туннель (порт 2222).
exec timeout ${WIN_TIMEOUT:-120} ssh -p 2222 -o ConnectTimeout=5 -o BatchMode=yes \
  -o IdentityFile=/root/.ssh/id_ed25519_winmount -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=no Administrator@localhost "$@"
