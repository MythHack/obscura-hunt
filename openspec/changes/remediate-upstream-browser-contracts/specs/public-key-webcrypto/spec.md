# Spec Delta

## Purpose

为真实页面的登录加密、令牌签名校验、DPoP 请求证明和密钥协商提供完整、可互操作的公钥 WebCrypto 契约。能力以浏览器实际生成和使用的密钥、标准编码及密码学结果为准，既不能用 API 存在性或伪造输出替代，也不能以只支持签名或导入密钥的子集冒充七类算法完成。

## ADDED Requirements

### Requirement: RC-17 完整公钥 WebCrypto

浏览器 SHALL 为下述七类公钥算法交付完整、真实且可互操作的 WebCrypto 密钥与运算契约，并保持标准格式、参数、用途、提取限制、错误及执行安全边界；全部场景共同构成本 Requirement 的完成条件。

#### Scenario: 完整算法矩阵与标准操作约束

**WHEN** 安全来源页面通过 `crypto.subtle` 生成、导入、导出或使用下表算法的公私钥

**THEN** 下列完整算法、格式、操作和拒绝约束均生效：

浏览器 SHALL 通过 `crypto.subtle` 为下表全部七类算法提供真实密钥生成、导入、导出及对应操作；所有密钥均须能完成相应密码学计算，而不是固定字节、随机占位符或仅保存调用参数的对象。密钥生成使用密码学安全随机源。表中未适用的操作或格式不因该算法已支持其他操作而被默认为可用。

| 算法 | 适用公钥操作 | 密钥格式 |
| --- | --- | --- |
| RSA-OAEP | `generateKey`、`importKey`、`exportKey`、`encrypt`、`decrypt`，及作为包装算法的 `wrapKey`、`unwrapKey` | 公钥 SPKI/JWK；私钥 PKCS8/JWK |
| RSASSA-PKCS1-v1_5 | `generateKey`、`importKey`、`exportKey`、`sign`、`verify` | 公钥 SPKI/JWK；私钥 PKCS8/JWK |
| RSA-PSS | `generateKey`、`importKey`、`exportKey`、`sign`、`verify` | 公钥 SPKI/JWK；私钥 PKCS8/JWK |
| ECDSA | `generateKey`、`importKey`、`exportKey`、`sign`、`verify` | 公钥 raw/SPKI/JWK；私钥 PKCS8/JWK |
| ECDH | `generateKey`、`importKey`、`exportKey`、`deriveBits`、`deriveKey` | 公钥 raw/SPKI/JWK；私钥 PKCS8/JWK |
| Ed25519 | `generateKey`、`importKey`、`exportKey`、`sign`、`verify` | 公钥 raw/SPKI/JWK；私钥 PKCS8/JWK |
| X25519 | `generateKey`、`importKey`、`exportKey`、`deriveBits`、`deriveKey` | 公钥 raw/SPKI/JWK；私钥 PKCS8/JWK |

浏览器 SHALL 遵循 WebCrypto 的算法归一化、密钥类型、`algorithm` 元数据、`usages` 分配及 `extractable` 规则；支持 RSA 的 SHA-1/SHA-256/SHA-384/SHA-512 参数，以及 ECDSA/ECDH 的 P-256、P-384、P-521 标准曲线。RSA 密钥的模数、公钥指数和哈希绑定，RSA-OAEP 的 label 与消息长度限制，RSA-PSS 的 saltLength，ECDSA 的操作哈希与定长 `r || s` 签名编码，以及两类协商算法的对端公钥、输出位数和派生目标密钥约束，均须实际影响输出或拒绝结果。Ed25519 和 X25519 不得被当作可任意替换哈希或曲线的通用算法。

浏览器 SHALL 按 WebCrypto 校验 DER、raw 和 JWK 的算法、密钥种类、曲线、必需参数及 `kty`、`alg`、`use`、`key_ops`、`ext` 等适用约束；格式或参数不支持时拒绝，不能把畸形公钥当成对称字节密钥。不可导出的私钥仍可执行获准操作，但不能通过导出、包装、属性修改或克隆变为可导出。包装与解包须尊重独立的 `wrapKey`/`unwrapKey` usage，并保留被包装公钥或私钥的标准格式和解包时指定的使用限制。

浏览器 SHALL 让适用操作与独立实现可互操作，并对确定性结果匹配已知向量。无效签名按算法规定返回 `false`；错误算法、用途、密钥种类、格式和运算失败分别按 WebCrypto 返回对应的 `NotSupportedError`、`InvalidAccessError`、`SyntaxError`、`DataError` 或 `OperationError`，而非统一假成功或进程异常。涉及随机性的密钥生成、OAEP、PSS 或 ECDSA 签名不要求重复产生同一字节串。既有对称算法、密钥克隆语义、原生线程亲和性及执行期限保护不得因新增公钥能力被移除。

#### Scenario: 七类算法均生成并使用真实密钥

**WHEN** 安全来源页面分别为七类算法生成可导出和不可导出的密钥对，并使用每一对密钥完成表中适用的加解密、签名验证或双向协商

**THEN** 每一类算法均产生与所选参数一致的真实公私钥，获准操作成功，独立生成的对端能使用导出的公钥验证、加密或协商，且请求非导出私钥不妨碍其获准的运算。

#### Scenario: RSA-OAEP 加密与包装独立互通

