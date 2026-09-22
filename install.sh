#!/usr/bin/env bash
# Quiczilla Unix (Linux / macOS) Installer
#
# Usage:
#   Remote: curl -fsSL https://raw.githubusercontent.com/cmdPromptCritical/quiczilla/master/install.sh | bash
#   Local:  ./install.sh

set -euo pipefail

APP_NAME="quiczilla"
REPO="${QUICZILLA_REPO:-cmdPromptCritical/quiczilla}"
VERSION="${QUICZILLA_VERSION:-latest}"

# Color codes
CYAN='\033[0;36m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

printf "\n"
printf "${CYAN}============================================================${NC}\n"
printf "${CYAN}   Quiczilla CLI - Fast Encrypted P2P File Transfer${NC}\n"
printf "${CYAN}============================================================${NC}\n\n"

# 1. Detect OS and Architecture
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$ARCH" in
    x86_64|amd64)
        NORM_ARCH="x86_64"
        ;;
    *)
        printf "${RED}Error: Unsupported architecture: %s${NC}\n" "$ARCH" >&2
        exit 1
        ;;
esac

case "$OS" in
    linux)
        PLATFORM="linux-${NORM_ARCH}"
        ;;
    *)
        printf "${RED}Error: Unsupported operating system: %s${NC}\n" "$OS" >&2
        exit 1
        ;;
esac

printf "${GREEN}[1/4] Detected Platform:${NC} %s (%s)\n" "$OS" "$NORM_ARCH"

# 2. Determine Install Directory
if [ -n "${QUICZILLA_INSTALL_DIR:-}" ]; then
    INSTALL_DIR="$QUICZILLA_INSTALL_DIR"
elif [ "$(id -u)" -eq 0 ]; then
    INSTALL_DIR="/usr/local/bin"
else
    INSTALL_DIR="$HOME/.local/bin"
fi

mkdir -p "$INSTALL_DIR"
TARGET_BIN="$INSTALL_DIR/$APP_NAME"
TARGET_WORKER="$INSTALL_DIR/quiczilla-worker"
TARGET_MSQUIC="$INSTALL_DIR/libmsquic.so"
printf "${GREEN}[2/4] Target Directory:${NC} %s\n" "$INSTALL_DIR"

# 3. Obtain Binary
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || echo "")"
LOCAL_BIN="$SCRIPT_DIR/target/release/quiczilla-cli"

if [ -n "$SCRIPT_DIR" ] && [ -f "$LOCAL_BIN" ]; then
    printf "${YELLOW}[3/4] Installing from local repository build...${NC}\n"
    cp -f "$LOCAL_BIN" "$TARGET_BIN"
    chmod +x "$TARGET_BIN"
