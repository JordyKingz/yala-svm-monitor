#!/bin/bash

# Docker run helper script

set -e

# Default values
IMAGE_NAME="solana-monitor:latest"
CONTAINER_NAME="solana-monitor"
ENV_FILE=".env"
MODE="monitor"  # monitor, test, generate-config

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --image)
            IMAGE_NAME="$2"
            shift 2
            ;;
        --name)
            CONTAINER_NAME="$2"
            shift 2
            ;;
        --env-file)
            ENV_FILE="$2"
            shift 2
            ;;
        --mode)
            MODE="$2"
            shift 2
            ;;
        --slots)
            SLOTS="$2"
            shift 2
            ;;
        --detach|-d)
            DETACH="-d"
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo "Options:"
            echo "  --image IMAGE      Docker image to use (default: solana-monitor:latest)"
            echo "  --name NAME        Container name (default: solana-monitor)"
            echo "  --env-file FILE    Environment file (default: .env)"
            echo "  --mode MODE        Run mode: monitor, test, generate-config"
            echo "  --slots SLOTS      Specific slots to monitor (comma-separated)"
            echo "  --detach, -d       Run in background"
            echo "  --help, -h         Show this help"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Check if env file exists
if [ ! -f "$ENV_FILE" ]; then
    echo "Error: Environment file '$ENV_FILE' not found"
    echo "Create one from .env.example:"
    echo "  cp .env.example $ENV_FILE"
    exit 1
fi

# Create necessary directories
mkdir -p config tx_data logs

# Build command based on mode
case $MODE in
    monitor)
        if [ -n "$SLOTS" ]; then
            CMD="monitor \"$SLOTS\""
        else
            CMD=""
        fi
        ;;
    test)
        CMD="telegram-setup"
        ;;
    generate-config)
        CMD="generate-config filters.json"
        ;;
    *)
        echo "Error: Unknown mode '$MODE'"
        exit 1
        ;;
esac

# Run the container
echo "Starting $CONTAINER_NAME in $MODE mode..."

docker run \
    ${DETACH:--it} \
    --rm \
    --name "$CONTAINER_NAME" \
    --env-file "$ENV_FILE" \
    -v "$(pwd)/config:/app/config" \
    -v "$(pwd)/slot_checkpoint.json:/app/slot_checkpoint.json" \
    -v "$(pwd)/tx_data:/app/tx_data" \
    -v "$(pwd)/logs:/app/logs" \
    "$IMAGE_NAME" \
    $CMD

if [ -n "$DETACH" ]; then
    echo "Container started in background. View logs with:"
    echo "  docker logs -f $CONTAINER_NAME"
    echo "Stop with:"
    echo "  docker stop $CONTAINER_NAME"
fi