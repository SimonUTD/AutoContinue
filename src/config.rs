//! # 配置管理模块 (config.rs)
//!
//! 该模块负责管理AutoContinue的配置，包括：
//! - 加载提示词（从命令行参数或文件）
//! - 支持IO模式：每次使用时动态读取文件
//! - 支持Pipe模式：每次使用时执行命令获取提示词
//! - 支持Format提取：从管道输出中按前缀后缀提取内容
//! - 管理默认值
//! - 配置验证
//!
//! ## 默认值
//! - `continue_prompt`: "继续"
//! - `retry_prompt`: "重试"
//! - `sleep_time`: 15秒
//!
//! ## 提示词模式优先级
//! pipe > io > file > direct > default
//!
//! ## Pipe模式
//! 使用 -cpp 或 -rpp 参数时，提示词会在每次使用时执行指定命令，
//! 将命令的 stdout 输出作为提示词内容。配合 --cformat / --rformat
//! 可从输出中提取特定标签包裹的内容。

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

/// 默认的继续提示词
const DEFAULT_CONTINUE_PROMPT: &str = "继续";

/// 默认的重试提示词
const DEFAULT_RETRY_PROMPT: &str = "重试";

/// 默认的等待时间（秒）
pub const DEFAULT_SLEEP_TIME: u64 = 15;

/// 默认的静默阈值（秒）
pub const DEFAULT_SILENCE_THRESHOLD: u64 = 30;

/// 默认的最大轮次限制（-1 表示无限制）
pub const DEFAULT_LIMIT: i64 = -1;

/// AutoContinue配置结构体
///
/// 该结构体包含所有运行时需要的配置信息。
/// 配置可以从命令行参数或文件加载。
/// 支持多种提示词模式：直接、文件、IO、管道（Pipe）。
#[derive(Debug, Clone)]
pub struct Config {
    /// 要运行的CLI程序名称
    pub cli: String,

    /// 传递给CLI程序的参数
    pub cli_args: Vec<String>,

    /// 继续的提示词（静态模式）
    /// 当CLI正常结束时发送此提示词
    /// 如果设置了 io/pipe 模式，此字段为空
    pub continue_prompt: String,

    /// 继续提示词的IO文件路径（动态模式）
    /// 如果设置了此字段，每次使用时会重新读取文件
    pub continue_prompt_io: Option<String>,

    /// 继续提示词的管道命令（Pipe模式）
    /// 如果设置了此字段，每次使用时执行命令获取提示词
    pub continue_prompt_pipe: Option<String>,

    /// 重试的提示词（静态模式）
    /// 当CLI出错时发送此提示词
    /// 如果设置了 io/pipe 模式，此字段为空
    pub retry_prompt: String,

    /// 重试提示词的IO文件路径（动态模式）
    /// 如果设置了此字段，每次使用时会重新读取文件
    pub retry_prompt_io: Option<String>,

    /// 重试提示词的管道命令（Pipe模式）
    /// 如果设置了此字段，每次使用时执行命令获取提示词
    pub retry_prompt_pipe: Option<String>,

    /// 继续管道输出格式提取标签 [前缀, 后缀]
    /// 仅在 pipe 模式下生效，从输出中提取最后一组匹配内容
    pub cformat: Option<(String, String)>,

    /// 重试管道输出格式提取标签 [前缀, 后缀]
    /// 仅在 pipe 模式下生效，从输出中提取最后一组匹配内容
    pub rformat: Option<(String, String)>,

    /// 等待时间（秒）
    /// 在自动发送提示词之前等待的时间
    /// 给用户自主回复的机会
    pub sleep_time: u64,

    /// 静默阈值（秒）
    /// CLI无输入/输出超过此时间后开始计算等待时间
    /// 总等待时间 = 静默阈值 + 等待时间
    pub silence_threshold: u64,

    /// 最大自动发送轮次限制
    /// -1 表示无限制，正数表示最大发送次数
    /// 达到限制后程序将停止自动发送并退出
    pub limit: i64,
}

