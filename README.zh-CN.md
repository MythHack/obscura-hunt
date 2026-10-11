# Obscura-Hunt

[English](README.md) | **简体中文**

Obscura 是面向网页抓取和 AI agent 自动化的 Rust 无头浏览器引擎，通过 V8 执行真实 JavaScript，
通过 Chrome DevTools Protocol（CDP）与 Puppeteer、Playwright 客户端交互。

**Obscura-Hunt** 是 [MythHack 维护的分支](https://github.com/MythHack/obscura-hunt)，
源自 [Obscura](https://github.com/h4ckf0r0day/obscura)，重点让 HuntForge 及其他自动化客户端的
浏览器操作更可靠。它保留上游的原生渲染、stealth 选项和 CDP/MCP 接口。
对于已支持的工作流程，它可以替代 headless Chrome，但并不完全兼容每个 Chrome API 或网站。

## 本分支有哪些实际改进？

- **更完整的弹窗流程。** 支持打开弹窗、与原页面传递消息，以及判断弹窗是否关闭。
  跳转或关闭一个页面，不会清掉其他页面已经加载的 JavaScript 模块。
- **可以实际操作 iframe 内的页面。** 支持点击内嵌页面的按钮、提交表单，输入和请求会送到
  正确的子页面。这些操作仍然遵守跨源访问限制和网页自身的嵌入策略。
- **修复页面跳转后的等待问题。** 页面加载新 URL 后，定时器和异步脚本继续运行。
  MCP 等待期间也会让页面正常执行，不再等着一个根本没机会运行的定时器。
- **缩放会真正改变画面和点击位置。** CSS `zoom` 同时影响布局、绘制和点击位置，
  而不只是返回一个缩放数值。`perspective` 已参与样式计算和定位，但尚不绘制 3D 透视效果。
- **弹窗沿用原来的请求拦截规则。** 如果自动化客户端只拦截指定 URL，
  打开弹窗不会意外变成拦截所有请求。

这些改进描述的是当前源码。引擎、V8、渲染器、stealth 传输和通用自动化接口都是上游已有的工作，
并非本分支从零创造的功能。

## 从源码构建

需要当前改动时，请从本仓库构建。项目名称是 Obscura-Hunt，命令和可执行文件仍叫
`obscura` 与 `obscura-worker`。

### 构建要求

- 当前稳定版 Rust 和 C/C++ 工具链。
- stealth 构建需要 CMake、Clang 和 libclang/LLVM 开发库。
- Windows x64 需要 Visual Studio C++ Build Tools、CMake、LLVM/Clang，以及 `PATH` 上的 NASM。
  找不到 libclang 时，将 `LIBCLANG_PATH` 指向 LLVM 的 `bin` 目录。

Ubuntu/Debian 可这样安装原生工具：

```bash
sudo apt-get install build-essential cmake clang libclang-dev llvm-dev
```

### 构建渲染与 stealth 版本

```bash
git clone https://github.com/MythHack/obscura-hunt.git
cd obscura-hunt

# 推荐使用编译缓存；已经安装时跳过这一步。
cargo install sccache --locked
```

POSIX shell：

```bash
export RUSTC_WRAPPER=sccache
cargo build --release --locked -p obscura-cli --bins --features render,stealth
```

PowerShell：

```powershell
$env:RUSTC_WRAPPER = "sccache"
cargo build --release --locked -p obscura-cli --bins --features render,stealth
```

默认构建产物位于 `target/release`（Windows 为 `.exe`）。复制到其他目录时，也要将同一构建的
`obscura` 与 `obscura-worker` 放在一起。并行抓取、多 worker 服务等命令需要 worker。
将该目录加入 `PATH`，或在命令和 MCP 客户端配置中使用可执行文件的完整路径。

Cargo 自动使用本分支随仓库提供的依赖补丁，构建时请保留它们。
补丁说明和更多平台指导见 [从源码构建](docs/Build-from-source.md)。
`sccache` 不能代替原生构建工具。

只需要渲染、不需要 stealth 时，使用 `--features render`。
不需要渲染时，使用 `--no-default-features`，可再加 `--features stealth`；
每种构建都保留 `--release -p obscura-cli --bins`。
无渲染构建不提供截图、screencast 或 PDF 输出。

### 下载发布包

发布包可用后，会放在 [本分支 Releases 页面](https://github.com/MythHack/obscura-hunt/releases)。
目前请从源码构建；上游发布包和 Docker 镜像不包含本分支的改动。

## 使用方法

以下示例假设本分支可执行文件已经加入 `PATH`。

### 读取页面或截图

```bash
obscura fetch https://example.com --eval "document.title"
obscura fetch https://example.com --dump links
obscura fetch https://example.com --dump text --output page.txt
obscura fetch https://example.com --screenshot page.png
```

截图需要渲染功能，由原生引擎生成，不会启动 Chromium。

### 连接 Puppeteer 或 Playwright

```bash
obscura serve --port 9222

# 已编译 stealth 时，在运行时启用。
obscura serve --port 9222 --stealth
```

Puppeteer 连接 `ws://127.0.0.1:9222/devtools/browser`。
Playwright 的 `chromium.connectOverCDP` 连接 `ws://127.0.0.1:9222`。
可用示例见 [Puppeteer 指南](docs/Use-with-Puppeteer.md) 和
[Playwright 指南](docs/Use-with-Playwright.md)。

### 同时抓取多个页面

```bash
obscura scrape https://example.com https://news.ycombinator.com --concurrency 2 --format json
```

将 `obscura-worker` 保持在 `obscura` 同目录。

### 使用 MCP 客户端

```bash
# stdio：供启动子进程的客户端使用
obscura mcp

# HTTP：端点 http://127.0.0.1:8080/mcp
obscura mcp --http --port 8080
```

先用 `browser_navigate` 导航，通过 `browser_snapshot` 读取页面，再用 `browser_click`、
`browser_fill` 等工具操作。执行异步 JavaScript 时，`browser_evaluate` 支持 `awaitPromise: true`。
等待稍后才会满足的条件时，使用 `browser_wait_for_function`；其 `timeout` 和 `pollIntervalMs`
以毫秒为单位，条件未满足时返回超时错误。

启用渲染的构建可通过 MCP 输出静态截图和 PDF；流式 screencast 使用 CDP。
客户端配置、工具和 HTTP 认证说明见 [MCP 指南](docs/Use-the-MCP-server.md)。
非 loopback 的 HTTP 绑定需要认证。如果安装了其他 Obscura，请明确选择本分支可执行文件。

### 访问自己的本机开发服务

默认阻止私有/内部网络请求。访问自己控制的服务时，显式启用：

```bash
obscura fetch http://127.0.0.1:3000 --allow-private-network --dump text
obscura serve --port 9222 --allow-private-network
```

完整网络规则见 [环境变量说明](docs/Environment-variables.md)。

## 已知限制

- 部分 CSS、Web API、媒体和字体渲染与 Chromium 不同。关联窗口已支持，但任意子文档
  isolated world 执行尚未实现。
- 不支持的 frame transform 和嵌入策略语法会安全拒绝，而不是报告成功交互。
  CSS `perspective` 尚不绘制 3D 透视效果。
- stealth 不保证绕过 Cloudflare 或其他反自动化系统。

## 更多指南

- [CLI 参数参考](docs/CLI-reference.md)
- [Cookie 与存储](docs/Persist-cookies-and-storage.md)
- [从源码构建](docs/Build-from-source.md)

## 署名与许可证

Obscura-Hunt 基于 [h4ckf0r0day/obscura](https://github.com/h4ckf0r0day/obscura)。
保留上游贡献者署名、版权和许可证声明。
采用 [Apache License 2.0](LICENSE)。
