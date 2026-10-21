# Scope Decisions

用户同意的修复与新增能力均在 proposal/tasks 中。全部 26 个 RC 按 [design 中的唯一归属表](design.md#1-按拥有边界收敛不建立总括框架)收敛为 1.1–10.1 十个交付任务，11.1 为“统一集成验证与交付”；不再拆分实现/验收复选框或另设基线任务。分类为可选或报告明确要求单独产品决定的事项，不被自动扩大成实现任务；下面每项仍有可见处置，不是遗漏。

| Issue | 处置 | 保留边界 |
| --- | --- | --- |
| [#1240](https://github.com/h4ckf0r0day/obscura/issues/1240) Add an opt-in Parallel Search MCP example for web discovery and browsing | 另立变更，当前不实现新增部分 | 完整原始需求与证据保存在 evidence/triage-report.json；未获独立实现授权，不伪造支持。 |
| [#1157](https://github.com/h4ckf0r0day/obscura/issues/1157) RFC: Persistent storage architecture (localStorage disk sync, ACID IndexedDB via redb, CacheStorage) | 另立变更，当前不实现新增部分 | 现有 localStorage 与内存 IndexedDB 事务保持；持久 IndexedDB/CacheStorage 不采用旧 PR 整体替换。 |
| [#1153](https://github.com/h4ckf0r0day/obscura/issues/1153) text layout of large pages allocates far more than it keeps | 另立变更，当前不实现新增部分 | 完整原始需求与证据保存在 evidence/triage-report.json；未获独立实现授权，不伪造支持。 |
| [#1051](https://github.com/h4ckf0r0day/obscura/issues/1051) Packaging: Official Fedora & CentOS Stream Copr repositories available | 另立变更，当前不实现新增部分 | 完整原始需求与证据保存在 evidence/triage-report.json；未获独立实现授权，不伪造支持。 |
| [#1011](https://github.com/h4ckf0r0day/obscura/issues/1011) is it possible to support realtime rendering feature? | 另立变更，当前不实现新增部分 | 现有 CDP screencast 和输入保留；SIXEL/framebuffer UI 不是本变更。 |
| [#863](https://github.com/h4ckf0r0day/obscura/issues/863) Feature: Chrome extension support (adblock/tracker-blocking first) for agent workflows | 另立变更，当前不实现新增部分 | 完整原始需求与证据保存在 evidence/triage-report.json；未获独立实现授权，不伪造支持。 |
| [#769](https://github.com/h4ckf0r0day/obscura/issues/769) Default (non-stealth) client can't reach servers that only offer legacy TLS 1.2 CBC cipher suites — rustls-tls has no CBC support | 另立变更，当前不实现新增部分 | 完整原始需求与证据保存在 evidence/triage-report.json；未获独立实现授权，不伪造支持。 |
| [#694](https://github.com/h4ckf0r0day/obscura/issues/694) Feature: persistent profile / user-data-dir for fetch & scrape (storage is lost between CLI invocations) | 另立变更，当前不实现新增部分 | fetch 已有 profile 保持；scrape 新 profile 协议和 IndexedDB 重启持久化另案。 |
| [#684](https://github.com/h4ckf0r0day/obscura/issues/684) Support `scoop` as a way to install in Windows | 另立变更，当前不实现新增部分 | 完整原始需求与证据保存在 evidence/triage-report.json；未获独立实现授权，不伪造支持。 |
| [#491](https://github.com/h4ckf0r0day/obscura/issues/491) Obscura doesn't automatically fallback to direct access when server on `HTTP_PROXY`/`HTTPS_PROXY` is down | 不采纳 | 代理失败保持失败，不静默直连；这是路由隐私边界。 |

## Candidate constraints

- [PR #1158](https://github.com/h4ckf0r0day/obscura/pull/1158)：不得用旧整体存储实现替换现有事务/typed keys/structured clone；新增持久化另案。
- [PR #1182](https://github.com/h4ckf0r0day/obscura/pull/1182)：作者记录未解决绘制/命中顺序回归，不整批盲合；适用实现仍优先复用，冲突和已知回归只在现有共享级联/布局/绘制边界最小修复，不据此另起一套实现。
- [PR #1154](https://github.com/h4ckf0r0day/obscura/pull/1154)：内存优化为可选工作，先匹配基线。
- [PR #1152](https://github.com/h4ckf0r0day/obscura/pull/1152)：优先复用适用代码；INV-1151 已确认的 classic fetch admission 剩余边界归 4.1，保留原生 readiness 与有序语义，不把已知反例退回未定调查。INV-531 已确认的 watchdog 边界亦由 4.1 承担，不关闭 watchdog。
- [PR #1156](https://github.com/h4ckf0r0day/obscura/pull/1156)：仅在 INV-1155 有具体当前剩余成本或改动触及不安全路径时采用适用优化，不恢复旧 observer 路径；历史未知不因此变成强制分析任务。

上游 PR 实现与已有测试是默认起点，不按“是否原作者代码”建立第二套实现。已合入且正确的代码不重复移植；仅处理本分支真实 realm/线程归属、scoped ID、SSRF/TLS/代理边界冲突及确实缺失行为。选用 PR 不等于接受其全部旧架构或将作者测试冒充本轮产物结果。

## Existing coverage

历史快照中 20 个已有源码/历史覆盖的 Issue 绑定相邻交付任务的回归保护，由 11.1 统一复用和核对结果，不建立 20 个重复修复或永久测试。静态覆盖不等于本次新实跑；相邻剩余缺口按各自 Issue 和 RC 主归属追踪。不可变的 [实施基线](evidence/execution-baseline.json)及其旧证据保留，不因任务重排新增通过状态。

## Investigation and verification boundaries

- [调查登记](investigations.md)保留历史缺失材料与未知，不是 19 项必过关卡。只有具体当前复现或本次改动触及不安全路径才形成对应交付任务的必修边界；已确认的 INV-1151/INV-531 归 4.1，INV-866 的具体 CDP timeout/context 问题在 3.1/4.1 内处理，不丢弃原反例。
- 11.1 集中执行仓库要求的 focused/full release nextest、精确 release build、obstacle course 33/33 和适用 render fixtures/representative top-bottom 验证；修复后只重跑受影响检查，不按每个 RC 或消费者重复整套流程。
- 不要求完整客户端×平台×代理产品矩阵、强制小时级 profile、新消费者框架，或仅为缺少 build.log 构建归档。实际受影响的平台、权限、代理或原生安全边界仍须有对应结果，不以减少重复验证删减功能。
- 先复用对应 PR 与仓库已有测试；新增测试只针对真实回归或未覆盖的必要行为/拒绝/生命周期边界，不测源码接线、空接口存在或 mock 回声。历史失败不为确认而重跑，旧产物不能验收新源码。
