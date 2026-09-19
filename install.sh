#!/usr/bin/env bash
# Instalador de Sonido para Linux.
#
#   ./install.sh              # instala dependencias (si puede), compila release e instala en ~/.local/bin
#   ./install.sh --no-deps    # no toca los paquetes del sistema
#   ./install.sh --debug      # build de debug (compila más rápido)
#   ./install.sh --uninstall  # borra binario, .desktop y datos
#
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

PROFILE="--release"
TARGET_DIR="target/release"
WITH_DEPS=1
UNINSTALL=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug) PROFILE=""; TARGET_DIR="target/debug" ;;
    --no-deps) WITH_DEPS=0 ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "opción desconocida: $1" >&2; exit 2 ;;
  esac
  shift
done

PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/sonido"
APPDIR="$PREFIX/share/applications"

say()  { printf '\033[1;35m▸\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m✔\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m✘\033[0m %s\n' "$*" >&2; exit 1; }

if [[ $UNINSTALL -eq 1 ]]; then
  say "Desinstalando Sonido"
  rm -f "$BIN" "$APPDIR/sonido.desktop"
  read -r -p "¿Borrar también configuración y caché (~/.config/sonido, ~/.cache/sonido, ~/.local/share/sonido)? [y/N] " a
  if [[ "$a" == "y" || "$a" == "Y" ]]; then
    rm -rf "$HOME/.config/sonido" "$HOME/.cache/sonido" "$HOME/.local/share/sonido"
  fi
  ok "listo"
  exit 0
fi

[[ "$(uname -s)" == "Linux" ]] || warn "Sonido apunta a Linux; en otros sistemas puede no compilar"

# ── 1. dependencias del sistema ──
if [[ $WITH_DEPS -eq 1 ]]; then
  say "Detectando gestor de paquetes"
  PKG=""
  for c in apt-get dnf pacman zypper apk xbps-install; do
    if command -v "$c" >/dev/null 2>&1; then PKG="$c"; break; fi
  done

  SUDO=""
  if [[ $EUID -ne 0 ]]; then
    if command -v sudo >/dev/null 2>&1; then SUDO="sudo"; fi
  fi

  case "$PKG" in
    apt-get)
      say "Instalando dependencias (apt)"
      $SUDO apt-get update -qq
      $SUDO apt-get install -y --no-install-recommends \
        build-essential pkg-config curl ca-certificates \
        libmpv-dev libasound2-dev libxkbcommon-dev libgl1-mesa-dev \
        libwayland-dev libfontconfig1-dev
      ;;
    dnf)
      say "Instalando dependencias (dnf)"
      $SUDO dnf install -y gcc gcc-c++ pkgconf-pkg-config mpv-devel \
        alsa-lib-devel libxkbcommon-devel mesa-libGL-devel wayland-devel fontconfig-devel
      ;;
    pacman)
      say "Instalando dependencias (pacman)"
      $SUDO pacman -S --needed --noconfirm base-devel pkgconf mpv alsa-lib \
        libxkbcommon mesa wayland fontconfig
      ;;
    zypper)
      say "Instalando dependencias (zypper)"
      $SUDO zypper install -y gcc gcc-c++ pkg-config libmpv-devel alsa-devel \
        libxkbcommon-devel Mesa-libGL-devel libwayland-devel fontconfig-devel
      ;;
    apk)
      say "Instalando dependencias (apk)"
      $SUDO apk add build-base pkgconf mpv-dev alsa-lib-dev libxkbcommon-dev \
        mesa-dev wayland-dev fontconfig-dev
      ;;
    *)
      warn "No reconocí el gestor de paquetes. Necesitás: compilador C, pkg-config, libmpv-dev y libasound2-dev"
      ;;
  esac
fi

# ── 2. toolchain de Rust ──
if ! command -v cargo >/dev/null 2>&1; then
  if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    say "Instalando Rust (rustup)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
fi
ok "cargo $(cargo --version | cut -d' ' -f2)"

# ── 3. build ──
if pkg-config --exists mpv 2>/dev/null; then
  ok "libmpv $(pkg-config --modversion mpv) → gapless + FLAC Hi-Res + Dolby Atmos"
else
  warn "libmpv no encontrada: se compila con el backend de audio 100% Rust (rodio)"
  warn "  para habilitar mpv instalá libmpv-dev y volvé a correr este script"
fi

NPROC=$(nproc 2>/dev/null || echo 2)
JOBS=$(( NPROC > 2 ? 2 : NPROC ))   # wgpu/eframe necesitan RAM; con poca memoria usá -j 1
say "Compilando (cargo build $PROFILE -j $JOBS)"
# shellcheck disable=SC2086
cargo build $PROFILE -j "$JOBS"

[[ -x "$TARGET_DIR/sonido" ]] || die "no se generó $TARGET_DIR/sonido"

# ── 4. instalar ──
mkdir -p "$PREFIX/bin" "$APPDIR"
install -m 0755 "$TARGET_DIR/sonido" "$BIN"
ok "binario: $BIN  ($(du -h "$BIN" | cut -f1))"

sed "s|@BIN@|$BIN|; s|@ICON@|$ROOT/assets/sonido.svg|" "$ROOT/packaging/sonido.desktop" \
  > "$APPDIR/sonido.desktop"
chmod 0644 "$APPDIR/sonido.desktop"
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APPDIR" >/dev/null 2>&1 || true
fi
ok "entrada de escritorio: $APPDIR/sonido.desktop"

cat <<EOF

$(printf '\033[1;32mSonido instalado\033[0m')

  Asegurate de que $PREFIX/bin esté en tu PATH:
      export PATH="$PREFIX/bin:\$PATH"

  Arrancar:
      sonido                       # interfaz gráfica
      sonido add <url-manifest>    # instalar un addon
      sonido search "daft punk"    # buscar por consola
      sonido play --addon <id> <track-id>
      sonido info                  # rutas, caché y motor de audio

  Addon de prueba incluido (no necesita red):
      python3 tools/mock_addon.py
      sonido add http://127.0.0.1:8787/manifest.json

EOF
