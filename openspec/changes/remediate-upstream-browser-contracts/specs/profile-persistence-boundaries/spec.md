# Spec Delta

## Purpose

补齐已有持久化 profile 在 CLI、CDP 及 MCP stdio/HTTP 生命周期中的载入、提交和重启契约，并使保存失败及平台私有权限真正对操作者可见。范围只包含现有 cookies 与 origin localStorage，不把手工导入导出当作自动持久化，不借此新增耐久 IndexedDB、CacheStorage、scrape profile 或跨进程数据库能力。

## ADDED Requirements

### Requirement: RC-26 已有 profile 的耐久与权限

浏览器 SHALL 对已有 cookies 与 origin localStorage profile 提供自动载入、有序原子提交、可观察失败、平台私有权限和重启隔离契约；全部场景共同构成本 Requirement 的完成条件，不将未批准存储扩展纳入范围。

#### Scenario: 完整 profile 生命周期与安全提交约束

**WHEN** 操作者通过已支持入口使用 profile，并执行导航、存储修改、提交、退出或重启

**THEN** 下列载入、提交、失败、权限和隔离约束均生效：

浏览器 SHALL 在已有支持 profile 的 CLI fetch/serve、CDP 以及 MCP stdio/HTTP 入口中使用明确配置的同一持久化 profile，在首次相关请求或脚本执行前载入已有 cookies 和按 origin 分区的 localStorage。显式配置的 profile 优先于环境默认值；未配置 profile 时保持进程内存语义，不隐式写盘。兼容的既有 profile 内容须可继续载入，不能以重新登录或强迫调用手工 storage-state 导入工具替代。

浏览器 SHALL 在完成一次导航时提交该边界前已生效的 cookie 与 localStorage 修改，并在可处理的干净退出时提交最后一次导航后已生效的修改，覆盖 MCP stdio EOF 及 MCP HTTP 的正常关闭路径。导航成功边界不能只保存其中一种存储；没有后续导航的工具求值和后台页面任务修改也必须在干净退出前保存。提交须与正在完成的页面工作有明确次序，不能移动原生 isolate 到其他线程、复活已销毁文档或通过关闭执行期限保护等待无限任务。

浏览器 SHALL 对每个被替换的持久化存储单元执行原子提交：提交成功时该单元的完整新状态可载入，写入或替换失败时原有已提交内容保持可载入，不能留下被截断的正常 profile 内容。删除 cookie、清空 origin 及删除 localStorage 项也须在提交后重启继续生效。未成功提交的修改不得被标记为已持久化；后续正常提交可保存尚未提交的状态，不要求额外重试服务，也不承诺 cookie 与所有 origin 文件组成跨存储事务。

浏览器 SHALL 明确报告 profile 载入、目录创建、写入、替换、删除及权限建立失败。对一次需要持久化的操作，CLI 通过实际可见的标准错误和非零退出状态报告提交失败，MCP/CDP 通过相应操作的错误结果报告，干净退出阶段的失败也不能被作为成功退出吞掉；消息须区分浏览器内存中已发生的操作与未被保存的状态，且不输出 cookie 值或其他会话秘密。日志订阅器是否启用不得决定操作者能否得知持久化失败。未配置 profile 的内存操作不因没有持久化而被误报为保存失败。

浏览器 SHALL 对受管理的 profile 目录和含会话内容的持久化文件明确建立并检查平台私有访问权限。Unix 上受管理目录为 0700、文件为 0600；Windows 上采用限制当前用户及运行所必需系统主体的等价 DACL，不向其他普通用户开放。新建、已有宽松权限文件的替换及卷挂载场景均须遵守该边界；无法安全建立权限时明确失败，不能依赖临时文件偶然权限或把 Unix mode 数字当作 Windows 权限证据。

浏览器 SHALL 保持 origin、浏览器上下文与 profile 之间的隔离、cookie 的既有作用域和有效期规则，以及现有 localStorage 配额和字符串往返语义。profile 恢复不会使 sessionStorage 跨进程存在，也不会自动授予位置或设备权限；不同独立进程须使用不同 profile，本需求不新增共享目录的跨进程一致性保证。IndexedDB、CacheStorage 和未获批准的 scrape profile 扩展不属于本次持久化完成条件。

#### Scenario: CLI 自动载入和提交后重启

**WHEN** 操作者使用显式 profile 运行 fetch 或 serve，本地服务设置具有明确有效期的 cookie，页面写入 localStorage，进程完成相应提交并退出后以同一 profile 重启

