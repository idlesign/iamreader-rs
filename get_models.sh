#!/bin/bash

set -e

MODELS_DIR="models"
mkdir -p "$MODELS_DIR"

WHISPER_FILE="$MODELS_DIR/whisper.bin"
WHISPER_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
if [ ! -f "$WHISPER_FILE" ]; then
    wget -q -nc --show-progress -O "$WHISPER_FILE" "$WHISPER_URL"
fi

DENOISE_FILE="$MODELS_DIR/denoise.onnx"
DENOISE_URL="https://github.com/skeskinen/resemble-denoise-onnx-inference/raw/master/denoiser.onnx"
if [ ! -f "$DENOISE_FILE" ]; then
    wget -q -nc --show-progress -O "$DENOISE_FILE" "$DENOISE_URL"
fi
