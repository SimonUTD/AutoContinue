# OpenCode Detector Design

## 目标

在保留 `ClaudeDetector` 与 `CodexDetector` 的前提下，新增 `OpenCodeDetector`，让 `ac opencode` 走结构化状态检测，而不是 `GenericDetector` 的纯文本启发式。

## 现状与问题

- 当前 `create_detector("opencode")` 会返回 `GenericDetector`。
- `GenericDetector` 仅基于 PTY 输出关键词判断，精度依赖终端文本，无法利用 OpenCode 的结构化会话数据。
- README 已宣称支持 OpenCode，但实现层未提供 OpenCode 专用 detector。

## OpenCode 数据面

OpenCode 的会话数据位于数据目录下（默认 `~/.local/share/opencode`）：

- `storage/message/ses_*/msg_*.json`：消息元数据（包含 `role`、`error`）
- `storage/part/msg_*/prt_*.json`：消息片段（工具调用、文本等）

本次实现优先使用 `storage/message` 完成状态判定，避免过度复杂化。

## 检测策略

- `Error`：最新 assistant message 含 `error` 字段
- `Busy`：最近文件仍在变化（稳定窗口内）
- `Idle`：文件稳定 + 超过静默阈值 + 最新消息角色为 assistant 且无 error
- `Unknown`：找不到会话消息文件或无法解析

## 目录解析策略

- 优先使用环境变量 `OPENCODE_DATA_DIR`
- 否则使用 `XDG_DATA_HOME/opencode`
- 否则回退到 `~/.local/share/opencode`

## 代码改动

1. 新增 `src/detector/opencode.rs`
2. 更新 `src/detector/mod.rs`：
   - 注册 `pub mod opencode;`
   - `create_detector()` 增加 `opencode -> OpenCodeDetector`
   - 工厂测试改为断言 `opencode` 返回 `OpenCodeDetector`
3. 更新 README 中检测机制说明，补充 OpenCode 专用 detector。

## 测试策略

- 单元测试覆盖：
  - detector 名称
  - 无会话文件时返回 `Unknown`
  - 解析 `error` 字段返回 `Error`
  - assistant 消息在稳定后返回 `Idle`

## 非目标

- 不改动 Claude/Codex detector 的现有逻辑
- 不引入静默 fallback/模拟成功路径
- 不改动 CLI 参数协议