impl Config {
    /// 从命令行参数创建配置
    ///
    /// # 参数
    /// - `args`: 解析后的命令行参数
    ///
    /// # 返回值
    /// 成功返回Config实例，失败返回错误
    ///
    /// # 错误
    /// - 当指定的提示词文件不存在或无法读取时返回错误
    ///
    /// # 示例
    /// ```
    /// let args = parse_args();
    /// let config = Config::from_args(&args)?;
    /// ```
    pub fn from_args(args: &crate::args::Args) -> Result<Self> {
        // 处理继续提示词：支持四种模式（优先级：pipe > io > file > direct/default）
        let (continue_prompt, continue_prompt_io, continue_prompt_pipe) =
            if let Some(ref pipe_cmd) = args.continue_prompt_pipe {
                // Pipe模式：存储命令，每次使用时执行
                (String::new(), None, Some(pipe_cmd.clone()))
            } else if let Some(ref io_path) = args.continue_prompt_io {
                // IO模式：验证文件存在，存储路径
                if !Path::new(io_path).exists() {
                    anyhow::bail!("继续提示词IO文件不存在: {}", io_path);
                }
                (String::new(), Some(io_path.clone()), None)
            } else if let Some(ref prompt) = args.continue_prompt {
                // 从命令行参数读取继续提示词
                (prompt.clone(), None, None)
            } else if let Some(ref file_path) = args.continue_prompt_file {
                // 从文件读取继续提示词（一次性）
                let prompt = load_prompt_from_file(file_path)
                    .with_context(|| format!("无法从文件加载继续提示词: {}", file_path))?;
                (prompt, None, None)
            } else {
                // 使用默认继续提示词
                (DEFAULT_CONTINUE_PROMPT.to_string(), None, None)
            };

        // 处理重试提示词：同样支持四种模式
        let (retry_prompt, retry_prompt_io, retry_prompt_pipe) =
            if let Some(ref pipe_cmd) = args.retry_prompt_pipe {
                // Pipe模式：存储命令，每次使用时执行
                (String::new(), None, Some(pipe_cmd.clone()))
            } else if let Some(ref io_path) = args.retry_prompt_io {
                // IO模式：验证文件存在，存储路径
                if !Path::new(io_path).exists() {
                    anyhow::bail!("重试提示词IO文件不存在: {}", io_path);
                }
                (String::new(), Some(io_path.clone()), None)
            } else if let Some(ref prompt) = args.retry_prompt {
                // 从命令行参数读取重试提示词
                (prompt.clone(), None, None)
            } else if let Some(ref file_path) = args.retry_prompt_file {
                // 从文件读取重试提示词（一次性）
                let prompt = load_prompt_from_file(file_path)
                    .with_context(|| format!("无法从文件加载重试提示词: {}", file_path))?;
                (prompt, None, None)
            } else {
                // 使用默认重试提示词
                (DEFAULT_RETRY_PROMPT.to_string(), None, None)
            };

        // 处理格式提取参数：将 Vec<String> 转换为 (prefix, suffix) 元组
        let cformat = args.cformat.as_ref().and_then(|v| {
            if v.len() == 2 {
                Some((v[0].clone(), v[1].clone()))
            } else {
                None
            }
        });
        let rformat = args.rformat.as_ref().and_then(|v| {
            if v.len() == 2 {
                Some((v[0].clone(), v[1].clone()))
            } else {
                None
            }
        });

        // 如果指定了 --cformat 但没有 -cpp，发出警告
        if cformat.is_some() && continue_prompt_pipe.is_none() {
            eprintln!("[AC] 警告: --cformat 仅在 -cpp (管道模式) 下生效，当前未使用管道模式");
        }
        // 如果指定了 --rformat 但没有 -rpp，发出警告
        if rformat.is_some() && retry_prompt_pipe.is_none() {
            eprintln!("[AC] 警告: --rformat 仅在 -rpp (管道模式) 下生效，当前未使用管道模式");
        }

        // 将CLI参数从OsString转换为String
        let cli_args: Vec<String> = args
            .cli_args
            .iter()
            .filter_map(|s| s.to_str().map(|s| s.to_string()))
            .collect();

        Ok(Config {
            cli: args.cli.clone(),
            cli_args,
            continue_prompt,
            continue_prompt_io,
            continue_prompt_pipe,
            retry_prompt,
            retry_prompt_io,
            retry_prompt_pipe,
            cformat,
            rformat,
            sleep_time: args.sleep_time,
            silence_threshold: args.silence_threshold,
            limit: args.limit,
        })
    }

