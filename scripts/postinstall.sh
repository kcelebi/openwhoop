#!/bin/bash
# Post-install script to download the correct binary for this platform

set -e

PACKAGE_NAME="openwhoop"
OWNER="kcelebi"
REPO="openwhoop"
BINARY_NAME="openwhoop"

# Detect platform
case "$(uname -s)" in
    Linux*)     PLATFORM="linux";;
    Darwin*)    PLATFORM="darwin";;
    *)          echo "Unsupported platform"; exit 1;;
esac

case "$(uname -m)" in
    x86_64)     ARCH="x86_64";;
    aarch64|arm64) ARCH="arm64";;
    *)          echo "Unsupported architecture"; exit 1;;
esac

# Map to GitHub release asset naming convention
if [ "$PLATFORM" = "darwin" ]; then
    ASSET_NAME="${BINARY_NAME}-${PLATFORM}-${ARCH}"
else
    ASSET_NAME="${BINARY_NAME}-${PLATFORM}-${ARCH}"
fi

# Get latest release tag
TAG=$(curl -sL "https://api.github.com/repos/${OWNER}/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"v?([^"]+)".*/\1/')

if [ -z "$TAG" ]; then
    echo "Could not determine latest release tag"
    exit 1
fi

echo "Downloading ${PACKAGE_NAME} v${TAG} for ${PLATFORM}-${ARCH}..."

# Download the binary
DOWNLOAD_URL="https://github.com/${OWNER}/${REPO}/releases/download/v${TAG}/${ASSET_NAME}"

mkdir -p "${HOME}/.local/bin"
curl -sL "${DOWNLOAD_URL}" -o "${HOME}/.local/bin/${BINARY_NAME}"
chmod +x "${HOME}/.local/bin/${BINARY_NAME}"

echo "Installed ${BINARY_NAME} to ${HOME}/.local/bin/"
echo "Add ${HOME}/.local/bin to your PATH to use it"