else
    printf "${YELLOW}[3/4] Downloading Quiczilla CLI (%s)...${NC}\n" "$VERSION"
    ASSET_NAME="quiczilla-${PLATFORM}.tar.gz"
    if [ "$VERSION" = "latest" ]; then
        DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ASSET_NAME}"
    else
        DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET_NAME}"
    fi

    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT
    TMP_ARCHIVE="$TMP_DIR/$ASSET_NAME"

    DOWNLOAD_AUTH_ARGS=()
    if [ -n "${QUICZILLA_GITHUB_TOKEN:-}" ]; then
        DOWNLOAD_AUTH_ARGS=(--header="Authorization: Bearer ${QUICZILLA_GITHUB_TOKEN}")
    fi

    DOWNLOAD_SUCCESS=0
    if command -v curl >/dev/null 2>&1; then
        if curl -fsSL "${DOWNLOAD_AUTH_ARGS[@]}" "$DOWNLOAD_URL" -o "$TMP_ARCHIVE" 2>/dev/null; then
            DOWNLOAD_SUCCESS=1
        fi
    elif command -v wget >/dev/null 2>&1; then
        if wget "${DOWNLOAD_AUTH_ARGS[@]}" -qO "$TMP_ARCHIVE" "$DOWNLOAD_URL" 2>/dev/null; then
            DOWNLOAD_SUCCESS=1
        fi
    fi

    if [ "$DOWNLOAD_SUCCESS" -eq 1 ]; then
        tar -xzf "$TMP_ARCHIVE" -C "$TMP_DIR"
        mv -f "$TMP_DIR/quiczilla" "$TARGET_BIN"
        mv -f "$TMP_DIR/quiczilla-worker" "$TARGET_WORKER"
        mv -f "$TMP_DIR/libmsquic.so" "$TARGET_MSQUIC"
        chmod +x "$TARGET_BIN"
        chmod +x "$TARGET_WORKER"
    else
        # If release asset not found on GitHub, check if cargo is available (including ~/.cargo/bin)
        export PATH="$HOME/.cargo/bin:$PATH"
        if command -v cargo >/dev/null 2>&1 && [ -f "$SCRIPT_DIR/Cargo.toml" ]; then
            printf "${YELLOW}Release artifact not found online; building locally via cargo...${NC}\n"
            (cd "$SCRIPT_DIR" && cargo build -p quiczilla-cli --release)
            cp -f "$SCRIPT_DIR/target/release/quiczilla-cli" "$TARGET_BIN"
            chmod +x "$TARGET_BIN"
        else
            printf "${RED}Error: Failed to download %s from %s${NC}\n" "$ASSET_NAME" "$DOWNLOAD_URL" >&2
            printf "${YELLOW}Hint: To install from a local checkout, run ./install.sh from the cloned repository root.${NC}\n" >&2
            exit 1
        fi
    fi
fi

# 4. Create 'quic' shorthand alias
ALIAS_BIN="$INSTALL_DIR/quic"
printf "${CYAN}  -> Creating 'quic' shorthand alias at %s...${NC}\n" "$ALIAS_BIN"
ln -sf "$TARGET_BIN" "$ALIAS_BIN"

# 5. Verify PATH
printf "${GREEN}[4/4] Verifying PATH configuration...${NC}\n"
PATH_CONFIGURED=0
case ":$PATH:" in
    *":$INSTALL_DIR:"*)
        PATH_CONFIGURED=1
        printf "  -> %s is already in your PATH.\n" "$INSTALL_DIR"
        ;;
    *)
        printf "${YELLOW}  -> Note: %s is not currently in your PATH.${NC}\n" "$INSTALL_DIR"
        SHELL_NAME="$(basename "${SHELL:-bash}")"
        case "$SHELL_NAME" in
            zsh)
                RC_FILE="$HOME/.zshrc"
                ;;
            bash)
                RC_FILE="$HOME/.bashrc"
                ;;
            *)
                RC_FILE="$HOME/.profile"
                ;;
        esac
        printf "  -> To add it automatically, run:\n"
        printf "     echo 'export PATH=\"%s:\$PATH\"' >> %s && source %s\n" "$INSTALL_DIR" "$RC_FILE" "$RC_FILE"
        ;;
esac

printf "\n"
printf "${GREEN}============================================================${NC}\n"
printf "${GREEN} Quiczilla CLI installed successfully!${NC}\n"
printf "${GREEN} Installed Location: %s${NC}\n" "$TARGET_BIN"
printf "${GREEN} Command Aliases:    quiczilla, quic${NC}\n"
printf "\n"
printf " Example usage:\n"
printf "   ${YELLOW}quic sample.bin user@remote:/destination/folder/${NC}\n"
printf "   ${YELLOW}quic sample.bin user@remote:/destination/folder/ --checksum${NC}\n"
printf "   ${YELLOW}quic sample.bin user@remote:/destination/folder/ --no-progress${NC}\n"
printf "   ${YELLOW}quic sample.bin user@remote:/destination/folder/ -p 22322${NC}\n"
printf "   ${YELLOW}quic pipe user@remote \"zfs receive backup/dataset\"${NC}\n"
printf "   ${YELLOW}quic ... --verbose  # show SSH/bootstrap diagnostics${NC}\n"
printf "${GREEN}============================================================${NC}\n\n"