    /// 获取当前的继续提示词
    ///
    /// 优先级：pipe > io > static
    /// - Pipe模式：执行命令，可选 format 提取
    /// - IO模式：重新读取文件
    /// - 静态模式：返回缓存内容
    ///
    /// # 返回值
    /// 成功返回提示词内容，失败返回错误
    pub fn get_continue_prompt(&self) -> Result<String> {
        if let Some(ref pipe_cmd) = self.continue_prompt_pipe {
            // Pipe模式：执行命令获取输出
            let output = execute_pipe_command(pipe_cmd)
                .with_context(|| format!("执行继续提示词管道命令失败: {}", pipe_cmd))?;
            // 如果配置了格式提取，从输出中提取匹配内容
            if let Some((ref prefix, ref suffix)) = self.cformat {
                match extract_format(&output, prefix, suffix) {
                    Some(extracted) => Ok(extracted),
                    None => {
                        eprintln!("[AC] 警告: 管道输出中未找到格式标签 {}...{}，使用完整输出", prefix, suffix);
                        Ok(output)
                    }
                }
            } else {
                Ok(output)
            }
        } else if let Some(ref io_path) = self.continue_prompt_io {
            // IO模式：每次重新读取文件
            load_prompt_from_file(io_path)
                .with_context(|| format!("无法读取继续提示词IO文件: {}", io_path))
        } else {
            // 静态模式：返回存储的提示词
            Ok(self.continue_prompt.clone())
        }
    }

    /// 获取当前的重试提示词
    ///
    /// 优先级：pipe > io > static
    /// - Pipe模式：执行命令，可选 format 提取
    /// - IO模式：重新读取文件
    /// - 静态模式：返回缓存内容
    ///
    /// # 返回值
    /// 成功返回提示词内容，失败返回错误
    pub fn get_retry_prompt(&self) -> Result<String> {
        if let Some(ref pipe_cmd) = self.retry_prompt_pipe {
            // Pipe模式：执行命令获取输出
            let output = execute_pipe_command(pipe_cmd)
                .with_context(|| format!("执行重试提示词管道命令失败: {}", pipe_cmd))?;
            // 如果配置了格式提取，从输出中提取匹配内容
            if let Some((ref prefix, ref suffix)) = self.rformat {
                match extract_format(&output, prefix, suffix) {
                    Some(extracted) => Ok(extracted),
                    None => {
                        eprintln!("[AC] 警告: 管道输出中未找到格式标签 {}...{}，使用完整输出", prefix, suffix);
                        Ok(output)
                    }
                }
            } else {
                Ok(output)
            }
        } else if let Some(ref io_path) = self.retry_prompt_io {
            // IO模式：每次重新读取文件
            load_prompt_from_file(io_path)
                .with_context(|| format!("无法读取重试提示词IO文件: {}", io_path))
        } else {
            // 静态模式：返回存储的提示词
            Ok(self.retry_prompt.clone())
        }
    }

    /// 检查是否使用继续提示词IO模式
    pub fn is_continue_prompt_io(&self) -> bool {
        self.continue_prompt_io.is_some()
    }

    /// 检查是否使用继续提示词Pipe模式
    pub fn is_continue_prompt_pipe(&self) -> bool {
        self.continue_prompt_pipe.is_some()
    }

    /// 检查是否使用重试提示词IO模式
    #[allow(dead_code)]
    pub fn is_retry_prompt_io(&self) -> bool {
        self.retry_prompt_io.is_some()
    }

    /// 检查是否使用重试提示词Pipe模式
    pub fn is_retry_prompt_pipe(&self) -> bool {
        self.retry_prompt_pipe.is_some()
    }

    /// 获取完整的CLI命令（包含程序名和参数）
    ///
    /// # 返回值
    /// 返回CLI程序名和所有参数组成的向量
    #[allow(dead_code)]
    pub fn get_full_command(&self) -> Vec<String> {
        let mut cmd = vec![self.cli.clone()];
        cmd.extend(self.cli_args.clone());
        cmd
    }
}

