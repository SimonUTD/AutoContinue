# AutoContinue (AC)

**自动继续/重试 CLI 工具的智能包装器**

AutoContinue 是一个 Rust 编写的命令行工具，用于包装 Claude Code、Codex、Gemini、OpenCode 等 AI CLI 工具，实现自动继续和错误重试功能。

## 功能特性

- **自动继续**: 检测 CLI 空闲状态，超时后自动发送继续提示词
- **智能检测**: 通过 Detector 适配器精确检测 CLI 状态（JSONL 会话文件监控 + 文本模式匹配）
- **自动重试**: 检测到错误时自动发送重试提示词
- **完整交互性**: 使用 PTY 保持 CLI 的完整功能，用户可正常操作
- **管道提示词**: 支持执行 shell 命令动态生成提示词（Prompt Pipe）
- **格式提取**: 从管道输出中按标签提取内容，过滤多余文本
- **轮次限制**: 可设置最大自动发送次数
- **跨平台**: 支持 Windows、Linux、macOS
- **动态提示词**: 支持从文件读取提示词，可在运行时修改

---

## 安装

### 🤖 方式一：使用 AI 助手自动安装（推荐）

只需将以下内容复制给任意 AI 助手（如 Claude Code、Cursor、Copilot 等）：

```
Install and configure AutoContinue by following the instructions here:
https://raw.githubusercontent.com/MoYeRanQianZhi/AutoContinue/master/install-by-agent.md
```

AI 助手会自动完成安装过程。

---

### 📦 方式二：直接下载

