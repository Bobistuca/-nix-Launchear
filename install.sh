#!/usr/bin/env sh
set -eu

REPO="Bobistuca/-nix-Launchear"
BASE="https://github.com/${REPO}/releases/latest/download"

if [ "$(uname -m)" != "x86_64" ]; then
  echo "Nix Launcher currently ships x86_64 builds only." >&2
  exit 1
fi

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fL --progress-bar -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$2" "$1"
  else
    echo "curl or wget is required." >&2
    exit 1
  fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if command -v apt-get >/dev/null 2>&1 && [ -z "${NIX_APPIMAGE:-}" ]; then
  echo "Installing the .deb package..."
  fetch "${BASE}/nix-launcher_amd64.deb" "${TMP}/nix-launcher_amd64.deb"
  if [ "$(id -u)" -eq 0 ]; then
    apt-get install -y "${TMP}/nix-launcher_amd64.deb"
  else
    sudo apt-get install -y "${TMP}/nix-launcher_amd64.deb"
  fi
else
  echo "Installing the AppImage..."
  BIN_DIR="${HOME}/.local/bin"
  APP_DIR="${HOME}/.local/share/applications"
  ICON_DIR="${HOME}/.local/share/icons/hicolor/scalable/apps"
  mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR"
  fetch "${BASE}/nix-launcher-x86_64.AppImage" "${BIN_DIR}/nix-launcher"
  chmod +x "${BIN_DIR}/nix-launcher"
  fetch "https://raw.githubusercontent.com/${REPO}/main/packaging/nix-launcher.svg" "${ICON_DIR}/nix-launcher.svg"
  cat > "${APP_DIR}/nix-launcher.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Nix Launcher
Comment=Minimalist black and white Minecraft launcher
Exec=${BIN_DIR}/nix-launcher
Icon=nix-launcher
Terminal=false
Categories=Game;
DESKTOP
  echo "Installed to ${BIN_DIR}/nix-launcher (make sure it is in your PATH)."
fi

echo "Done. Run: nix-launcher"