/// 从文件加载提示词
///
/// # 参数
/// - `path`: 提示词文件路径
///
/// # 返回值
/// 成功返回文件内容（去除首尾空白，标准化换行符），失败返回错误
///
/// # 错误
/// - 文件不存在
/// - 文件无法读取
///
/// # 注意
/// 该函数会标准化换行符：
/// - Windows换行符 `\r\n` 会被转换为 `\n`
/// - 单独的 `\r` 也会被转换为 `\n`
/// 这确保了跨平台的一致行为
fn load_prompt_from_file<P: AsRef<Path>>(path: P) -> Result<String> {
    let content = fs::read_to_string(path.as_ref()).with_context(|| "读取文件失败")?;

    // 标准化换行符：将 \r\n 和单独的 \r 都转换为 \n
    // 这样可以避免在PTY中换行被重复处理
    let normalized = content
        .replace("\r\n", "\n") // Windows换行符 -> Unix换行符
        .replace("\r", "\n"); // 旧Mac换行符 -> Unix换行符

    // 去除首尾空白字符
    Ok(normalized.trim().to_string())
}

/// 执行管道命令并返回 stdout 输出
///
/// 跨平台实现：
/// - Windows: 使用 cmd /C 执行命令
/// - Unix: 使用 sh -c 执行命令
///
/// # 参数
/// - `command`: 要执行的 shell 命令字符串
///
/// # 返回值
/// 成功返回命令的 stdout 输出（去除首尾空白），失败返回错误
///
/// # 错误
/// - 命令执行失败（找不到命令等）
/// - 命令返回非零退出码
pub fn execute_pipe_command(command: &str) -> Result<String> {
    // 根据平台选择 shell
    #[cfg(windows)]
    let output = Command::new("cmd")
        .args(["/C", command])
        .output()
        .with_context(|| format!("无法执行命令: {}", command))?;

    #[cfg(not(windows))]
    let output = Command::new("sh")
        .args(["-c", command])
        .output()
        .with_context(|| format!("无法执行命令: {}", command))?;

    // 检查命令退出状态
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = output.status.code().unwrap_or(-1);
        anyhow::bail!(
            "管道命令退出码 {}，stderr: {}",
            code,
            stderr.trim()
        );
    }

    // 将 stdout 转换为字符串
    let stdout = String::from_utf8(output.stdout)
        .with_context(|| "管道命令输出不是有效的 UTF-8")?;

    Ok(stdout.trim().to_string())
}

/// 从文本中提取最后一组前缀后缀包裹的内容
///
/// 在输出中查找最后一个 `prefix...suffix` 模式，返回中间内容。
/// 用于从管道命令输出中提取特定标签包裹的提示词，过滤多余文本。
///
/// # 参数
/// - `output`: 完整输出文本
/// - `prefix`: 前缀标签（如 `<continue>`）
/// - `suffix`: 后缀标签（如 `</continue>`）
///
/// # 返回值
/// 找到匹配返回 Some(内容)（去除首尾空白），未找到返回 None
///
/// # 示例
/// ```
/// let output = "some text <continue>real prompt</continue> more text";
/// let result = extract_format(output, "<continue>", "</continue>");
/// assert_eq!(result, Some("real prompt".to_string()));
/// ```
pub fn extract_format(output: &str, prefix: &str, suffix: &str) -> Option<String> {
    // 从后往前查找最后一个 prefix 的位置
    let prefix_pos = output.rfind(prefix)?;

    // 从 prefix 之后开始查找 suffix
    let content_start = prefix_pos + prefix.len();
    let remaining = &output[content_start..];
    let suffix_pos = remaining.find(suffix)?;

    // 提取并返回中间内容
    let content = &remaining[..suffix_pos];
    Some(content.trim().to_string())
}

