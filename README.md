# PageCatch

PageCatch 是一个带网页界面的视频下载服务，使用 Rust 编写，通过 Docker 部署。

输入视频播放页链接后，程序尝试提取 HLS / DASH 播放清单；静态解析没有找到有效清单时，会启动 Chromium 捕获媒体请求。下载由 N_m3u8DL-RE 执行，FFmpeg 和 ffprobe 负责合并后的文件检查。

本仓库提供源码和 Docker 构建配置。当前 GitHub Actions 只构建镜像，不发布镜像或可下载安装包。

## 已实现的功能

- 提交播放页或 HLS / DASH 清单链接，每次最多提交 200 条。清单地址不必以 `.m3u8` 或 `.mpd` 结尾，程序会检查响应内容。
- 网页支持导入 TXT / CSV，以及直接粘贴包含昵称、时间的视频链接聊天记录；自动提取完整 HTTP/HTTPS 链接并去重，确认后加入队列。
- 下载队列、并发控制、失败重试、取消任务和查看日志。
- Docker 内的下载引擎会识别冒充视频分片的 HTML 响应，并对残缺响应或 AES 解密失败单独重试分片，保留已成功下载的分片。分片重试仍失败时，任务报错，不保存缺片成品。
- 下载后检查视频轨道、时长，并执行 FFmpeg 解码检查；检查通过后保存 MP4。
- 按 UTC+8 日期保存到 `YYYY-MM-DD` 子目录，目录不存在时创建。日期在校验完成、开始保存时计算。
- SQLite 保存任务和下载配置；服务重启后，未结束任务重新进入队列。
- 相同链接检查，以及基于时长和开头 6 秒帧指纹的已有视频检查。
- 删除已结束任务的记录，保留视频文件和日志。

## 批量导入链接

选择 **New Task → Import File**，从电脑或手机选择 `.txt` / `.csv` 文件，也可以把聊天记录直接粘贴到输入框。例如，TXT 可以保留原来的昵称和时间：

```text
昵称: 10-07 00:43:26
https://example.com/#/videoDetail/123

昵称: 10-07 00:44:08
https://example.com/watch?id=456
```

导入后，输入框显示提取出的链接，可以编辑。文件里的链接会追加到当前输入，完全相同的链接只保留一次；选择 **Add to Queue** 才创建任务。`#/videoDetail/...` 和查询参数会保留，`https:xxxxx` 等不完整地址不会作为链接导入。

CSV 不要求固定列名，程序会从每个单元格提取链接；按标准 CSV 格式保存，包含逗号或换行的单元格用双引号包住，例如：

```csv
name,time,url
昵称,00:43:26,"https://example.com/#/videoDetail/123"
昵称,00:44:08,"https://example.com/watch?ids=1,2&token=abc"
```

文件最大 1 MiB，支持 UTF-8（含 BOM）、带 BOM 的 UTF-16 和 GB18030。每次最多 200 条唯一链接，超过时会提示，保留全部提取结果供编辑，不会自动截断或分批提交。文件读取和链接提取在浏览器完成，服务器只接收确认提交的链接及请求头。

## 从源码部署

需要 Git、支持 BuildKit 的 Docker，以及 Docker Compose。构建时需要联网获取基础镜像、系统软件包和下载引擎。

```bash
git clone https://github.com/jmjFortune/m3u8-download.git
cd m3u8-download
cp .env.example .env
```

编辑 `.env`，将 `PC_TOKEN` 改为自己的访问令牌，然后启动：

```bash
docker compose up -d --build
```

在浏览器打开 `http://你的NAS地址:8787`。进入左下角 **Settings → Network**，输入与 `.env` 中 `PC_TOKEN` 相同的值并选择 **Save Changes**。这里填写的是服务器已配置的令牌；修改服务器令牌需要编辑 `.env` 并重建容器。

Docker 镜像内安装了 N_m3u8DL-RE、FFmpeg、ffprobe 和 Chromium。Dockerfile 包含 `linux/amd64` 和 `linux/arm64` 的构建配置，普通 Compose 构建使用当前主机的架构。

查看运行状态和日志：

```bash
docker compose ps
docker compose logs -f
```

更新源码后重新构建：

```bash
git pull
docker compose up -d --build
```

停止服务：

```bash
docker compose down
```

## 数据和保存目录

默认挂载关系如下，主机路径相对于项目目录：

| 主机目录 | 容器目录 | 内容 |
|---|---|---|
| `./data` | `/data` | `tasks.sqlite`、任务日志、下载临时文件 |
| `./downloads` | `/downloads` | 校验通过的视频及日期子目录 |

成品可以通过 NAS 文件管理器访问。停止或重建容器会保留上述主机目录。

要指定 NAS 上已有的绝对目录，在 `.env` 中设置 `PC_DATA_DIR` 和 `PC_DOWNLOAD_DIR`。如果希望网页显示的保存位置与 NAS 实际路径相同，将 `PC_DOWNLOAD_DIR` 和 `PC_OUTPUT` 设为同一绝对路径。例如，下面的占位路径需替换为自己的目录：

