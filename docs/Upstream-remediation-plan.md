# 上游浏览器契约修复计划

本页是 OpenSpec 变更 `remediate-upstream-browser-contracts` 的入口。用户已同意原先两组修复、完整公钥 WebCrypto 和补查后的范围处置。原调查与实施基线为 `2086082`，当时上游同步至 `a754dce`；它们是历史定位信息，不代表当前源码或产物状态。

**交付结构：26 个 RC 功能契约保留，合并为 1.1–10.1 十个交付任务，11.1 为“统一集成验证与交付”。** [任务书](../openspec/changes/remediate-upstream-browser-contracts/tasks.md)不再重复拆分生产实现/验收或另设基线复选框；共享源码统一集成，仓库必需验证集中运行，修复后只重跑受影响检查。编号重排、创建规范、读取源码或保存历史观察都不增加通过数。

## 开始执行

- [Proposal：目标、能力和明确边界](../openspec/changes/remediate-upstream-browser-contracts/proposal.md)
- [Tasks：十个交付任务与统一集成验证/交付关卡](../openspec/changes/remediate-upstream-browser-contracts/tasks.md)
- [Design：现有拥有边界、切换和验证方法](../openspec/changes/remediate-upstream-browser-contracts/design.md)
- [Traceability：102 个 Issue、候选 PR、Requirement 和主任务的关联](../openspec/changes/remediate-upstream-browser-contracts/traceability.md)
- [Investigations：缺失证据与有界调查](../openspec/changes/remediate-upstream-browser-contracts/investigations.md)
- [Scope decisions：已有覆盖、另案和不采纳事项](../openspec/changes/remediate-upstream-browser-contracts/scope-decisions.md)

仓库首次初始化 OpenSpec，因此规范使用 ADDED Requirements；这不表示所有现有能力要重写。对应上游 PR 实现与已有测试是首选，只最小化修复本分支 realm/线程归属、scoped ID、SSRF/TLS 等真实冲突及确实缺失的行为；根因分组不要求新增运行时模块或消费者框架。

## 范围中的重点

- 请求身份、真实发送前拦截和字节保真、原生客户端选择、文件/CIDR 权限、证书链与请求证据。
- 跨连接目标发现、页面端点、会话订阅、实时 DOM 发布、MCP 等待和导航取消。
- Window/Worker/MessagePort/importScripts、XML、preload、流式编码、本地 scheme、同步 XHR 和真正的 WebSocket。
- CSS 查询与级联、UA 样式、控件/激活、布局/几何、Canvas/SVG/字体、真实矢量文字 PDF。
- 七类公钥 WebCrypto，包括 Issue 正文中的 X25519，以及适用的密钥格式、操作和错误语义。
- 自动/显式 IP GeoIP、人口加权的稳定语言、原生区域配置、独立位置权限与真实可验证的 RTC 地址/传输行为。
- 已有 cookie/localStorage profile 的正确提交、私有权限和可观察失败，不暗中换成新的持久 IndexedDB/CacheStorage 架构。

## 覆盖与证据边界

[原始分类快照](../openspec/changes/remediate-upstream-browser-contracts/evidence/triage-report.json)完整保留当时 102 个开放 Issue 的逐项结论；[原始 Issue 正文](../openspec/changes/remediate-upstream-browser-contracts/evidence/open-issues.json)和 [92 个开放 PR 正文](../openspec/changes/remediate-upstream-browser-contracts/evidence/open-pull-requests.json)同时保留，不把历史数量改写为当前开放状态。最初将 16 项首批结果误认完整列表的错误也在快照中记录，不重写历史。

全部正文已读不等于全部现象已复现；历史关闭 Issue 和全部历史评论未做穷举审计。已有覆盖项按回归保护处置，未知历史调查继续可见而非强制关卡；具体当前复现或本次改动触及的不安全路径仍由相应交付任务修复。已确认的 INV-1151/INV-531 归 4.1，INV-866 的具体 CDP timeout/context 问题在 3.1/4.1 内处理。可选/另案/拒绝事项仍有明确归属，不把全部开放项变成生产修复。

[历史本地观察](../openspec/changes/remediate-upstream-browser-contracts/evidence/local-observations.json)只说明此前看到了哪些缺口，没有执行当时的二进制 SHA，不能用于勾选本计划的新任务。正式验收需要实际二进制 SHA、源码状态、平台/features、输入、命令、退出码和可观察结果。

[实施基线](../openspec/changes/remediate-upstream-browser-contracts/evidence/execution-baseline.json)原样保留当时源码、初始工作区、CLI/worker 的实际 SHA 和 CLI build-info。该记录中的 CLI 属于较早的 dirty render 构建，只用于诊断，不能验收本轮新源码，也不因任务重排新增通过状态。

## 查看与验证

在仓库根目录运行：

```bash
openspec status --change remediate-upstream-browser-contracts
openspec instructions apply --change remediate-upstream-browser-contracts --json
openspec validate remediate-upstream-browser-contracts --strict --no-interactive
```

`instructions apply` 只读取任务，不启动实施。实施时继续遵守[构建](Build-from-source.md)及[测试与渲染验收](Testing-and-debugging.md)规则；不能用 OpenSpec 格式通过替代浏览器行为证明。

11.1 保留集中 focused/full release nextest、精确 release build、obstacle course 33/33，以及适用的 deterministic render fixtures 和 representative top/bottom 验证。已有消费者和测试优先；新增测试只针对真实回归或未覆盖的必要边界。不扩成客户端×平台×代理的完整产品矩阵，不强制小时级 profile，不为缺少 build.log 重编或构建归档。
