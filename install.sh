#!/bin/sh
set -e

REPO="bdtfs/yubikey-notifier"
INSTALL_DIR="/usr/local/bin"
BINARY="yubikey-notifier"

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
  arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
  x86_64)        TARGET="x86_64-apple-darwin" ;;
  *)
    echo "Error: unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac

# Detect OS
OS=$(uname -s)
if [ "$OS" != "Darwin" ]; then
  echo "Error: yubikey-notifier only supports macOS" >&2
  exit 1
fi

# Get latest release tag
echo "Fetching latest release..."
TAG=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
if [ -z "$TAG" ]; then
  echo "Error: could not determine latest release" >&2
  exit 1
fi

URL="https://github.com/${REPO}/releases/download/${TAG}/${BINARY}-${TARGET}"

echo "Downloading ${BINARY} ${TAG} for ${TARGET}..."
curl -fSL "$URL" -o "/tmp/${BINARY}"
chmod +x "/tmp/${BINARY}"

echo "Installing to ${INSTALL_DIR}/${BINARY}..."
if [ -w "$INSTALL_DIR" ]; then
  mv "/tmp/${BINARY}" "${INSTALL_DIR}/${BINARY}"
else
  sudo mv "/tmp/${BINARY}" "${INSTALL_DIR}/${BINARY}"
fi

echo "Configuring as scdaemon wrapper..."
"${INSTALL_DIR}/${BINARY}" --setup

echo ""
echo "Done! Test with: echo test | gpg --sign > /dev/null"