从 [GitHub Releases](https://github.com/MoYeRanQianZhi/AutoContinue/releases/latest) 下载对应平台的预编译版本：

| 平台 | 架构 | 下载文件 |
|------|------|----------|
| Windows | x64 | `ac-vX.X.X-x86_64-pc-windows-msvc.zip` |
| Windows | x86 (32位) | `ac-vX.X.X-i686-pc-windows-msvc.zip` |
| Linux | x64 | `ac-vX.X.X-x86_64-unknown-linux-gnu.tar.gz` |
| Linux | x86 (32位) | `ac-vX.X.X-i686-unknown-linux-gnu.tar.gz` |
| macOS | Intel (x64) | `ac-vX.X.X-x86_64-apple-darwin.tar.gz` |
| macOS | Apple Silicon (ARM64) | `ac-vX.X.X-aarch64-apple-darwin.tar.gz` |

**安装步骤：**

1. 下载对应平台的压缩包
2. 解压到任意目录
3. 将 `ac` (或 `ac.exe`) 所在目录添加到系统 PATH

**Windows 快速安装：**
```powershell
# 解压后复制到 cargo bin 目录（如果已安装 Rust）
Copy-Item ac.exe $env:USERPROFILE\.cargo\bin\
```

**Linux/macOS 快速安装：**
```bash
# 解压后复制到系统目录
sudo cp ac /usr/local/bin/
chmod +x /usr/local/bin/ac
```

---

### 🔧 方式三：从源码编译

#### 前置要求

- Git
- [Rust](https://rustup.rs/) 1.85+（edition 2024）

#### 从源码编译

```bash
# 克隆仓库
git clone https://github.com/MoYeRanQianZhi/AutoContinue.git
cd AutoContinue

# 编译发布版本
cargo build --release

# 二进制文件位于 target/release/ac (Linux/macOS) 或 target/release/ac.exe (Windows)
```

#### 添加到系统 PATH

**Windows (PowerShell):**
```powershell
Copy-Item target\release\ac.exe $env:USERPROFILE\.cargo\bin\
```

**Linux/macOS:**
```bash
cp target/release/ac ~/.cargo/bin/
# 或
sudo cp target/release/ac /usr/local/bin/
```

#### 使用 Cargo 安装（可选）

```bash
cargo install --path .
```

#### 验证安装

```bash
ac --version
```

---

## 使用方法

### 基本语法

```bash
ac <CLI程序> [CLI参数...] [AC参数...]
```

AC 参数和 CLI 参数可以混合使用，顺序不限。

### 快速开始

```bash
# 最简单的使用方式（使用默认提示词）
ac claude

# 带 CLI 参数
ac claude --resume

# 自定义继续提示词
ac claude --resume -cp "请继续完成任务"

# 自定义继续和重试提示词
ac claude --resume -cp "继续" -rp "重试上一步"

# 调整等待时间
ac claude -st 30 -sth 60

# 限制最大轮次
ac claude -cp "继续" -l 10
```

### AC 参数说明

| 参数 | 长参数 | 说明 | 默认值 |
|------|--------|------|--------|
| `-cp` | `--continue-prompt` | 继续提示词 | "继续" |
| `-cpf` | `--continue-prompt-file` | 继续提示词文件（启动时读取一次） | - |
| `-cpio` | `--continue-prompt-io` | 继续提示词IO文件（每次使用时重新读取） | - |
| `-cpp` | `--continue-prompt-pipe` | 继续提示词管道命令（每次执行命令获取） | - |
| `-rp` | `--retry-prompt` | 重试提示词 | "重试" |
| `-rpf` | `--retry-prompt-file` | 重试提示词文件（启动时读取一次） | - |
| `-rpio` | `--retry-prompt-io` | 重试提示词IO文件（每次使用时重新读取） | - |
| `-rpp` | `--retry-prompt-pipe` | 重试提示词管道命令（每次执行命令获取） | - |
| | `--cformat <前缀> <后缀>` | 继续管道输出格式提取（取最后一组匹配） | - |
| | `--rformat <前缀> <后缀>` | 重试管道输出格式提取（取最后一组匹配） | - |
| `-st` | `--sleep-time` | 额外等待时间（秒） | 15 |
| `-sth` | `--silence-threshold` | 静默阈值（秒） | 30 |
| `-l` | `--limit` | 最大自动发送轮次（-1 为无限制） | -1 |
| `-h` | `--help` | 显示帮助信息 | - |
| `-v` | `--version` | 显示版本信息 | - |

### 提示词模式

AC 支持五种提示词来源模式（互斥，优先级从高到低）：

| 模式 | 参数 | 说明 |
|------|------|------|
| **Pipe** | `-cpp` / `-rpp` | 每次执行 shell 命令，用 stdout 作为提示词 |
| **IO** | `-cpio` / `-rpio` | 每次重新读取文件，可运行时修改 |
| **File** | `-cpf` / `-rpf` | 启动时读取一次文件 |
| **Direct** | `-cp` / `-rp` | 直接指定字符串 |
| **Default** | 无参数 | 继续="继续"，重试="重试" |

### 时间参数说明

- **静默阈值 (`-sth`)**: CLI 无输入/输出超过此时间后开始计时
- **额外等待时间 (`-st`)**: 给用户自主回复的缓冲时间
- **总等待时间** = 静默阈值 + 额外等待时间

例如：默认设置下，CLI 静默 45 秒（30+15）后才会自动发送提示词。

### 使用示例

#### 示例 1：基础使用

```bash
ac claude --resume -cp "继续迭代，不断优化代码"
```

#### 示例 2：使用文件提示词

```bash
# 创建提示词文件
echo "请继续完成上一步的任务，保持代码风格一致" > continue.txt

# 使用文件提示词
ac claude --resume -cpf continue.txt
```

#### 示例 3：动态提示词（IO模式）

```bash
# 使用 IO 模式，可以在运行时修改提示词
ac claude --resume -cpio continue.txt

# 在另一个终端修改提示词，下次触发时会使用新内容
echo "新的提示词内容" > continue.txt
```

#### 示例 4：管道提示词（Pipe模式）

```bash
# 使用 echo 作为管道命令
ac claude -cpp "echo 继续开发"

# 使用 cat 读取文件（等同于 IO 模式，但更灵活）
ac claude -cpp "cat prompt.txt"

# 配合格式提取，只取标签内的内容
ac claude -cpp "echo '分析完成 <continue>请继续优化性能</continue> 结束'" --cformat "<continue>" "</continue>"
```

#### 示例 5：配合其他 AI CLI

```bash
# Codex
ac codex -cp "continue"

# OpenCode
ac opencode -cp "继续"

# Gemini
ac gemini -cp "请继续"
```

#### 示例 6：限制轮次

```bash
# 最多自动发送 5 次后停止
ac claude -cp "继续" -l 5
```

---

## 最佳实践：多Agent协同 Prompt Pipe

Prompt Pipe 的典型场景是让一个 AI CLI 分析当前项目状态，动态生成下一步的继续提示词。

以下是一个实际示例——使用 Codex 作为"评审Agent"，为主力 CLI 生成每一轮的继续 prompt：

```bash
ac claude -cpp "codex exec \"并行多Agent检查当前项目中新增部分并评价给出下一步继续建议，建议内容是给codex的一句话纯文本prompt，prompt需格式化包裹于<continue>...</continue>中以便程序识别，如有大段内容需传递，可写入文件中并在prompt中给出指示和文件地址。\" --dangerously-bypass-approvals-and-sandbox" --cformat "<continue>" "</continue>"
```

**工作流程：**

1. AC 检测到 Claude 空闲
2. 执行 `-cpp` 管道命令：Codex 分析项目新增部分，给出建议
3. Codex 输出中包含 `<continue>具体建议prompt</continue>`
4. AC 通过 `--cformat` 提取标签内的内容
5. 提取到的 prompt 被自动发送给 Claude，Claude 继续工作
6. 循环往复，形成 Agent 间的自动协作

**关键点：**

- 管道命令每轮都重新执行，因此每次获取的建议都是基于最新项目状态
- `--cformat` 确保只取标签内容，避免 Codex 输出中的分析文本污染 prompt
- 如需传递大段上下文，评审Agent可将内容写入文件并在 prompt 中引用文件路径

---

## 工作原理

1. **启动**: AC 使用伪终端（PTY）启动目标 CLI，保持完整的终端交互能力
2. **监控**: 通过 Detector 适配器实时监控 CLI 状态
3. **状态检测**:
   - **Claude Code**: 监控 `~/.claude/projects/` 下的 JSONL 会话文件
   - **Codex**: 监控 `~/.codex/sessions/` 下的 JSONL 会话文件
   - **其他工具**: 输出文本模式匹配 + 静默超时 fallback
4. **自动响应**:
   - 检测到空闲 (`Idle`) → 发送继续提示词
   - 检测到错误 (`Error`) → 发送重试提示词
   - 状态未知 (`Unknown`) → fallback 到静默超时检测
   - 正在工作 (`Busy`) → 不干预
5. **用户优先**: 任何用户输入都会重置计时器，不会打断用户操作

## 退出方式

- 按 `Ctrl+C` 优雅退出
- CLI 进程自行退出时 AC 也会退出
- 使用 `-l` 限制轮次，达到上限后自动退出

## 常见问题

### Q: 为什么我的输入没有响应？

A: 确保 CLI 程序支持标准输入。某些 CLI 可能需要特定参数才能接受输入。

### Q: 如何在 Windows 上使用？

A: AC 完全支持 Windows，使用 ConPTY 技术。直接运行即可。

### Q: 自动发送的提示词会打断我的输入吗？

A: 不会。任何用户输入/输出都会重置静默计时器，AC 只在真正静默时才会发送提示词。

### Q: 支持中文提示词吗？

A: 完全支持中文和其他 Unicode 字符。

### Q: Pipe 模式的命令执行超时怎么办？

A: 当前管道命令没有内置超时限制，命令会一直执行直到完成。如果需要超时控制，可以在命令中使用 `timeout` 等工具。

## 开发

### 项目结构

```
AutoContinue/
├── Cargo.toml          # 项目配置
├── README.md           # 本文档
├── CLAUDE.md           # Claude Code 项目说明
├── docs/
│   └── development/    # 开发文档
├── src/
│   ├── main.rs         # 程序入口和主循环
│   ├── args.rs         # 命令行参数解析
│   ├── config.rs       # 配置管理、提示词加载、管道执行
│   ├── runner.rs       # CLI 运行器（PTY管理、IO转发）
│   ├── monitor.rs      # 状态监控、Ctrl+C处理
│   └── detector/       # 状态/错误检测引擎
│       ├── mod.rs      #   Detector trait + CliStatus 枚举
│       ├── claude.rs   #   Claude Code 适配器（JSONL 会话文件监控）
│       ├── codex.rs    #   Codex 适配器（JSONL 会话文件监控）
│       └── generic.rs  #   通用适配器（输出文本模式匹配）
```

### 运行测试

```bash
cargo test
```

### 构建发布版本

```bash
cargo build --release
```

## 许可证

MIT License

## 作者

MoYeRanQianZhi <MoYeRanQianZhi@gmail.com>
