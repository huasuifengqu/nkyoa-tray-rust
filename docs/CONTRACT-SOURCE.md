# 桌面通知协议 v1 快照来源

- 客户端侧冻结锚点：`crates/tray-core/src/protocol/mod.rs`（有基线测试）。
- 服务端导出的契约快照：`contracts/nkyoa-desktop-v1.openapi.json`，由服务端仓的
  `npm run contracts:desktop-export` 从 `contracts/NKY_OA.json` 生成。
- 导入清单：`contracts/source-manifest.json`（协议版本 1、SHA-256、导出器版本、
  来源服务器提交号与干净/未提交状态）。
- 本轮快照含注册能力位 `previewCapable`（服务端提交 `85410f3`、`serverDirty=true`）：
  客户端注册固定声明 `true`；流程标题与流转意见只用于内存展示与系统通知，
  待处理页落盘前统一替换为通用提示。
- 黄金样本：`contracts/golden/*.json`，来自服务端提交 `5465d83` 的 HTTP 管线与临时
  PostgreSQL 数据库；样本中的 ID、时间、请求追踪号和令牌已替换为固定测试值。

本仓不引用服务端源码或相邻目录；`crates/tray-core/tests/api_contract.rs` 每次运行都会：

1. 重新计算 OpenAPI 的 SHA-256 并与来源清单比对；
2. 校验 5 个路径与配对/设备两层安全方案；
3. 按顺序重放注册、设备、拉取、确认、历史过期、重同步、撤销样本；
4. 断言请求方法、路由、查询串、CSRF 头、Authorization 前缀与请求体；
5. 对拉取页做 16 类非法变异，确认游标无法推进。

v1 实际确认回执字段为 `ackedSequence`，客户端发送的请求字段为
`receivedThroughSequence`。
