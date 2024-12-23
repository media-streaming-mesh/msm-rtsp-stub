FROM --platform=linux/amd64 lukemathwalker/cargo-chef:latest-rust-latest AS amd64-chef
FROM --platform=linux/arm64 lukemathwalker/cargo-chef:latest-rust-latest AS arm64-chef

# Base image for the build stage - this is a multi-stage build that uses cross-compilation (thanks to --platform switch)
FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:latest-rust-latest AS chef
WORKDIR /msm-rtsp-stub

# Planner stage
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Builder stage
FROM chef AS builder 
COPY --from=planner /msm-rtsp-stub/recipe.json recipe.json

ARG BUILDARCH
ARG TARGETPLATFORM
ARG TARGETARCH

# Copy runtime dependencies for specific target platform/architecture
# ARM specific folders
WORKDIR /all-files/linux/arm64/lib/aarch64-linux-gnu

# AMD64 specific folders
WORKDIR /all-files/linux/amd64/lib/x86_64-linux-gnu
WORKDIR /all-files/linux/amd64/lib64

# Common folders
WORKDIR /all-files/${TARGETPLATFORM}/etc/ssl/certs
WORKDIR /all-files/${TARGETPLATFORM}/msm-rtsp-stub

# ARM64
COPY --from=arm64-chef \
        /lib/aarch64-linux-gnu/libssl.so.3 \
        /lib/aarch64-linux-gnu/libcrypto.so.3 \
        /lib/aarch64-linux-gnu/libgcc_s.so.1 \
        /lib/aarch64-linux-gnu/libm.so.6 \
        /lib/aarch64-linux-gnu/libc.so.6 \
        /all-files/linux/arm64/lib/aarch64-linux-gnu/

COPY --from=arm64-chef \
       /lib/ld-linux-aarch64.so.1 \
       /all-files/linux/arm64/lib

# AMD64
COPY --from=amd64-chef \
        /lib/x86_64-linux-gnu/libssl.so.3 \
        /lib/x86_64-linux-gnu/libcrypto.so.3 \
        /lib/x86_64-linux-gnu/libgcc_s.so.1 \
        /lib/x86_64-linux-gnu/libm.so.6 \
        /lib/x86_64-linux-gnu/libc.so.6 \
        /all-files/linux/amd64/lib/x86_64-linux-gnu/

COPY --from=amd64-chef \
        /lib64/ld-linux-x86-64.so.2 \
        /all-files/linux/amd64/lib64/

# Common files - certs
COPY --from=amd64-chef \
        /etc/ssl/certs/ca-certificates.crt \
        /all-files/linux/amd64/etc/ssl/certs/
COPY --from=arm64-chef \
        /etc/ssl/certs/ca-certificates.crt \
        /all-files/linux/arm64/etc/ssl/certs/

WORKDIR /msm-rtsp-stub

# install base dependencies - incuding protobuf compiler
RUN update-ca-certificates
RUN apt clean
RUN apt update
RUN apt-get -y install protobuf-compiler

# Install dependencies for cross-compilation
RUN if [ "$BUILDARCH" != "$TARGETARCH" ]; then \
        if [ "$TARGETARCH" = "arm64" ]; then \
            dpkg --add-architecture arm64 \
            && apt-get update \ 
            && apt-get install -y \
            g++-aarch64-linux-gnu \
            libc6-dev-arm64-cross \
            libssl-dev:arm64 \
            && rustup target add aarch64-unknown-linux-gnu \
            && rustup toolchain install stable-aarch64-unknown-linux-gnu \
            && rm -rf /var/lib/apt/lists/*; \
        elif [ "$TARGETARCH" = "amd64" ]; then \
            dpkg --add-architecture amd64 \
            && apt-get update \ 
            && apt-get install -y \
            g++-x86-64-linux-gnu \
            libc6-dev-amd64-cross \
            libssl-dev:amd64 \
            && rustup target add x86_64-unknown-linux-gnu \
            && rustup toolchain install stable-x86_64-unknown-linux-gnu \
            && rm -rf /var/lib/apt/lists/*; \
        fi \
    fi

# Build dependencies - this is the caching Docker layer!
RUN case ${TARGETARCH} in \
        arm64) PKG_CONFIG_SYSROOT_DIR=/ CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc cargo chef cook --target=aarch64-unknown-linux-gnu --release --recipe-path recipe.json ;; \
        amd64) PKG_CONFIG_SYSROOT_DIR=/ CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc cargo chef cook --target=x86_64-unknown-linux-gnu --release --recipe-path recipe.json ;; \
        *) exit 1 ;; \
    esac 

# Copy the source code
COPY . /msm-rtsp-stub

RUN cargo update

# Build application - this is the caching Docker layer!
RUN case ${TARGETARCH} in \
        arm64) PKG_CONFIG_SYSROOT_DIR=/ CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc cargo build --target=aarch64-unknown-linux-gnu --release ;; \
        amd64) PKG_CONFIG_SYSROOT_DIR=/ CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc cargo build --target=x86_64-unknown-linux-gnu --release ;; \
        *) exit 1 ;; \
    esac

# Copy all the dependencies to a separate folder
RUN set -ex; \
    # Determine target (source folder for the binary and env files)
    if [ "$TARGETARCH" = "arm64" ]; then target='target/aarch64-unknown-linux-gnu/release'; fi; \
    if [ "$TARGETARCH" = "amd64" ]; then target='target/x86_64-unknown-linux-gnu/release'; fi; \
    # Copy files from the target folder to app folder
    cp $target/msm_rtsp_stub     /all-files/${TARGETPLATFORM}/msm-rtsp-stub

# # Create a single layer image
FROM scratch AS runtime

# Make build arguments available in the runtime stage
ARG TARGETPLATFORM
ARG TARGETARCH

WORKDIR /app

# Copy the binary as a single layer
COPY --from=builder /all-files/${TARGETPLATFORM}/msm-rtsp-stub /

# Expose the port that the application listens on.
EXPOSE 8554

# Run the application
ENTRYPOINT ["/msm_rtsp_stub"]
