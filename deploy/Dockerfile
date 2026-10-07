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
    set -e; cargo test --lib --locked; \
    target=$(cat /build-target); linker=$(cat /build-linker); \
    key=$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_'); \
    env "CARGO_TARGET_${key}_LINKER=$linker" "CC=$linker" cargo build --release --locked --target "$target" && cp "target/$target/release/pagecatch" /build/pagecatch

FROM --platform=$BUILDPLATFORM mcr.microsoft.com/dotnet/sdk:10.0@sha256:e70cdb7f80b0348f5cb85f19a8f670fca061f033d57eed12fa003d58b0e06317 AS engine
ARG TARGETARCH
ARG ENGINE_COMMIT=df70f0b3da0c630bd413bf617e758051f6b64757
ARG ENGINE_SOURCE_SHA256=be493c00760a3ee7848604e04547e48132522c23c9b50889e4afe0628038986b
RUN apt-get update && apt-get install -y --no-install-recommends curl patch ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /engine
RUN curl -fL --retry 3 "https://codeload.github.com/nilaoda/N_m3u8DL-RE/tar.gz/${ENGINE_COMMIT}" -o source.tar.gz && \
    printf '%s  source.tar.gz\n' "$ENGINE_SOURCE_SHA256" | sha256sum -c - && \
    tar -xzf source.tar.gz --strip-components=1 && rm source.tar.gz
COPY deploy/engine-segment-retry.patch ./
RUN patch -p1 < engine-segment-retry.patch
# Self-contained publishing supports both target architectures from the build host.
# The runtime stays bundled with the engine; no .NET installation is needed on the NAS.
RUN --mount=type=cache,target=/root/.nuget/packages \
    set -eux; case "$TARGETARCH" in amd64) rid=linux-x64;; arm64) rid=linux-arm64;; *) exit 1;; esac; \
    dotnet publish src/N_m3u8DL-RE/N_m3u8DL-RE.csproj -c Release -r "$rid" --self-contained true \
      -p:PublishAot=false -p:PublishTrimmed=false -p:PublishSingleFile=true \
      -p:IncludeNativeLibrariesForSelfExtract=true -p:DebugType=None -p:DebugSymbols=false -o /engine/out
RUN cp /usr/share/dotnet/LICENSE.txt /engine/out/DOTNET-LICENSE.txt && \
    cp /usr/share/dotnet/ThirdPartyNotices.txt /engine/out/DOTNET-ThirdPartyNotices.txt

FROM debian:bookworm-slim
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl ffmpeg chromium libicu72 libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=engine /engine/out/N_m3u8DL-RE /usr/local/bin/N_m3u8DL-RE
RUN chmod +x /usr/local/bin/N_m3u8DL-RE
COPY --from=build /build/pagecatch /usr/local/bin/pagecatch
ENV PC_HOST=0.0.0.0 PC_DATA=/data PC_OUTPUT=/downloads PC_BROWSER=/usr/bin/chromium PC_BROWSER_NO_SANDBOX=true DOTNET_BUNDLE_EXTRACT_BASE_DIR=/tmp/pagecatch-dotnet
COPY third-party /usr/share/doc/pagecatch/third-party
COPY --from=engine /engine/out/DOTNET-*.txt /usr/share/doc/pagecatch/third-party/
COPY deploy/engine-segment-retry.patch /usr/share/doc/pagecatch/third-party/engine-segment-retry.patch
COPY LICENSE /usr/share/doc/pagecatch/LICENSE
EXPOSE 8787
WORKDIR /app
HEALTHCHECK --interval=30s --timeout=5s CMD curl -fsS http://127.0.0.1:8787/healthz || exit 1
ENTRYPOINT ["pagecatch"]
