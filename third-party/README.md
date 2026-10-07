# 第三方组件

Rust 依赖许可见 RUST-NOTICES.md 与 rust/。下载引擎 v0.6.0-beta 的 MIT 许可见 N_m3u8DL-RE-LICENSE.txt，来源 https://github.com/nilaoda/N_m3u8DL-RE/tree/v0.6.0-beta 。

Docker 中的下载引擎从该版本的固定提交 df70f0b3da0c630bd413bf617e758051f6b64757 构建，应用 deploy/engine-segment-retry.patch：检测假冒分片的 HTML 响应，并对残缺响应与 AES 解密异常单独重试分片。补丁副本保留在镜像此目录中。引擎使用自包含 .NET 发布，运行时随引擎打包；源码和依赖版本仍以该固定提交为准。

Docker 的 FFmpeg、Chromium 及系统库来自 Debian bookworm 软件仓库，许可和来源说明保留在镜像 /usr/share/doc 各包的 copyright 文件中。相应源码可通过 Debian source 软件包获取。