```dotenv
PC_DATA_DIR=/path/to/pagecatch/data
PC_DOWNLOAD_DIR=/path/to/videos
PC_OUTPUT=/path/to/videos
```

创建相应目录并保证容器可写，然后重建容器。网页 **Settings → Downloads** 中保存过的设置优先于环境变量；已有部署还需在网页中同步保存位置。修改保存目录不会搬移已有视频。

## 网页使用

1. 选择 **New Task**，粘贴链接，点击 **Add to Queue**。
2. 在任务列表查看状态；失败或取消的任务可以重试，活动任务可以取消。
3. 在 **Settings → Downloads** 修改保存位置、并发数、分片线程数和尝试上限。新设置应用于之后开始的任务，运行中的任务保留原参数。
4. 已结束任务可以选择垃圾桶 **Delete record**。正在排队、运行或取消清理中的任务需要先停止，再删除记录。

需要 Cookie / Referer 的页面，可在 **Settings → Network** 的 **Request headers** 中填写 JSON：

```json
{"Cookie":"填写自己的 Cookie","Referer":"https://example.com/"}
```

请求头用于新提交的任务，并随任务存入 SQLite；网页中的默认请求头不会在刷新后恢复。访问令牌验证成功后保存在当前浏览器的 localStorage，浏览器允许站点存储时可以在刷新后恢复；更换浏览器、地址或端口需要重新填写。

## Compose 配置

以下变量由仓库内的 Compose 文件使用：

| 变量 | 默认值 | 用途 |
|---|---|---|
| `PC_TOKEN` | 必须填写 | API 访问令牌 |
| `PC_PORT` | `8787` | 主机网页端口 |
| `PC_DATA_DIR` | `./data` | 主机数据目录 |
| `PC_DOWNLOAD_DIR` | `./downloads` | 主机视频目录 |
| `PC_OUTPUT` | `/downloads` | 容器内保存根目录，必须对应已挂载的位置 |
| `PC_WORKERS` | `2` | 初始并发任务数，范围 1–8 |
| `PC_THREADS` | `4` | 初始分片线程数，范围 1–32 |

尝试上限默认是 3 次，包含首次，可在网页中改为 1–10 次。网页保存的下载配置优先于启动配置。

## 在其他机器构建，再导入 NAS

`compose.nas.yaml` 仅启动已有镜像，不执行构建，也不从镜像仓库拉取。使用它之前，需要自行构建或导入镜像，并用 `PC_IMAGE` 指定镜像名。

例如，在支持 Docker Buildx 的机器上为 x86_64 NAS 构建并导出：

```bash
docker buildx build --platform linux/amd64 -f deploy/Dockerfile -t pagecatch:nas --load .
docker save pagecatch:nas | gzip > pagecatch-nas.tar.gz
```

ARM64 NAS 将构建平台改为 `linux/arm64`。把镜像归档、`compose.nas.yaml` 和配置好的 `.env` 传到 NAS；在 `.env` 中设置 `PC_IMAGE=pagecatch:nas`，然后执行：

```bash
docker load -i pagecatch-nas.tar.gz
docker compose -f compose.nas.yaml up -d
```

此部署方式的状态和日志命令也需加上 `-f compose.nas.yaml`。

## 当前范围

页面解析受网站结构、请求头、登录状态和网络限制影响，不能保证任意网站都能解析。Chromium 使用独立临时配置，不使用访问网页的浏览器登录会话。

当前不提供交互式登录、验证码处理、DRM 解密或直播录制。已有视频检查只比较开头 6 秒的帧指纹，不是完整视频内容比较。

## 源码结构

```text
src/                Rust 后端：解析、任务队列、下载、校验、SQLite、API
web/                HTML / CSS / JavaScript，编译时内嵌到后端
Cargo.toml          Rust 依赖配置
Cargo.lock          锁定依赖版本
deploy/Dockerfile   Docker 多阶段构建
compose.yaml        从源码构建并运行
compose.nas.yaml    使用已有镜像运行
.env.example        Compose 配置模板
third-party/        第三方组件许可证和来源说明
.github/workflows/  Docker 构建工作流
```

源码内的单元测试可以通过 `cargo test --lib --locked` 运行，需要本机安装 Rust。网页不需要单独执行 Node.js 或前端打包命令。

下载引擎的回归测试使用本地生成的 AES HLS 视频，覆盖正常下载、假分片、残缺密文、解密失败和持续错误；需要 Docker、Python 3 和 OpenSSL：

```bash
python3 deploy/verify-engine.py 镜像名
```

Docker 构建会从固定提交编译带分片重试补丁的下载引擎，构建期间需要访问 GitHub、NuGet 和 Debian 软件源。运行时依赖已包含在镜像中。

## 许可

项目采用 [MIT License](LICENSE)。第三方组件的许可和来源见 [third-party](third-party/README.md)。
