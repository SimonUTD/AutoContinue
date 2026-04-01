//! # Claude Code 检测适配器 (detector/claude.rs)
//!
//! 通过监控 Claude Code 的 JSONL 会话文件来精确检测 CLI 状态。
//!
//! ## Claude Code 会话文件位置
//!
//! Claude Code 将会话消息写入以下路径：
//! `~/.claude/projects/{projectHash}/{sessionId}.jsonl`
//!
//! 每行是一个 JSON 对象，包含消息类型和内容。
//!
//! ## 消息类型
//!
//! - `user`: 用户输入的消息
//! - `assistant`: Claude 的回复
//! - `system`: 系统消息
//! - `summary`: 会话摘要
//!
//! ## 检测逻辑
//!
//! - 文件正在增长 → `Busy`
//! - 最后一条消息为 `assistant` 且文件停止增长 → `Idle`
//! - 消息内容包含错误关键词 → `Error`
//! - 文件不存在或无法解析 → `Unknown`（fallback 到静默超时）

use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::{CliStatus, Detector};

/// JSONL 文件轮询间隔（秒）
///
/// 每隔此时间重新扫描 JSONL 文件。
/// 参考 Happy 的 SessionScanner 使用 3 秒间隔。
const POLL_INTERVAL_SECS: u64 = 3;

/// 文件"停止增长"判定时间（秒）
///
/// 如果 JSONL 文件在此时间内没有变化，视为 CLI 已完成输出。
const FILE_STABLE_SECS: u64 = 2;

/// Claude Code JSONL 消息的最小化解析结构
///
/// 只提取我们需要的字段，忽略其他内容。
/// Claude Code 的 JSONL 格式较复杂，包含大量我们不需要的字段。
#[derive(Debug, Deserialize)]
struct ClaudeMessage {
    /// 消息类型：user / assistant / system / summary
    #[serde(rename = "type")]
    msg_type: Option<String>,

    /// 消息内容（仅在 type 为 user/assistant 时存在）
    message: Option<MessageContent>,
}

/// 消息内容结构
#[derive(Debug, Deserialize)]
struct MessageContent {
    /// 角色：user / assistant
    role: Option<String>,

    /// 文本内容（可能是字符串或数组，这里简化处理）
    #[serde(default)]
    content: serde_json::Value,
}

/// Claude Code 检测适配器
///
/// 通过监控 Claude Code 的 JSONL 会话文件来检测 CLI 状态。
/// 会定期扫描 `~/.claude/projects/` 目录下的最新会话文件。
pub struct ClaudeDetector {
    /// Claude 项目目录路径
    /// 格式: ~/.claude/projects/{projectHash}/
    projects_dir: PathBuf,

    /// 当前监控的 JSONL 文件路径
    current_session_file: Option<PathBuf>,

    /// 上次扫描文件时读取到的文件大小（字节）
    last_file_size: u64,

    /// 上次文件大小发生变化的时间
    last_size_change_time: Instant,

    /// 上次轮询扫描文件的时间
    last_poll_time: Instant,

    /// 解析到的最后一条消息的类型
    last_message_type: Option<String>,

    /// 解析到的最后一条消息的角色
    last_message_role: Option<String>,

    /// 是否检测到错误
    error_detected: bool,

    /// 错误信息摘要
    error_message: String,

    /// 当前工作目录（用于定位项目目录）
    working_dir: PathBuf,
}

impl ClaudeDetector {
    /// 创建新的 Claude Code 检测器
    pub fn new() -> Self {
        // 获取 Claude 项目目录
        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let projects_dir = home_dir.join(".claude").join("projects");

        // 获取当前工作目录
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        ClaudeDetector {
            projects_dir,
            current_session_file: None,
            last_file_size: 0,
            last_size_change_time: Instant::now(),
            last_poll_time: Instant::now(),
            last_message_type: None,
            last_message_role: None,
            error_detected: false,
            error_message: String::new(),
            working_dir,
        }
    }

