#!/usr/bin/env bash
set -euo pipefail

# agent-code installer
# Usage: curl -fsSL https://raw.githubusercontent.com/avala-ai/agent-code/main/install.sh | bash
#
# Environment overrides:
#   AGENT_CODE_INSTALL_DIR   Directory to install the binary into (default: /usr/local/bin)
#   AGENT_CODE_NO_SHELL_SETUP=1
#                            Skip writing the shell name guard (see below). The binary is
#                            still installed, but `agent` may be shadowed by another command
#                            of the same name that appears earlier in your PATH.

REPO="avala-ai/agent-code"
BINARY="agent"
INSTALL_DIR="${AGENT_CODE_INSTALL_DIR:-/usr/local/bin}"

# Markers delimiting the block we manage in shell rc files. Kept stable so the
# block can be found, replaced in place, and cleanly removed later.
GUARD_BEGIN="# >>> agent-code name guard >>>"
GUARD_END="# <<< agent-code name guard <<<"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

info() { echo -e "${CYAN}${BOLD}==>${RESET} $1"; }
success() { echo -e "${GREEN}${BOLD}==>${RESET} $1"; }
warn() { echo -e "${YELLOW}${BOLD}warning:${RESET} $1" >&2; }
error() { echo -e "${RED}${BOLD}error:${RESET} $1" >&2; exit 1; }

# Detect OS and architecture
detect_platform() {
    local os arch

    case "$(uname -s)" in
        Linux*)  os="linux" ;;
        Darwin*) os="macos" ;;
        *)       error "Unsupported OS: $(uname -s). Use cargo install agent-code instead." ;;
    esac

    case "$(uname -m)" in
        x86_64|amd64)  arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)             error "Unsupported architecture: $(uname -m). Use cargo install agent-code instead." ;;
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

# Resolve a path to its canonical form, following symlinks where possible.
# Falls back to the input unchanged when no realpath tool is available.
canonicalize() {
    local p="$1"
    if command -v realpath >/dev/null 2>&1; then
        realpath "$p" 2>/dev/null || echo "$p"
    elif command -v readlink >/dev/null 2>&1; then
        readlink -f "$p" 2>/dev/null || echo "$p"
    else
        echo "$p"
    fi
}

# What would a fresh shell run for `agent`? Resolve the first match on PATH and
# canonicalize it, so it can be compared against the binary we just installed.
resolved_agent_path() {
    local found
    found="$(command -v "$BINARY" 2>/dev/null || true)"
    [ -n "$found" ] || return 1
    canonicalize "$found"
}

# Append the name guard to a single rc file, replacing any existing block.
# $1 = rc file path, $2 = flavor (posix|fish)
write_guard_to() {
    local rc="$1" flavor="$2" body tmp

    mkdir -p "$(dirname "$rc")"
    touch "$rc"

    if [ "$flavor" = "fish" ]; then
        # Prepend PATH inline rather than with `fish_add_path`, which persists to
        # the universal `fish_user_paths` outside this block and would survive
        # deletion of the block. Keeping it inline means removing the block fully
        # undoes the change.
        body="$(cat <<EOF
${GUARD_BEGIN}
# Ensures \`agent\` runs agent-code even if another command claims the name.
# Managed by the agent-code installer. Delete this block to opt out.
if not contains ${INSTALL_DIR} \$PATH
    set -gx PATH ${INSTALL_DIR} \$PATH
end
alias agent="${INSTALL_DIR}/agent"
${GUARD_END}
EOF
)"
    else
        body="$(cat <<EOF
${GUARD_BEGIN}
# Ensures \`agent\` runs agent-code even if another command claims the name.
# Managed by the agent-code installer. Delete this block to opt out.
case ":\$PATH:" in
    *":${INSTALL_DIR}:"*) ;;
    *) export PATH="${INSTALL_DIR}:\$PATH" ;;
esac
alias agent="${INSTALL_DIR}/agent"
${GUARD_END}
EOF
)"
    fi

    # Strip any previous block (idempotent re-runs), then append the fresh one.
    # Only a balanced begin/end pair is removed: buffer lines after a begin
    # marker and drop them only once the matching end is seen. If the end is
    # missing (a hand-edited rc), the buffered lines are flushed back verbatim
    # so no user content is ever lost.
    tmp="$(mktemp)"
    awk -v b="$GUARD_BEGIN" -v e="$GUARD_END" '
        !in_block && $0 == b { in_block=1; buf=$0; next }
        in_block {
            buf = buf ORS $0
            if ($0 == e) { in_block=0; buf="" }
            next
        }
        { print }
        END { if (in_block) print buf }
    ' "$rc" > "$tmp"

    # Drop trailing blank lines, then separate the block with one blank line.
    printf '%s\n\n%s\n' "$(cat "$tmp")" "$body" > "$rc"
    rm -f "$tmp"
}

