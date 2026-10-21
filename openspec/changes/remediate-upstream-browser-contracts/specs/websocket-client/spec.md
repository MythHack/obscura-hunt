# Spec Delta

## Purpose

规定页面 WebSocket 客户端的真实连接、传输、事件与关闭契约，使应用能够通过 ws 和 wss 与受控服务器交换实际消息，并据真实握手和连接失败决定下一步。该能力属于页面网络通信而非 CDP 服务端自身的 WebSocket，必须保持页面来源、代理、TLS、网络授权、取消和生命周期的现有边界。

## ADDED Requirements

### Requirement: RC-16 真实 WebSocket 客户端

浏览器 SHALL 为页面提供真正的 ws/wss WebSocket 客户端，由实际握手、帧收发及关闭过程驱动 `readyState`、协议协商、`bufferedAmount`、message/error/close 事件；必须沿用配置的代理、来源与网络授权以及 TLS 校验，不得在没有连接时制造 open、丢弃有效发送帧或只提供显式不支持来代替本次批准的真实实现。

#### Scenario: WS 和 WSS 握手后才能 Open

- **WHEN** 本地受控页面连接真实 ws echo 服务及使用受信任测试证书的 wss echo 服务，并提供可协商的子协议列表。
- **THEN** 构造后最初为 CONNECTING，仅在实际成功握手后进入 OPEN 并触发一次 open；服务器记录真实 Upgrade、正确的受保护 Origin 和所选子协议，页面 `protocol` 反映服务器真正接受的值而非直接复制首个候选，`extensions` 只报告实际协商结果。

#### Scenario: 文本与二进制帧真实往返

- **WHEN** 页面向 echo 服务发送 Unicode 文本、Blob、ArrayBuffer 和带 offset/length 的 TypedArray/DataView，并依次使用默认 `binaryType='blob'` 与 `arraybuffer` 接收。
- **THEN** 服务端记录与发出内容相同的文本或精确字节，页面收到真实 echo，二进制类型符合 binaryType 且仅包含视图指定范围；消息保持顺序，`bufferedAmount` 对应待发送字节并随真实传输排空，不以本地直接回调模拟 echo。

#### Scenario: 帧协议与控制帧不是页面假事件

- **WHEN** 服务端把一条文本或二进制消息拆为多个合法 continuation 帧，并发送 ping，随后发送另一条独立消息；另一个连接返回非法帧或损坏 UTF-8 文本。
- **THEN** 页面按完整消息而非帧片段交付数据，控制帧按协议处理且不伪装成 message，服务端可观察到合法 pong 和客户端帧编码；协议错误导致真实连接失败及对应关闭，不把损坏内容当成正常文本继续交付。

#### Scenario: 代理与来源策略共同约束真实传输

- **WHEN** 页面分别在配置的 HTTP 代理和 SOCKS5 代理下连接 ws/wss 受控服务，并通过服务器记录检查 Origin、Cookie 与协商头；再尝试页面篡改受保护 Origin 或连接不被允许的 private-network、file/non-WebSocket URL 及被策略禁止的目标。
- **THEN** 服务与代理记录证明通信确实经过配置代理及适用隧道，握手使用浏览器来源和凭据规则；不允许的目标不会出现成功握手，页面不能写入伪造 Origin、绕过来源/CSP/混合内容与网络权限，协议服务端的连接也不能被当作页面客户端验收。

#### Scenario: 死端口和 TLS 失败绝不制造连接成功

- **WHEN** 连接没有监听的受控端口、握手被拒绝的服务、子协议响应非法的服务或证书不可信/主机名不匹配的 wss 服务。
- **THEN** 不触发 open，页面观察与失败对应的 error 及非正常 close，状态最终为 CLOSED；无有效关闭握手时不能声称 `wasClean=true`，不得降低 TLS 校验或回退到直接、不加密的连接来取得成功。

#### Scenario: 发送时机与 Close 参数遵守浏览器边界

- **WHEN** 页面在 CONNECTING 时 send，使用无效 URL、重复/非法子协议、无效 close code 或超过 123 UTF-8 字节的 close reason，随后对正常 OPEN 连接请求合法关闭。
- **THEN** 无效构造/参数产生相应的浏览器异常，CONNECTING send 抛出 `InvalidStateError`；合法关闭经历 CLOSING 并发出真正 close 帧，双方完成握手后才交付对应 code/reason/wasClean；关闭后不再交付新的应用消息，也不借重试或静默重新连接改变一次 WebSocket 的身份。

#### Scenario: 对端异常和文档退出释放真实连接

- **WHEN** echo 服务中断底层连接，或页面在连接、收发、关闭过程中导航、关闭或取消所属文档。
- **THEN** 对端异常按真实网络结果体现，文档退出终止其待处理连接和传输并释放网络资源；旧连接不能向新文档派发 open/message、复用新文档权限或阻塞执行期限，其他独立页面的连接不受误关闭影响。
