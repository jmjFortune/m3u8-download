# PageCatch

输入普通视频播放页链接，自动解析 HLS / DASH，下载并校验 MP4。Rust 后端与英文网页一起打包，使用 Docker 部署到飞牛 NAS。

## NAS 部署

当前 NAS 项目位于 `/vol1/1000/docker/pagecatch`，访问地址为 `http://192.168.193.2:8787`。

`dist/` 保留两种 NAS 镜像：x86_64 使用 `pagecatch-docker-amd64.tar.gz`，ARM64 使用 `pagecatch-docker-arm64.tar.gz`。将对应镜像包、`compose.nas.yaml` 和 `.env.example` 放入 NAS 项目目录。

首次部署：

```bash
docker load -i pagecatch-docker-amd64.tar.gz
cp .env.example .env
# 编辑 .env：设置 PC_TOKEN、数据目录和视频目录
# ARM64 还需设置 PC_IMAGE=pagecatch:0.1.0-arm64
docker compose -f compose.nas.yaml up -d
```

已有部署升级时保留原 `.env` 和数据目录，导入新镜像后再重建容器：

```bash
docker load -i pagecatch-docker-amd64.tar.gz
docker compose -f compose.nas.yaml up -d --force-recreate
```

`.env` 的目录配置示例：

```dotenv
PC_IMAGE=pagecatch:0.1.0-amd64
PC_TOKEN=自行填写访问令牌
PC_PORT=8787
PC_WORKERS=2
PC_THREADS=4
PC_DATA_DIR=/vol1/1000/docker/pagecatch/data
PC_DOWNLOAD_DIR=/vol4/1000/ZiLiao/资料/Resources/xxzl/BUFFER/Videos
PC_OUTPUT=/vol4/1000/ZiLiao/资料/Resources/xxzl/BUFFER/Videos
```

提前创建这些目录。`PC_DOWNLOAD_DIR` 和 `PC_OUTPUT` 使用同一绝对路径，网页显示与 NAS 实际位置一致。已经在网页保存过配置时，网页设置优先于环境变量；需要在 **Settings → Downloads** 同步目录。成品在校验完成后保存到北京时间当天的 `YYYY-MM-DD` 子目录，不存在时自动创建。

```bash
docker compose -f compose.nas.yaml ps
docker compose -f compose.nas.yaml logs -f
docker compose -f compose.nas.yaml down
```

停止或重建容器不会清空挂载的数据目录和视频目录。镜像包含下载引擎、FFmpeg、ffprobe 与 Chromium，无需在 NAS 单独安装依赖。

## 使用

打开 NAS 网页，在左下角 **Settings → Network** 保存 `.env` 中的访问令牌。成功验证后令牌保存在当前浏览器，刷新仍保持连接；换浏览器、地址或端口后需要重新输入。Cookie / Referer 只保留在当前页面。

**New Task** 中粘贴网页或媒体链接，一行一个，然后选择 **Add to Queue**。页面会自动解析媒体地址；需要登录的页面可填写请求头。验证码、DRM、特殊播放操作及直播录制不在自动处理范围内。

任务支持取消、重试和查看日志。已结束任务的垃圾桶 **Delete record** 只删除记录，保留视频和调试日志。活动任务先取消，等待清理完成后才能删除。

下载配置在 **Settings → Downloads** 保存到 SQLite，重启后恢复。运行中的任务继续使用原参数，之后启动的任务使用新配置。

## 从源码构建 Docker 镜像

在项目根目录执行：

```bash
docker build -f deploy/Dockerfile -t pagecatch:0.1.0-amd64 .
```

此命令构建当前机器架构的镜像，适用于 x86_64 NAS。跨架构构建使用 Docker Buildx：

```bash
docker buildx build --platform linux/amd64 -f deploy/Dockerfile -t pagecatch:0.1.0-amd64 --load .
# ARM64 将 amd64 改为 arm64
```

使用 `compose.yaml` 可以直接从源码构建并启动：

```bash
docker compose up -d --build
```

## 项目结构

```text
src/              Rust 后端：解析、队列、下载、校验、SQLite 和 API
web/              NAS 网页，编译时内嵌进后端，必须保留
Cargo.toml        Rust 依赖配置
Cargo.lock        锁定依赖版本
compose.nas.yaml  使用预构建镜像部署 NAS
compose.yaml      从源码构建并运行
.env.example      部署环境变量模板
deploy/Dockerfile Docker 镜像构建文件
dist/            NAS Docker 镜像及 SHA256 校验清单
third-party/      第三方组件许可通知
local/nas-deploy/  当前 NAS 私有配置、部署辅助工具及验收记录，不上传
.github/          Docker 构建工作流
```

电脑端脚本、安装包、旧版工具、本地构建缓存、独立集成测试目录和非 NAS 测试产物已移除。源码内的单元测试仍可用于后续调试；重新在本机编译时会自动生成 `target/`。

## 接口和配置

认证接口使用 `Authorization: Bearer <令牌>`。`/healthz` 为健康检查。

- `POST /api/tasks`：提交 `{"urls":"网页链接\n网页链接","headers":{}}`。
- `GET /api/tasks`：任务列表。
- `POST /api/tasks/{id}/cancel`、`/retry`：取消或重试。
- `DELETE /api/tasks/{id}`：删除终态记录，保留视频；活动或取消清理中的任务返回 400，未知记录返回 404。
- `GET /api/tasks/{id}/log`：日志。
- `GET /api/settings`、`PUT /api/settings`：读取和保存下载配置。

可保存配置为绝对输出路径、并发数（1–8）、分片线程数（1–32）和尝试上限（1–10，包含首次）。环境变量还支持 `PC_RETRIES`（默认 3）、`PC_TIMEOUT`（默认 21600 秒）、`PC_DOWNLOADER`、`PC_FFMPEG`、`PC_FFPROBE`、`PC_BROWSER`；Docker 已配置这些工具。

## 许可

主项目 MIT。下载引擎来自 N_m3u8DL-RE，媒体处理使用 FFmpeg，动态解析使用 Chromium。第三方许可和来源保留在 `third-party/`，构建镜像时一并复制。
