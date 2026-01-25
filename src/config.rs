//! # 配置管理模块 (config.rs)
//!
//! 该模块负责管理AutoContinue的配置，包括：
//! - 加载提示词（从命令行参数或文件）
//! - 管理默认值
//! - 配置验证
//!
//! ## 默认值
//! - `continue_prompt`: "继续"
//! - `retry_prompt`: "重试"
//! - `sleep_time`: 15秒

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// 默认的继续提示词
const DEFAULT_CONTINUE_PROMPT: &str = "继续";

/// 默认的重试提示词
const DEFAULT_RETRY_PROMPT: &str = "重试";

/// 默认的等待时间（秒）
pub const DEFAULT_SLEEP_TIME: u64 = 15;

/// 默认的静默阈值（秒）
pub const DEFAULT_SILENCE_THRESHOLD: u64 = 30;

/// AutoContinue配置结构体
///
/// 该结构体包含所有运行时需要的配置信息。
/// 配置可以从命令行参数或文件加载。
#[derive(Debug, Clone)]
pub struct Config {
    /// 要运行的CLI程序名称
    pub cli: String,

    /// 传递给CLI程序的参数
    pub cli_args: Vec<String>,

    /// 继续的提示词
    /// 当CLI正常结束时发送此提示词
    pub continue_prompt: String,

    /// 重试的提示词
    /// 当CLI出错时发送此提示词
    pub retry_prompt: String,

    /// 等待时间（秒）
    /// 在自动发送提示词之前等待的时间
    /// 给用户自主回复的机会
    pub sleep_time: u64,

    /// 静默阈值（秒）
    /// CLI无输入/输出超过此时间后开始计算等待时间
    /// 总等待时间 = 静默阈值 + 等待时间
    pub silence_threshold: u64,
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
        // 加载继续提示词：优先从参数读取，否则从文件读取，最后使用默认值
        let continue_prompt = if let Some(ref prompt) = args.continue_prompt {
            // 从命令行参数读取继续提示词
            prompt.clone()
        } else if let Some(ref file_path) = args.continue_prompt_file {
            // 从文件读取继续提示词
            load_prompt_from_file(file_path)
                .with_context(|| format!("无法从文件加载继续提示词: {}", file_path))?
        } else {
            // 使用默认继续提示词
            DEFAULT_CONTINUE_PROMPT.to_string()
        };

        // 加载重试提示词：优先从参数读取，否则从文件读取，最后使用默认值
        let retry_prompt = if let Some(ref prompt) = args.retry_prompt {
            // 从命令行参数读取重试提示词
            prompt.clone()
        } else if let Some(ref file_path) = args.retry_prompt_file {
            // 从文件读取重试提示词
            load_prompt_from_file(file_path)
                .with_context(|| format!("无法从文件加载重试提示词: {}", file_path))?
        } else {
            // 使用默认重试提示词
            DEFAULT_RETRY_PROMPT.to_string()
        };

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
            retry_prompt,
            sleep_time: args.sleep_time,
            silence_threshold: args.silence_threshold,
        })
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
    let content = fs::read_to_string(path.as_ref())
        .with_context(|| "读取文件失败")?;

    // 标准化换行符：将 \r\n 和单独的 \r 都转换为 \n
    // 这样可以避免在PTY中换行被重复处理
    let normalized = content
        .replace("\r\n", "\n")  // Windows换行符 -> Unix换行符
        .replace("\r", "\n");   // 旧Mac换行符 -> Unix换行符

    // 去除首尾空白字符
    Ok(normalized.trim().to_string())
}

impl Default for Config {
    /// 创建默认配置
    ///
    /// 默认配置使用空CLI名称和默认提示词
    fn default() -> Self {
        Config {
            cli: String::new(),
            cli_args: Vec::new(),
            continue_prompt: DEFAULT_CONTINUE_PROMPT.to_string(),
            retry_prompt: DEFAULT_RETRY_PROMPT.to_string(),
            sleep_time: DEFAULT_SLEEP_TIME,
            silence_threshold: DEFAULT_SILENCE_THRESHOLD,
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
    }
}
