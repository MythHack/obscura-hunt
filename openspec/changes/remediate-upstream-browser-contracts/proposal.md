# Proposal

## Why

当前分支已合入上游 a754dce，但浏览器契约在请求、CDP/MCP 生命周期、DOM/Worker 和渲染边界仍有已复现或源码支持的缺口。用户已同意前两组修复及完整公钥 WebCrypto，并要求补齐 GeoIP 和全部开放 Issue 的处置；本变更保留 26 个 RC 功能契约，收敛为 10 个交付任务和 11.1 统一收口。对应上游 PR 实现及已有测试是首选，不整批盲合，也不从零重写已经可复用的实现。

## What Changes

- 按首个信息丢失/状态分歧边界保留全部 26 个 RC，按共同拥有边界合并为 1.1–10.1 十个交付任务；生产闭环、必要回归和消费者结果属于同一任务，不再拆成独立实现/验收清单。映射见 [design](design.md#1-按拥有边界收敛不建立总括框架)，11.1 只承担共享集成验证与交付。
- 补齐请求身份、发送前拦截、原生传输选择、权限下沉、证书补链和请求证据；维持证书、代理及来源边界。
- 修复目标/会话所有权、实时 DOM mutation、导航取消及等待；保留原生 realm、线程归属与 scoped node IDs。
- 修复真实 DOM/Worker/资源语义、XML、流式解码、本地 scheme、同步 XHR、WebSocket、CSS/控件/Canvas/SVG/几何和矢量文字 PDF。
- 新增完整七类公钥 WebCrypto；新增自动/显式 IP GeoIP、区域加权语言、原生区域配置、位置权限与可验证的 RTC 地址/传输一致性。不能用空接口或 IP 字符串重写冒充完成。
- 修复已有 cookie/localStorage profile 的持久化收尾和权限；不把 IndexedDB/CacheStorage 的新持久化体系悄悄加入。
- **BREAKING**：字节保真 API 如需调整，将迁移所有 Rust/CLI/CDP/MCP 消费者并删除旧字符串别名；不保留会损坏二进制的兼容路径。未实现触摸/媒体请求从假成功改为明确错误。
- 保留 102 个开放 Issue、92 个开放 PR 的原始历史快照与分类、本地观察及已有执行证据；这些不是当前开放数量或新产物通过声明。未知历史调查继续在 [调查登记](investigations.md) 可见，不自动形成必过任务；已有具体复现或本次改动触及的不安全路径仍归对应交付任务。

## Capabilities

### New Capabilities

- `network-request-contracts`: 请求身份与元数据、原生传输选择、请求前拦截与字节保真、文件与地址访问权限、证书链缺失恢复、级联选中资源的加载。
- `cdp-browser-lifecycle`: 控制面路由与敏感产物边界、网络事件与响应来源、浏览器目标与会话所有权、实时 DOM 发布。
- `navigation-cancellation`: 导航与等待取消边界。
- `dom-worker-resource-semantics`: Document 与脚本资源生命周期、Worker realm 与可转移消息、XML 文档解析、流式文本解码、本地 scheme 与同步 XHR。
- `websocket-client`: 真实 WebSocket 客户端。
- `public-key-webcrypto`: 完整公钥 WebCrypto。
- `regional-profile-permissions`: GeoIP 配置和权限一致性。
- `render-css-input-contracts`: 选择器与 typed custom properties、原生激活与输入能力契约、布局与盒几何保真、UA 样式与 CSSOM 值、Canvas SVG 与字体度量。
- `document-export`: 矢量和文字 PDF。
- `profile-persistence-boundaries`: 已有 profile 的耐久与权限。

### Modified Capabilities

无。仓库此前没有 OpenSpec 规范；新增规范文件不表示对应浏览器能力全部从零实现。

## Impact

主要涉及 obscura-net/ssrf、obscura-browser、obscura-js/dom、obscura-cdp/mcp/cli 和 obscura-render 的现有边界。共享文件由一个集成 owner 顺序落地，不把根因分组当作新增模块要求。先移植对应上游 PR 的适用实现并复用现有测试，仅修复本分支 realm/线程归属、scoped ID、SSRF/TLS 等冲突及确实缺失的行为。规范、任务和追踪见 [tasks.md](tasks.md)、[design.md](design.md) 与 [traceability.md](traceability.md)。

1.1–10.1 各自交付全部所属 RC 的生产者到真实消费者闭环；11.1 集中运行仓库要求的 release nextest、精确 release build、obstacle 和适用 render 验证，不在每个 RC 重复整套验证。修复后只重跑受影响检查；不新增完整客户端×平台×代理笛卡尔积、强制小时级 profile、新消费者框架，亦不为缺少 build.log 重建归档。状态只以任务书及匹配产物的行为证据为准，历史源码覆盖、旧构建和上游作者测试不冒充新二进制证据。