    /// 将工作目录路径转换为 Claude 项目目录名
    ///
    /// Claude Code 使用路径分隔符替换策略：
    /// 将路径中的 `/` 替换为 `-`，形成项目目录名。
    /// 例如：`/Users/foo/project` → `-Users-foo-project`
    fn cwd_to_project_dir_name(cwd: &Path) -> String {
        cwd.to_str()
            .map(|s| s.replace('/', "-"))
            .unwrap_or_default()
    }

    /// 扫描项目目录，找到当前工作目录对应的最新 JSONL 文件
    ///
    /// 搜索策略：
    /// 1. 优先在当前工作目录对应的项目目录中查找（精确匹配，支持多实例隔离）
    /// 2. 如果当前项目目录不存在或没有 jsonl 文件，回退到全局搜索最新文件
    fn find_latest_session_file(&self) -> Option<PathBuf> {
        // 如果项目根目录不存在，返回 None
        if !self.projects_dir.exists() {
            return None;
        }

        // 策略1：精确匹配当前工作目录的项目目录
        let project_dir_name = Self::cwd_to_project_dir_name(&self.working_dir);
        let specific_project_path = self.projects_dir.join(&project_dir_name);

        if specific_project_path.is_dir() {
            if let Some(file) =
                Self::find_latest_jsonl_in_dir(&specific_project_path)
            {
                return Some(file);
            }
        }

        // 策略2：回退到全局搜索（兼容旧版或异常情况）
        Self::find_latest_jsonl_global(&self.projects_dir)
    }

    /// 在指定项目目录中查找最新的 .jsonl 文件
    fn find_latest_jsonl_in_dir(dir: &Path) -> Option<PathBuf> {
        let mut latest_file: Option<PathBuf> = None;
        let mut latest_time = SystemTime::UNIX_EPOCH;

        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let file_path = entry.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            if let Ok(metadata) = file_path.metadata() {
                if let Ok(modified) = metadata.modified() {
                    if modified > latest_time {
                        latest_time = modified;
                        latest_file = Some(file_path);
                    }
                }
            }
        }

