#!/bin/bash

set -e

if [ "$EUID" -ne 0 ]; then
    SUDO="sudo"
else
    SUDO=""
fi

if ! command -v rustc &> /dev/null; then
    if ! command -v curl &> /dev/null; then
        $SUDO apt-get install -y curl
    fi
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env" 2>/dev/null || true
fi

$SUDO apt-get update -qq
$SUDO apt-get install -y \
    build-essential \
    pkg-config \
    cmake \
    clang \
    libssl-dev \
    libclang-dev \
    libasound2-dev \
    libpulse-dev \
    libfontconfig1-dev \
    libfreetype6-dev \
    libxcb1-dev \
    libxcb-render0-dev \
    libxcb-shape0-dev \
    libxcb-xfixes0-dev \
    wget

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
./get_models.sh
