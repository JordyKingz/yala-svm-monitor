#!/bin/bash

# Docker build script with versioning and multi-platform support

set -e

# Configuration
IMAGE_NAME="solana-monitor"
PLATFORMS="linux/amd64,linux/arm64"
REGISTRY="${DOCKER_REGISTRY:-}"

# Get version from Cargo.toml or git
VERSION=$(grep '^version' Cargo.toml | sed 's/.*= "\(.*\)"/\1/' || echo "latest")
GIT_COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
BUILD_DATE=$(date -u +'%Y-%m-%dT%H:%M:%SZ')

echo "Building ${IMAGE_NAME}:${VERSION} (commit: ${GIT_COMMIT})"

# Build arguments
BUILD_ARGS="--build-arg VERSION=${VERSION} --build-arg GIT_COMMIT=${GIT_COMMIT} --build-arg BUILD_DATE=${BUILD_DATE}"

# Function to build image
build_image() {
    local tag=$1
    local platform=$2
    
    if [ -n "$platform" ]; then
        echo "Building for platforms: $platform"
        docker buildx build \
            --platform "$platform" \
            ${BUILD_ARGS} \
            -t "${tag}" \
            --push \
            .
    else
        echo "Building for current platform"
        docker build \
            ${BUILD_ARGS} \
            -t "${tag}" \
            .
    fi
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --push)
            PUSH=true
            shift
            ;;
        --multi-platform)
            MULTI_PLATFORM=true
            shift
            ;;
        --registry)
            REGISTRY="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Ensure buildx is available for multi-platform builds
if [ "$MULTI_PLATFORM" = true ]; then
    docker buildx create --name solana-monitor-builder --use 2>/dev/null || true
fi

# Build tags
TAGS=(
    "${IMAGE_NAME}:${VERSION}"
    "${IMAGE_NAME}:${GIT_COMMIT}"
    "${IMAGE_NAME}:latest"
)

# Add registry prefix if specified
if [ -n "$REGISTRY" ]; then
    PREFIXED_TAGS=()
    for tag in "${TAGS[@]}"; do
        PREFIXED_TAGS+=("${REGISTRY}/${tag}")
    done
    TAGS=("${PREFIXED_TAGS[@]}")
fi

# Build the image
if [ "$MULTI_PLATFORM" = true ]; then
    # Multi-platform build
    TAG_ARGS=""
    for tag in "${TAGS[@]}"; do
        TAG_ARGS="${TAG_ARGS} -t ${tag}"
    done
    
    docker buildx build \
        --platform "${PLATFORMS}" \
        ${BUILD_ARGS} \
        ${TAG_ARGS} \
        ${PUSH:+--push} \
        .
else
    # Single platform build
    for tag in "${TAGS[@]}"; do
        build_image "$tag"
    done
    
    # Push if requested
    if [ "$PUSH" = true ] && [ -n "$REGISTRY" ]; then
        for tag in "${TAGS[@]}"; do
            docker push "$tag"
        done
    fi
fi

echo "Build complete!"
echo "Images built:"
for tag in "${TAGS[@]}"; do
    echo "  - $tag"
done

# Cleanup
if [ "$MULTI_PLATFORM" = true ]; then
    docker buildx rm solana-monitor-builder 2>/dev/null || true
fi