**THEN** 重启后的首次请求携带符合域、路径、Secure 与有效期规则的 cookie，首次页面脚本可读取先前 origin 的 localStorage，不需要手工导入状态；过期或不匹配作用域的 cookie 不发送。

#### Scenario: MCP 两种 transport 的 profile 入口真实生效

**WHEN** 操作者分别以 MCP stdio 和 HTTP 使用同一个已提交 profile 的独立顺序运行实例，并通过 `browser_navigate` 导航到本地会话服务、通过 `browser_evaluate` 读取 localStorage

**THEN** 两种 transport 都在首个请求前载入会话和对应 origin 状态，服务认可已有会话，脚本读到已提交值；明确指定的 profile 不被环境默认目录替换，不能以手工 `browser_set_storage_state` 调用代替。

#### Scenario: 导航提交同时覆盖 cookies 与 localStorage

**WHEN** MCP stdio、MCP HTTP 或 CDP 导航完成，本地响应更新 cookie，导航脚本写入 localStorage，操作者在观察到导航操作成功后停止该实例而不执行手工保存

**THEN** 另一个顺序启动的实例从该提交点读到完整 cookie 和 localStorage 更新；导航前的旧版本不会因只刷新其中一类存储而被错误保留，强制停止后的保证只覆盖此前已成功提交的内容。

#### Scenario: 最后导航后的修改在干净退出提交

**WHEN** MCP stdio 或 HTTP 已完成导航，随后 `browser_evaluate` 和已实际执行的页面任务再次修改 cookie/localStorage，此后不再导航，分别通过 stdio EOF 和可处理的正常关闭信号结束进程

**THEN** 服务先确定已完成页面工作的提交次序，再保存两类最终状态并成功退出；重启后读取最后的值，不只恢复导航时的快照，未完成的无限页面任务仍受原有执行期限保护。

#### Scenario: 删除和清空不会在重启后复活

**WHEN** 页面或既有控制接口删除已持久化 cookie、删除 localStorage 项或清空某个 origin，完成导航提交或干净退出后重新打开同一 profile

**THEN** 被删除状态保持不存在，其他 origin 的有效状态不变，cookie 的删除属性和作用域仍按既有浏览器规则处理，不会因残留旧文件而恢复已清空内容。

#### Scenario: 写入或替换失败保留旧提交并明确报错

**WHEN** 一个已有可载入 profile 在下次实际提交时遇到不可写目录、受阻替换目标或空间不足，客户端通过 CLI、MCP 或 CDP 触发需要提交的导航、导入或退出边界

**THEN** 对应操作者收到可观察的提交错误，CLI 或退出失败不能返回成功状态，错误区分内存修改和未保存结果；重启仍可载入原先已提交内容，没有截断数据或成功落盘的假象，修复条件后一次正常提交能保存仍待提交的状态。

#### Scenario: 损坏与不可读输入不被当作全新空 profile

**WHEN** 启动时已有 cookie 或 localStorage 内容损坏、不可读或违反现有 origin/配额约束

**THEN** 操作者得到具体载入错误和受影响范围，既有内容不会被无提示地覆盖为空状态，消息不包含会话秘密；不存在的初始 profile 则可正常创建，不误判为损坏。

#### Scenario: 平台私有权限覆盖新建和既有目录

**WHEN** 操作者在 Unix、Windows 和可支持相应私有权限的挂载卷上新建 profile，或对含宽松权限旧文件的 profile 完成替换提交

**THEN** 实际目录和文件访问控制符合平台私有权限要求，同一用户可重启载入，其他普通用户不能读取会话内容；平台拒绝权限建立或卷不支持必要保护时产生明确错误，不静默以宽松权限继续。

#### Scenario: origin 和上下文边界在重启前后保持

**WHEN** 两个 origin 和两个独立 profile 分别保存不同 cookie/localStorage，随后顺序重启；页面还写入 sessionStorage、内存 IndexedDB 并查询位置权限

**THEN** 每个 origin 和 profile 只恢复自身允许的数据，不相互串用；sessionStorage 不从前一进程恢复，IndexedDB 不被宣称具备新增磁盘耐久，保存区域坐标或会话状态不自动授予 geolocation 权限。

#### Scenario: 无 profile 与强制结束的边界不被扩大

**WHEN** 浏览器未配置 profile，或进程在尚未完成提交时被无法处理的强制终止，而后以新进程继续运行

**THEN** 未配置 profile 的状态不被隐式保存，强制终止不被承诺能保留未提交修改；已成功提交的存储单元仍须可载入，文档不把本需求描述为跨进程数据库、耐久 CacheStorage 或新增 scrape profile 能力。
