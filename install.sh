#!/bin/bash
set -e

REPO="foxl-ai/signal-cli"
INSTALL_DIR="/usr/local/bin"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
    Linux*)   PLATFORM="linux";;
    Darwin*)  PLATFORM="macos";;
    MINGW*|CYGWIN*|MSYS*) PLATFORM="windows";;
    *)        echo "Unsupported OS: ${OS}"; exit 1;;
esac

case "${ARCH}" in
    x86_64|amd64)  ARCH="x86_64";;
    aarch64|arm64) ARCH="aarch64";;
    *)             echo "Unsupported architecture: ${ARCH}"; exit 1;;
esac

ASSET_NAME="signal-cli-${PLATFORM}-${ARCH}"

echo "Detecting system: ${PLATFORM} ${ARCH}"

# Get latest release URL
LATEST_TAG=$(curl -sL "https://api.github.com/repos/${REPO}/releases/latest" \
    -H "Accept: application/vnd.github.v3+json" | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "${LATEST_TAG}" ]; then
    echo "Error: Could not fetch latest release. Is the repo accessible?"
    echo "If the repo is private, set GITHUB_TOKEN env var."
    exit 1
fi

echo "Latest version: ${LATEST_TAG}"

if [ "${PLATFORM}" = "windows" ]; then
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}.zip"
else
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}.tar.gz"
fi

TMPDIR=$(mktemp -d)
trap "rm -rf ${TMPDIR}" EXIT

echo "Downloading ${DOWNLOAD_URL}..."

if [ -n "${GITHUB_TOKEN}" ]; then
    curl -sL -H "Authorization: token ${GITHUB_TOKEN}" \
        -H "Accept: application/octet-stream" \
        "${DOWNLOAD_URL}" -o "${TMPDIR}/archive"
else
    curl -sL "${DOWNLOAD_URL}" -o "${TMPDIR}/archive"
fi

cd "${TMPDIR}"

if [ "${PLATFORM}" = "windows" ]; then
    unzip -q archive
else
    tar xzf archive
fi

echo "Installing to ${INSTALL_DIR}..."
if [ -w "${INSTALL_DIR}" ]; then
    cp signal-cli "${INSTALL_DIR}/" 2>/dev/null || true
    cp signal-mcp "${INSTALL_DIR}/" 2>/dev/null || true
    chmod +x "${INSTALL_DIR}/signal-cli" "${INSTALL_DIR}/signal-mcp" 2>/dev/null || true
else
    sudo cp signal-cli "${INSTALL_DIR}/" 2>/dev/null || true
    sudo cp signal-mcp "${INSTALL_DIR}/" 2>/dev/null || true
    sudo chmod +x "${INSTALL_DIR}/signal-cli" "${INSTALL_DIR}/signal-mcp" 2>/dev/null || true
fi

echo ""
echo "signal-cli installed successfully!"
echo ""
echo "Quick start:"
echo "  1. Link with your Signal account:"
echo "     signal-cli link-device -n \"my-cli\""
echo "  2. Scan the QR code with your Signal app"
echo "  3. Send a message:"
echo "     signal-cli send -n +1234567890 -m \"Hello!\""
echo ""