        latest_file
    }

    /// 全局搜索所有项目目录中最新的 .jsonl 文件
    fn find_latest_jsonl_global(projects_dir: &Path) -> Option<PathBuf> {
        let mut latest_file: Option<PathBuf> = None;
        let mut latest_time = SystemTime::UNIX_EPOCH;

        for entry in std::fs::read_dir(projects_dir).ok()?.flatten() {
            let project_path = entry.path();
            if !project_path.is_dir() {
                continue;
            }

            if let Some(file) = Self::find_latest_jsonl_in_dir(&project_path) {
                if let Ok(metadata) = file.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        if modified > latest_time {
                            latest_time = modified;
                            latest_file = Some(file);
                        }
                    }
                }
            }
        }

        latest_file
    }

    /// 轮询检查 JSONL 文件的变化
    ///
    /// 读取文件末尾新增的内容，解析最后几条消息。
    fn poll_session_file(&mut self) {
        // 检查轮询间隔
        if self.last_poll_time.elapsed() < Duration::from_secs(POLL_INTERVAL_SECS) {
            return;
        }
        self.last_poll_time = Instant::now();

        // 如果没有当前文件，尝试发现
        if self.current_session_file.is_none() {
            self.current_session_file = self.find_latest_session_file();
        }

        // 检查文件是否有变化
        let file_path = match &self.current_session_file {
            Some(path) => path.clone(),
            None => return,
        };

        // 获取文件大小
        let current_size = match std::fs::metadata(&file_path) {
            Ok(meta) => meta.len(),
            Err(_) => {
                // 文件可能被删除，尝试重新发现
                self.current_session_file = self.find_latest_session_file();
                return;
            }
        };

        // 检查是否有新文件出现（可能是新会话）
        if let Some(latest) = self.find_latest_session_file() {
            if latest != file_path {
                // 发现了更新的会话文件，切换监控目标
                self.current_session_file = Some(latest);
                self.last_file_size = 0;
                self.last_size_change_time = Instant::now();
                // 用新文件重新轮询
                self.poll_session_file();
                return;
            }
        }

        // 记录文件大小变化
        if current_size != self.last_file_size {
            self.last_file_size = current_size;
            self.last_size_change_time = Instant::now();
        }

        // 解析文件末尾的消息
        self.parse_last_messages(&file_path);
    }

    /// 解析 JSONL 文件末尾的最后几条消息
    ///
    /// 只读取文件最后 4KB 的内容，避免读取整个大文件。
    fn parse_last_messages(&mut self, path: &Path) {
        // 读取文件末尾内容
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return,
        };

        // 从后向前遍历行，找到最后几条有效消息
        let mut found_last = false;
        for line in content.lines().rev() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // 尝试解析 JSON
            let msg: ClaudeMessage = match serde_json::from_str(line) {
                Ok(m) => m,
                Err(_) => continue,
            };

            // 提取消息类型和角色
            if let Some(ref msg_type) = msg.msg_type {
                if !found_last {
                    self.last_message_type = Some(msg_type.clone());

                    // 提取角色
                    if let Some(ref message) = msg.message {
                        self.last_message_role = message.role.clone();
                    }

                    // 检查是否有错误内容
                    self.check_for_errors(&msg);

                    found_last = true;
                }

                // 只需要最后一条消息，找到就退出
                if found_last {
                    break;
                }
            }
        }
    }

    /// 检查消息内容中是否包含错误信息
    ///
    /// Claude Code 的错误通常出现在 assistant 消息的内容中，
    /// 包含 "error", "failed" 等关键词。
    fn check_for_errors(&mut self, msg: &ClaudeMessage) {
        let content_str = match &msg.message {
            Some(message) => {
                // 将 content 转为字符串进行分析
                match &message.content {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Array(arr) => {
                        // content 可能是 TextBlock 数组
                        let mut text = String::new();
                        for item in arr {
                            if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                                text.push_str(t);
                                text.push('\n');
                            }
                        }
                        text
                    }
                    _ => return,
                }
            }
            None => return,
        };

        // 检查 assistant 消息中是否报告了错误
        // 注意：这里不检查所有包含 "error" 的消息，
        // 只检查明确的错误报告模式
        let lower = content_str.to_lowercase();
        let error_indicators = [
            "error occurred",
            "failed to",
            "i encountered an error",
            "an error happened",
            "i'm unable to",
            "cannot complete",
            "command failed",
            "compilation error",
            "build failed",
        ];

        for indicator in &error_indicators {
            if lower.contains(indicator) {
                self.error_detected = true;
                self.error_message = indicator.to_string();
                return;
            }
        }
    }
}

impl Detector for ClaudeDetector {
    /// 初始化 Claude Code 检测器
    ///
    /// 尝试定位当前工作目录对应的 Claude 项目目录，
    /// 并查找最新的会话文件。
    fn init(&mut self, _cli_name: &str, _cli_args: &[String]) -> Result<()> {
        // 更新工作目录
        if let Ok(cwd) = std::env::current_dir() {
            self.working_dir = cwd;
        }

        // 尝试找到最新的会话文件
        self.current_session_file = self.find_latest_session_file();

        if let Some(ref path) = self.current_session_file {
            // 记录当前文件大小作为基准
            if let Ok(meta) = std::fs::metadata(path) {
                self.last_file_size = meta.len();
            }
        }

        Ok(())
    }

    /// 处理输出数据
    ///
    /// Claude Code 适配器主要依赖 JSONL 文件监控，
    /// 但也会利用输出数据的时间信息来辅助判断。
    /// 每次收到输出数据时触发一次文件轮询。
    fn feed_output(&mut self, _data: &[u8]) {
        // 触发文件轮询（会受轮询间隔限制）
        self.poll_session_file();
    }

