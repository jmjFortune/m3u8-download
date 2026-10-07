FROM --platform=$BUILDPLATFORM rust:1.96-bookworm AS build
ARG TARGETARCH
RUN set -eux; case "$TARGETARCH" in amd64) target=x86_64-unknown-linux-gnu; linker=x86_64-linux-gnu-gcc; package=gcc-x86-64-linux-gnu; libc=libc6-dev-amd64-cross;; arm64) target=aarch64-unknown-linux-gnu; linker=aarch64-linux-gnu-gcc; package=gcc-aarch64-linux-gnu; libc=libc6-dev-arm64-cross;; *) exit 1;; esac; \
    rustup target add "$target"; \
    if ! command -v "$linker" >/dev/null; then apt-get update && apt-get install -y --no-install-recommends "$package" "$libc" && rm -rf /var/lib/apt/lists/*; fi; \
    printf '%s' "$target" > /build-target; printf '%s' "$linker" > /build-linker
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/build/target \
    target=$(cat /build-target); linker=$(cat /build-linker); \
    key=$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_'); \
    env "CARGO_TARGET_${key}_LINKER=$linker" "CC=$linker" cargo build --release --locked --target "$target" && cp "target/$target/release/pagecatch" /build/pagecatch

FROM debian:bookworm-slim
ARG TARGETARCH
ARG ENGINE_VERSION=v0.6.0-beta
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl ffmpeg chromium libicu72 libssl3 && rm -rf /var/lib/apt/lists/*
RUN set -eux; case "$TARGETARCH" in amd64) arch=x64;; arm64) arch=arm64;; *) exit 1;; esac; \
    curl -fL --retry 3 "https://github.com/nilaoda/N_m3u8DL-RE/releases/download/${ENGINE_VERSION}/N_m3u8DL-RE_${ENGINE_VERSION}_linux-${arch}_20260629.tar.gz" -o /tmp/engine.tar.gz; \
    mkdir /tmp/engine; tar -xzf /tmp/engine.tar.gz -C /tmp/engine; \
    find /tmp/engine -type f -name N_m3u8DL-RE -exec cp '{}' /usr/local/bin/N_m3u8DL-RE \;; \
    chmod +x /usr/local/bin/N_m3u8DL-RE; rm -rf /tmp/engine /tmp/engine.tar.gz
COPY --from=build /build/pagecatch /usr/local/bin/pagecatch
ENV PC_HOST=0.0.0.0 PC_DATA=/data PC_OUTPUT=/downloads PC_BROWSER=/usr/bin/chromium PC_BROWSER_NO_SANDBOX=true
COPY third-party /usr/share/doc/pagecatch/third-party
COPY LICENSE /usr/share/doc/pagecatch/LICENSE
EXPOSE 8787
WORKDIR /app
HEALTHCHECK --interval=30s --timeout=5s CMD curl -fsS http://127.0.0.1:8787/healthz || exit 1
ENTRYPOINT ["pagecatch"]