**WHEN** 页面与独立浏览器交换 RSA-OAEP 的 SPKI/JWK 公钥及可导出私钥，使用 SHA-1、SHA-256、SHA-384、SHA-512 和含非空 label 的合法消息相互加解密，并以仅有 `wrapKey`/`unwrapKey` 用途的 RSA 密钥包装、解包尺寸允许的 AES 密钥

**THEN** 双向解密获得原始明文，解包后的 AES 密钥能解密对端实际生成的密文，且合法包装不额外要求包装密钥拥有 `encrypt` 或 `decrypt` usage。

#### Scenario: RSA-OAEP 错误边界不产生明文

**WHEN** 页面以错误 label、错误私钥、篡改密文、超过所选模数和哈希允许长度的消息或不匹配的密钥用途执行 RSA-OAEP

**THEN** 无效运算以适用错误拒绝，不返回占位明文；包装不可导出的密钥失败，且失败后原密钥仍能完成其他获准操作。

#### Scenario: RSA 签名两类算法覆盖哈希与盐长

**WHEN** 页面使用 RSASSA-PKCS1-v1_5 和 RSA-PSS 的生成或导入密钥，分别针对支持的四种哈希签名，RSA-PSS 同时使用零盐长、哈希长度盐及合法边界盐长，并与独立浏览器交换签名

**THEN** 双向验证均成功，RSASSA-PKCS1-v1_5 的固定密钥和消息签名与已知向量逐字节相同；错误消息、错误签名及错误 PSS 盐长不能通过验证，超出允许范围的签名盐长拒绝执行。

#### Scenario: ECDSA 三条曲线与 DPoP 消费者

**WHEN** 页面为 P-256、P-384、P-521 分别生成密钥并以标准 SHA 参数签名，同时以 P-256 公钥 JWK 构造并签署本地服务要求的 ES256 DPoP JWT

**THEN** 独立浏览器可验证每条曲线的定长 `r || s` 签名，本地服务能用 JWK 独立验证 DPoP 证明后接收实际 HTTP 请求；服务拒绝被篡改的证明，不以跳过证明或网站专用分支获得成功。

#### Scenario: ECDH 和 X25519 双向协商与派生消费

**WHEN** 页面与独立浏览器分别使用 ECDH 的三条标准曲线及 X25519，交换真实公钥，调用 `deriveBits`，并用 `deriveKey` 生成 AES-GCM 密钥加密实际消息

**THEN** 双方得到相同的规定长度共享秘密，已知向量的共享秘密逐字节相同，派生密钥能跨浏览器解密；曲线不匹配、错误对端密钥种类、过长输出和 X25519 全零共享结果按标准拒绝。

#### Scenario: Ed25519 确定性签名

**WHEN** 页面导入 RFC 8032 的 Ed25519 测试密钥和消息，并另行生成密钥与独立浏览器交换公钥和签名

**THEN** 已知密钥的签名逐字节匹配向量，双向验证真实签名成功，篡改消息或签名返回 `false`，不能通过替换输出为固定长度随机数据满足该结果。

#### Scenario: 所有适用格式均可被独立实现消费

**WHEN** 页面为七类算法分别导入和导出表中全部适用的公钥及私钥格式，使用另一浏览器生成的 JWK、SPKI、PKCS8 以及适用的 raw 公钥完成往返

**THEN** 密钥参数、公开元数据、提取限制和获准操作保持一致，往返后的密钥能完成真实密码学操作；RSA raw 格式、raw 私钥、错误 DER 类型、错误 JWK 曲线、缺少必需参数或相冲突的 `key_ops`/`ext` 不会被当作成功导入。

#### Scenario: 标准参数和缓冲区视图不丢失

**WHEN** 页面使用大小写可归一化的算法名称、合法 RSA 指数和模数参数，以及带非零偏移的 TypedArray/DataView 作为明文、签名、label 或导入数据

**THEN** 运算只消费该视图覆盖的字节，返回的算法名和参数为标准形式，结果可与独立浏览器交换；未知算法、未知哈希、非标准曲线及不合法参数按各自标准错误拒绝，不回落为其他算法。

#### Scenario: usage 分配和 extractable 不可绕过

**WHEN** 页面生成仅供签名、解密或派生的不可导出私钥，尝试用公钥签名、无 `deriveKey` usage 的密钥派生目标密钥、无包装 usage 的密钥包装、修改公开属性或克隆后导出

**THEN** 公私钥 usages 按标准分别分配，所有越权操作以适用错误拒绝，私钥材料不出现在导出结果中；合法克隆仍保留相同限制，并能执行原本获准的运算。

#### Scenario: 公私钥可被现有包装算法保护

**WHEN** 页面以适用的既有 AES 包装算法包装可导出的公钥或私钥，以对端解包后执行签名、校验或协商，并修改页面的 JSON 原型方法干扰 JWK 包装

**THEN** 允许的格式和长度产生可互操作的真实密钥，JWK 包装内容不被页面原型修改替换；篡改包装数据、不可导出密钥及不允许的解包用途均失败，原有 AES/HMAC/PBKDF2/HKDF 消费场景继续保持其原有结果。

#### Scenario: 异常输入与执行期限不会变成假成功

**WHEN** 页面提交畸形大密钥、无法满足的参数或触发执行期限的公钥操作，随后由自动化客户端读取结果并开启另一合法页面

**THEN** 调用收到明确拒绝或执行期限错误，不收到伪造密钥或成功结果，不因原生异常破坏其他页面，也不为完成公钥操作而关闭既有执行期限和线程安全保护。
