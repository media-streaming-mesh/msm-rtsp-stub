# syntax=docker/dockerfile:experimental
####################################################################################################
## Builder
####################################################################################################
FROM --platform=$BUILDPLATFORM rust:latest AS builder

RUN update-ca-certificates

RUN apt clean
RUN apt update

RUN apt-get -y install protobuf-compiler build-essential

RUN rustup update

ARG TARGETARCH

RUN if [ "$TARGETARCH" = "arm64" ]; then rustup target add aarch64-unknown-linux-gnu && rustup toolchain add stable-aarch64-unknown-linux-gnu; fi;
RUN if [ "$TARGETARCH" = "amd64" ]; then rustup target add x86_64-unknown-linux-gnu && rustup toolchain add stable-x86_64-unknown-linux-gnu; fi;

RUN if [ "$BUILDARCH" != "$TARGETARCH" ]; then \
      if [ "$TARGETARCH" = "arm64" ]; then apt-get -y install crossbuild-essential-arm64; fi; \
      if [ "$TARGETARCH" = "amd64" ]; then apt-get -y install crossbuild-essential-amd64; fi; \
    fi

WORKDIR /msm-rtsp-stub

COPY ./ .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/msm-rtsp-stub/target \
    touch .env; \
    if [ "$TARGETARCH" = "arm64" ]; then export TARGET=aarch64-unknown-linux-gnu; else export TARGET=x86_64-unknown-linux-gnu; fi; \
    echo TARGET=$TARGET > .env; \
    cargo build --target $TARGET && cp target/$TARGET/debug/msm_rtsp_stub .

####################################################################################################
## Final image
####################################################################################################
FROM ubuntu

WORKDIR /

# Copy our build
COPY --from=builder /msm-rtsp-stub/msm_rtsp_stub .

ENTRYPOINT ["/msm_rtsp_stub"]