# Write the name guard into the rc file(s) for the user's shell so `agent`
# resolves to agent-code regardless of PATH ordering or a competing symlink.
# Returns 0 if at least one rc file was written.
install_shell_guard() {
    local shell_name updated=1
    shell_name="$(basename "${SHELL:-}")"

    case "$shell_name" in
        zsh)
            write_guard_to "${ZDOTDIR:-$HOME}/.zshrc" posix && updated=0
            ;;
        fish)
            write_guard_to "$HOME/.config/fish/config.fish" fish && updated=0
            ;;
        bash)
            write_guard_to "$HOME/.bashrc" posix && updated=0
            # macOS bash login shells read .bash_profile, not .bashrc.
            [ "$(uname -s)" = "Darwin" ] && write_guard_to "$HOME/.bash_profile" posix
            ;;
        *)
            write_guard_to "$HOME/.profile" posix && updated=0
            ;;
    esac

    return $updated
}

main() {
    info "Installing agent-code..."

    local platform version url tmpdir installed_path resolved

    platform=$(detect_platform)
    info "Detected platform: ${platform}"

    version=$(get_latest_version)
    if [ -z "$version" ]; then
        error "Could not determine latest version. Check https://github.com/${REPO}/releases"
    fi
    info "Latest version: ${version}"

    url="https://github.com/${REPO}/releases/download/${version}/agent-${platform}.tar.gz"
    info "Downloading ${url}..."

    tmpdir=$(mktemp -d)
    trap 'rm -rf "${tmpdir:-/nonexistent}"' EXIT

    if ! curl -fsSL "$url" -o "${tmpdir}/agent.tar.gz"; then
        error "Download failed. Check that a release exists for your platform at:\n  https://github.com/${REPO}/releases"
    fi

    tar xzf "${tmpdir}/agent.tar.gz" -C "$tmpdir"

    if [ ! -f "${tmpdir}/${BINARY}" ]; then
        error "Binary not found in archive. The release may be packaged differently."
    fi

    # Ensure the install directory exists before moving into it. On some systems
    # (notably macOS, where Homebrew lives under /opt/homebrew) the default
    # /usr/local/bin does not exist, and `mv` will not create it — so create it
    # first, escalating to sudo only if an unprivileged mkdir fails.
    if [ ! -d "$INSTALL_DIR" ]; then
        if ! mkdir -p "$INSTALL_DIR" 2>/dev/null; then
            info "Creating ${INSTALL_DIR} (requires sudo)..."
            sudo mkdir -p "$INSTALL_DIR" || error "Could not create ${INSTALL_DIR}."
        fi
    fi

    # Install, using sudo for both the move and chmod when the directory is not
    # writable — a plain chmod after a `sudo mv` would fail with permission denied.
    if [ -w "$INSTALL_DIR" ]; then
        mv "${tmpdir}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
        chmod +x "${INSTALL_DIR}/${BINARY}"
    else
        info "Installing to ${INSTALL_DIR} (requires sudo)..."
        sudo mv "${tmpdir}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
        sudo chmod +x "${INSTALL_DIR}/${BINARY}"
    fi

    installed_path="$(canonicalize "${INSTALL_DIR}/${BINARY}")"

    success "agent-code ${version} installed to ${INSTALL_DIR}/${BINARY}"

    # Claim the `agent` name. Without this, another command of the same name in
    # an earlier PATH directory silently wins every new shell. The guard both
    # prepends our dir and defines an alias — an alias is resolved before PATH,
    # so it holds even if a competing symlink reappears later.
    if [ "${AGENT_CODE_NO_SHELL_SETUP:-0}" = "1" ]; then
        info "Skipping shell setup (AGENT_CODE_NO_SHELL_SETUP=1)."
    elif install_shell_guard; then
        info "Wrote the agent-code name guard to your shell config."
    fi

    # Verify what `agent` will actually resolve to — not merely that some
    # `agent` exists. A bare 'command -v' would report success even when a
    # different program owns the name.
    resolved="$(resolved_agent_path || true)"
    echo ""
    if [ "$resolved" = "$installed_path" ]; then
        echo -e "  ${BOLD}${BINARY} --version${RESET}"
        "${INSTALL_DIR}/${BINARY}" --version 2>/dev/null || true
    elif [ -n "$resolved" ]; then
        warn "\`${BINARY}\` currently resolves to a different command:"
        warn "    ${resolved}"
        warn "  agent-code was installed at ${installed_path} but is shadowed on your PATH."
        if [ "${AGENT_CODE_NO_SHELL_SETUP:-0}" = "1" ]; then
            warn "  Shell setup was skipped, so nothing was changed automatically."
        else
            warn "  The name guard was written to your shell config to take over the name."
        fi
        echo ""
        echo "  Open a new terminal (or run 'hash -r') and confirm with:"
        echo "    command -v ${BINARY}   # should print ${installed_path}"
    else
        echo "  Make sure ${INSTALL_DIR} is in your PATH:"
        echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
    fi

    echo ""
    echo "  Get started (in a new shell):"
    echo "    export AGENT_CODE_API_KEY=\"your-api-key\""
    echo "    ${BINARY}"
    echo ""
    echo "  Docs: https://avala-ai.github.io/agent-code/"
}

main "$@"
