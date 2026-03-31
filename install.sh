#!/usr/bin/env bash
set -euo pipefail

# rs-code installer
# Usage: curl -fsSL https://raw.githubusercontent.com/avala-ai/rs-code/main/install.sh | bash

REPO="avala-ai/rs-code"
BINARY="rc"
INSTALL_DIR="${RC_INSTALL_DIR:-/usr/local/bin}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

info() { echo -e "${CYAN}${BOLD}==>${RESET} $1"; }
success() { echo -e "${GREEN}${BOLD}==>${RESET} $1"; }
error() { echo -e "${RED}${BOLD}error:${RESET} $1" >&2; exit 1; }

# Detect OS and architecture
detect_platform() {
    local os arch

    case "$(uname -s)" in
        Linux*)  os="linux" ;;
        Darwin*) os="macos" ;;
        *)       error "Unsupported OS: $(uname -s). Use cargo install rs-code instead." ;;
    esac

    case "$(uname -m)" in
        x86_64|amd64)  arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)             error "Unsupported architecture: $(uname -m). Use cargo install rs-code instead." ;;
    esac

    echo "${os}-${arch}"
}

# Get the latest release version
get_latest_version() {
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' \
        | head -1 \
        | sed 's/.*"tag_name": *"//;s/".*//'
}

main() {
    info "Installing rs-code..."

    local platform version url tmpdir

    platform=$(detect_platform)
    info "Detected platform: ${platform}"

    version=$(get_latest_version)
    if [ -z "$version" ]; then
        error "Could not determine latest version. Check https://github.com/${REPO}/releases"
    fi
    info "Latest version: ${version}"

    url="https://github.com/${REPO}/releases/download/${version}/rc-${platform}.tar.gz"
    info "Downloading ${url}..."

    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' EXIT

    if ! curl -fsSL "$url" -o "${tmpdir}/rc.tar.gz"; then
        error "Download failed. Check that a release exists for your platform at:\n  https://github.com/${REPO}/releases"
    fi

    tar xzf "${tmpdir}/rc.tar.gz" -C "$tmpdir"

    if [ ! -f "${tmpdir}/${BINARY}" ]; then
        error "Binary not found in archive. The release may be packaged differently."
    fi

    # Install
    if [ -w "$INSTALL_DIR" ]; then
        mv "${tmpdir}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
    else
        info "Installing to ${INSTALL_DIR} (requires sudo)..."
        sudo mv "${tmpdir}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
    fi

    chmod +x "${INSTALL_DIR}/${BINARY}"

    # Verify
    if command -v "$BINARY" &>/dev/null; then
        success "rs-code ${version} installed to ${INSTALL_DIR}/${BINARY}"
        echo ""
        echo "  ${BOLD}${BINARY} --version${RESET}"
        "$BINARY" --version 2>/dev/null || true
        echo ""
        echo "  Get started:"
        echo "    export RC_API_KEY=\"your-api-key\""
        echo "    ${BINARY}"
        echo ""
        echo "  Docs: https://avala-ai.github.io/rs-code/"
    else
        success "Installed to ${INSTALL_DIR}/${BINARY}"
        echo ""
        echo "  Make sure ${INSTALL_DIR} is in your PATH:"
        echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
    fi
}

main "$@"