    /// 查询当前 CLI 状态
    ///
    /// 判断逻辑：
    /// 1. 检测到错误 → `Error`
    /// 2. JSONL 文件正在增长（2秒内有变化） → `Busy`
    /// 3. 最后消息为 assistant 类型 + 文件停止增长 + 超过静默阈值 → `Idle`
    /// 4. 没有会话文件 → `Unknown`（fallback）
    fn status(&self, silence_duration: Duration, silence_threshold: Duration) -> CliStatus {
        // 优先检查错误
        if self.error_detected {
            return CliStatus::Error {
                message: self.error_message.clone(),
            };
        }

        // 没有会话文件，无法精确判断
        if self.current_session_file.is_none() {
            return CliStatus::Unknown;
        }

        // 文件最近有变化 → 忙碌
        let file_stable_duration = self.last_size_change_time.elapsed();
        if file_stable_duration < Duration::from_secs(FILE_STABLE_SECS) {
            return CliStatus::Busy;
        }

        // 文件已稳定 + 超过静默阈值
        if silence_duration >= silence_threshold {
            // 最后一条消息是 assistant → CLI 已完成回复，处于空闲状态
            if let Some(ref role) = self.last_message_role {
                if role == "assistant" {
                    return CliStatus::Idle;
                }
            }

            // 最后一条消息是 summary → 也视为空闲（会话初始化完成）
            if let Some(ref msg_type) = self.last_message_type {
                if msg_type == "summary" {
                    return CliStatus::Idle;
                }
            }

            // 其他情况，fallback
            return CliStatus::Unknown;
        }

        // 文件已稳定但未超过静默阈值 → 可能在等待，但还没到发送时机
        CliStatus::Busy
    }

    /// 重置检测状态
    fn reset(&mut self) {
        self.error_detected = false;
        self.error_message.clear();
        self.last_message_type = None;
        self.last_message_role = None;
        // 重置文件大小基准，准备检测新的变化
        self.last_size_change_time = Instant::now();
    }

    /// 返回检测器名称
    fn name(&self) -> &str {
        "ClaudeDetector"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试检测器名称
    #[test]
    fn test_detector_name() {
        let detector = ClaudeDetector::new();
        assert_eq!(detector.name(), "ClaudeDetector");
    }

    /// 测试没有会话文件时返回 Unknown
    #[test]
    fn test_no_session_file_returns_unknown() {
        let detector = ClaudeDetector::new();
        let status = detector.status(Duration::from_secs(60), Duration::from_secs(30));
        match status {
            CliStatus::Unknown => {} // 预期结果
            _ => panic!("没有会话文件时应返回 Unknown"),
        }
    }

    /// 测试错误检测
    #[test]
    fn test_error_check() {
        let mut detector = ClaudeDetector::new();
        let msg = ClaudeMessage {
            msg_type: Some("assistant".to_string()),
            message: Some(MessageContent {
                role: Some("assistant".to_string()),
                content: serde_json::Value::String(
                    "I encountered an error while running the build".to_string(),
                ),
            }),
        };
        detector.check_for_errors(&msg);
        assert!(detector.error_detected);
    }

    /// 测试正常消息不触发错误
    #[test]
    fn test_normal_message_no_error() {
        let mut detector = ClaudeDetector::new();
        let msg = ClaudeMessage {
            msg_type: Some("assistant".to_string()),
            message: Some(MessageContent {
                role: Some("assistant".to_string()),
                content: serde_json::Value::String(
                    "I've successfully completed the task".to_string(),
                ),
            }),
        };
        detector.check_for_errors(&msg);
        assert!(!detector.error_detected);
    }

    /// 测试重置
    #[test]
    fn test_reset() {
        let mut detector = ClaudeDetector::new();
        detector.error_detected = true;
        detector.error_message = "test error".to_string();
        detector.last_message_type = Some("assistant".to_string());

        detector.reset();

        assert!(!detector.error_detected);
        assert!(detector.error_message.is_empty());
        assert!(detector.last_message_type.is_none());
    }
}