impl Default for Config {
    /// 创建默认配置
    ///
    /// 默认配置使用空CLI名称和默认提示词（静态模式）
    fn default() -> Self {
        Config {
            cli: String::new(),
            cli_args: Vec::new(),
            continue_prompt: DEFAULT_CONTINUE_PROMPT.to_string(),
            continue_prompt_io: None,
            continue_prompt_pipe: None,
            retry_prompt: DEFAULT_RETRY_PROMPT.to_string(),
            retry_prompt_io: None,
            retry_prompt_pipe: None,
            cformat: None,
            rformat: None,
            sleep_time: DEFAULT_SLEEP_TIME,
            silence_threshold: DEFAULT_SILENCE_THRESHOLD,
            limit: DEFAULT_LIMIT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// 测试从文件加载提示词
    #[test]
    fn test_load_prompt_from_file() -> Result<()> {
        // 创建临时文件
        let mut file = NamedTempFile::new()?;
        writeln!(file, "  测试提示词  ")?;

        // 加载并验证
        let prompt = load_prompt_from_file(file.path())?;
        assert_eq!(prompt, "测试提示词");

        Ok(())
    }

    /// 测试换行符标准化（Windows格式 \r\n）
    #[test]
    fn test_normalize_crlf() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        // 写入Windows格式换行符
        file.write_all(b"Line1\r\nLine2\r\nLine3")?;

        let prompt = load_prompt_from_file(file.path())?;
        // 应该只有 \n，没有 \r
        assert!(!prompt.contains('\r'));
        assert_eq!(prompt, "Line1\nLine2\nLine3");

        Ok(())
    }

    /// 测试换行符标准化（单独的 \r）
    #[test]
    fn test_normalize_cr() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        // 写入旧Mac格式换行符
        file.write_all(b"Line1\rLine2\rLine3")?;

        let prompt = load_prompt_from_file(file.path())?;
        // \r 应该被转换为 \n
        assert!(!prompt.contains('\r'));
        assert_eq!(prompt, "Line1\nLine2\nLine3");

        Ok(())
    }

    /// 测试默认配置
    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.continue_prompt, DEFAULT_CONTINUE_PROMPT);
        assert_eq!(config.retry_prompt, DEFAULT_RETRY_PROMPT);
        assert_eq!(config.sleep_time, DEFAULT_SLEEP_TIME);
        assert_eq!(config.silence_threshold, DEFAULT_SILENCE_THRESHOLD);
        assert_eq!(config.limit, DEFAULT_LIMIT);
        assert!(config.continue_prompt_pipe.is_none());
        assert!(config.retry_prompt_pipe.is_none());
        assert!(config.cformat.is_none());
        assert!(config.rformat.is_none());
    }

    /// 测试 extract_format 基本提取
    #[test]
    fn test_extract_format_basic() {
        let output = "some text <continue>real prompt</continue> more text";
        let result = extract_format(output, "<continue>", "</continue>");
        assert_eq!(result, Some("real prompt".to_string()));
    }

    /// 测试 extract_format 多组匹配取最后一组
    #[test]
    fn test_extract_format_last_match() {
        let output = "<c>first</c> middle <c>second</c> end";
        let result = extract_format(output, "<c>", "</c>");
        assert_eq!(result, Some("second".to_string()));
    }

    /// 测试 extract_format 无匹配返回 None
    #[test]
    fn test_extract_format_no_match() {
        let output = "no tags here";
        let result = extract_format(output, "<c>", "</c>");
        assert_eq!(result, None);
    }

    /// 测试 extract_format 只有前缀没有后缀
    #[test]
    fn test_extract_format_no_suffix() {
        let output = "text <c>content without closing";
        let result = extract_format(output, "<c>", "</c>");
        assert_eq!(result, None);
    }

    /// 测试 extract_format 多行内容
    #[test]
    fn test_extract_format_multiline() {
        let output = "header\n<continue>\nline1\nline2\n</continue>\nfooter";
        let result = extract_format(output, "<continue>", "</continue>");
        assert_eq!(result, Some("line1\nline2".to_string()));
    }

    /// 测试 execute_pipe_command 基本执行
    #[test]
    fn test_execute_pipe_command_echo() {
        let result = execute_pipe_command("echo hello").unwrap();
        assert_eq!(result, "hello");
    }

    /// 测试 execute_pipe_command 命令失败
    #[test]
    fn test_execute_pipe_command_failure() {
        // 执行一个必定失败的命令
        let result = execute_pipe_command("exit 1");
        assert!(result.is_err());
    }
}
