# AutoContinue 检测引擎改造设计文档

> 日期: 2026-03-14
> 状态: 已批准
> 作者: Claude (AI Assistant)

## 1. 背景与问题

AutoContinue (AC) 当前使用 883 行的虚拟终端模拟器 (`terminal.rs`) 来解析 ANSI 转义序列，
通过统计红色字符数 (>=50) 判断是否出错。该方案存在四个核心稳定性问题：

1. **自动发送不稳定**: PTY writer 注入 prompt 时机不对，150ms 延迟是 workaround
2. **错误检测不准确**: 红色字符计数法误判多（正常红色 UI 元素被误判为错误）
3. **状态识别不准**: 无法区分"正在思考/执行"和"已完成等待输入"
4. **终端交互异常**: ANSI 解析转发导致 UI 渲染问题

## 2. 参考项目

参考 [Happy](https://github.com/slopus/happy) 开源项目的方法：

- **Happy 读取方式**: 监控 Claude Code 的 JSONL 会话文件 (`~/.claude/projects/`)，获取结构化消息
- **Happy 写入方式**: Local mode 使用 stdio inherit，Remote mode 通过 SDK/MCP 发送
- **Codex 交互**: 通过 MCP STDIO protocol，会话文件存储在 `~/.codex/sessions/`
- **关键洞察**: 不解析终端输出，而是读取 CLI 工具自身写入的结构化数据

## 3. 设计方案：PTY 保留 + JSONL Adapter 架构

### 3.1 核心思路

- **保留 PTY**: 子进程在 PTY 中运行（保持交互性），输出直接转发不解析
- **删除 VirtualTerminal**: 不再解析 ANSI 转义序列
- **新增 Detector trait**: 可扩展的状态/错误检测引擎
- **多适配器**: Claude Code (JSONL) / Codex (JSONL) / Generic (文本模式匹配)

### 3.2 模块架构

```
src/
├── main.rs              # 入口 + 主循环（改造）
├── args.rs              # 参数解析（不改动）
├── config.rs            # 配置管理（不改动）
├── monitor.rs           # Ctrl+C / 退出信号（不改动）
├── runner.rs            # PTY 进程管理（大幅简化）
├── detector/            # 新增：状态/错误检测引擎
│   ├── mod.rs           #   Detector trait + CliStatus + create_detector()
│   ├── claude.rs        #   Claude Code adapter (JSONL 会话文件监控)
│   ├── codex.rs         #   Codex adapter (JSONL 会话文件监控)
│   └── generic.rs       #   通用 adapter (输出文本模式匹配)
└── terminal.rs          # 删除
```

### 3.3 Detector Trait

```rust
/// CLI 当前状态
pub enum CliStatus {
    /// 正在运行/思考中，不要干预
    Busy,
    /// 空闲等待输入
    Idle,
    /// 发生了错误，需要重试
    Error { message: String },
    /// 未知状态，fallback 到静默超时检测
    Unknown,
}

/// CLI 工具检测适配器 trait
pub trait Detector: Send {
    /// 初始化检测器
    fn init(&mut self, cli_name: &str, cli_args: &[String]) -> Result<()>;
    /// 处理新的输出数据
    fn feed_output(&mut self, data: &[u8]);
    /// 查询当前状态
    fn status(&self) -> CliStatus;
    /// 重置状态
    fn reset(&mut self);
}
```

### 3.4 适配器选择

根据 `cli_name` 自动选择适配器：

| CLI 名称 | 适配器 | 检测机制 |
|----------|--------|---------|
| `claude` | ClaudeDetector | `~/.claude/projects/{hash}/{id}.jsonl` |
| `codex`  | CodexDetector  | `~/.codex/sessions/*-{id}.jsonl` |
| 其他     | GenericDetector | 输出文本模式匹配 + 静默超时 |

### 3.5 Claude Code 适配器

- 扫描 `~/.claude/projects/` 目录，找到当前工作目录对应的项目目录
- 监控最新的 `.jsonl` 文件变化
- 解析消息类型 (`user` / `assistant` / `system` / `summary`)
- 判断逻辑：
  - 最后一条消息为 `assistant` 类型 + 文件停止增长 → `Idle`
  - 消息内容包含 error 关键信息 → `Error`
  - 文件正在增长 → `Busy`

### 3.6 Codex 适配器

- 扫描 `~/.codex/sessions/` 目录
- 监控 `*-{sessionId}.jsonl` 文件
- 类似 Claude Code 的解析逻辑，适配 Codex 的消息格式

### 3.7 Generic 适配器

- 不依赖任何 JSONL 文件
- 基于原始输出数据做简化的文本模式匹配
- 检测常见错误模式：`Error:`, `error:`, `failed`, `FATAL`, `panic` 等
- 结合静默超时返回 `Idle`（与当前 AC 的 fallback 行为一致）

### 3.8 Runner 简化

**删除**:
- `terminal: SharedTerminal` 字段及所有引用
- `has_error_output()`, `get_error_content()`, `clear_error_state()` 方法
- 输出线程中的 `terminal.lock().process()` 调用

**保留**:
- PTY 创建和进程管理
- 双向 IO 转发
- PTY writer 输入注入
- `last_activity_time` 静默检测
- `inject_sender` channel

**新增**:
- 输出线程将原始数据通过 channel 发送给 Detector

### 3.9 Main 循环改造

```rust
loop {
    if exit_flag { break; }
    if !runner.is_running() { break; }

    let status = detector.lock().status();
    match status {
        CliStatus::Idle => { /* 发送继续 prompt */ }
        CliStatus::Error { message } => { /* 发送重试 prompt */ }
        CliStatus::Unknown => {
            // fallback: 静默超时检测
            if silence_duration >= threshold {
                send_continue_prompt();
            }
        }
        CliStatus::Busy => { /* 不干预 */ }
    }
    thread::sleep(Duration::from_millis(500));
}
```

## 4. 依赖变更

| 依赖 | 操作 | 用途 |
|------|------|------|
| `serde` | 新增 | JSON 序列化/反序列化 |
| `serde_json` | 新增 | JSONL 文件解析 |
| `dirs` | 新增 | 跨平台获取用户主目录 |
| `portable-pty` | 保留 | PTY 管理 |
| `crossterm` | 保留 | 终端操作和输入处理 |
| `clap` | 保留 | 参数解析 |
| `ctrlc` | 保留 | 信号处理 |
| `anyhow` | 保留 | 错误处理 |
| `thiserror` | 保留 | 错误类型定义 |

## 5. 变更行数估算

| 文件 | 操作 | 行数变化 |
|------|------|---------|
| `terminal.rs` | 删除 | -883 |
| `runner.rs` | 简化 | -120, +30 |
| `detector/mod.rs` | 新增 | +80 |
| `detector/claude.rs` | 新增 | +200 |
| `detector/codex.rs` | 新增 | +150 |
| `detector/generic.rs` | 新增 | +100 |
| `main.rs` | 改造 | ±40 |
| **净变化** | | **约 -400** |

## 6. 实施步骤

1. 新增 `detector/` 模块：Trait 定义 + 三个适配器
2. 简化 `runner.rs`：移除 VirtualTerminal 依赖，集成 Detector
3. 改造 `main.rs`：使用 Detector 驱动主循环
4. 删除 `terminal.rs`
5. 更新 `Cargo.toml` 依赖
6. 全面测试
7. 更新文档
