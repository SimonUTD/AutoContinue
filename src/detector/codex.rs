//! # Codex 检测适配器 (detector/codex.rs)
//!
//! 通过监控 Codex 的 JSONL 会话文件来检测 CLI 状态。
//!
//! ## Codex 会话文件位置
//!
//! Codex 将会话记录写入以下路径：
//! `~/.codex/sessions/{subdir}/{prefix}-{sessionId}.jsonl`
//!
//! 环境变量 `CODEX_HOME` 可覆盖默认的 `~/.codex` 路径。
//!
//! ## 检测逻辑
//!
//! 与 Claude Code 适配器类似，通过监控 JSONL 文件变化来判断 CLI 状态。
//! Codex 的消息格式与 Claude Code 不同，但核心思路一致：
//! - 文件正在增长 → `Busy`
//! - 文件停止增长 + 超过静默阈值 → `Idle`
//! - 包含错误信息 → `Error`

use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::{CliStatus, Detector};

/// JSONL 文件轮询间隔（秒）
const POLL_INTERVAL_SECS: u64 = 3;

/// 文件"停止增长"判定时间（秒）
const FILE_STABLE_SECS: u64 = 2;

/// Codex JSONL 消息的最小化解析结构
///
/// Codex 的 JSONL 格式可能与 Claude Code 不同，
/// 这里使用宽松的解析策略，只提取通用字段。
#[derive(Debug, Deserialize)]
struct CodexMessage {
    /// 消息类型
    #[serde(rename = "type")]
    msg_type: Option<String>,

    /// 消息角色
    role: Option<String>,

    /// 消息内容（可以是字符串或复杂结构）
    #[serde(default)]
    content: serde_json::Value,

    /// 可能存在的 message 嵌套结构
    message: Option<CodexMessageContent>,
}

/// Codex 嵌套消息内容
#[derive(Debug, Deserialize)]
struct CodexMessageContent {
    /// 角色
    role: Option<String>,

    /// 文本内容
    #[serde(default)]
    content: serde_json::Value,
}

/// Codex 检测适配器
///
/// 通过监控 Codex 的 JSONL 会话文件来检测 CLI 状态。
/// 自动扫描 `~/.codex/sessions/` 目录（或 `CODEX_HOME/sessions/`）。
pub struct CodexDetector {
    /// Codex sessions 目录路径
    sessions_dir: PathBuf,

    /// 当前监控的 JSONL 文件路径
    current_session_file: Option<PathBuf>,

    /// 上次扫描文件时读取到的文件大小（字节）
    last_file_size: u64,

    /// 上次文件大小发生变化的时间
    last_size_change_time: Instant,

    /// 上次轮询扫描文件的时间
    last_poll_time: Instant,

    /// 解析到的最后一条消息的角色
    last_message_role: Option<String>,

    /// 是否检测到错误
    error_detected: bool,

    /// 错误信息摘要
    error_message: String,
}

impl CodexDetector {
    /// 创建新的 Codex 检测器
    ///
    /// 使用 `CODEX_HOME` 环境变量或默认的 `~/.codex` 作为基础目录。
    pub fn new() -> Self {
        // 优先使用 CODEX_HOME 环境变量
        let codex_home = std::env::var("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
                home.join(".codex")
            });

        let sessions_dir = codex_home.join("sessions");

        CodexDetector {
            sessions_dir,
            current_session_file: None,
            last_file_size: 0,
            last_size_change_time: Instant::now(),
            last_poll_time: Instant::now(),
            last_message_role: None,
            error_detected: false,
            error_message: String::new(),
        }
    }

    /// 递归扫描 sessions 目录，找到最新的 JSONL 文件
    ///
    /// Codex 的会话文件可能在子目录中，文件名格式为：
    /// `{prefix}-{sessionId}.jsonl`
    fn find_latest_session_file(&self) -> Option<PathBuf> {
        if !self.sessions_dir.exists() {
            return None;
        }

        let mut latest_file: Option<PathBuf> = None;
        let mut latest_time = SystemTime::UNIX_EPOCH;

        // 递归扫描目录
        self.scan_dir_recursive(&self.sessions_dir, &mut latest_file, &mut latest_time);

        latest_file
    }

    /// 递归扫描目录中的 JSONL 文件
    fn scan_dir_recursive(
        &self,
        dir: &Path,
        latest_file: &mut Option<PathBuf>,
        latest_time: &mut SystemTime,
    ) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // 递归进入子目录
                self.scan_dir_recursive(&path, latest_file, latest_time);
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                // 比较修改时间
                if let Ok(metadata) = path.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        if modified > *latest_time {
                            *latest_time = modified;
                            *latest_file = Some(path);
                        }
                    }
                }
            }
        }
    }

    /// 轮询检查 JSONL 文件的变化
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

        let file_path = match &self.current_session_file {
            Some(path) => path.clone(),
            None => return,
        };

        // 获取文件大小
        let current_size = match std::fs::metadata(&file_path) {
            Ok(meta) => meta.len(),
            Err(_) => {
                self.current_session_file = self.find_latest_session_file();
                return;
            }
        };

        // 检查是否有更新的文件
        if let Some(latest) = self.find_latest_session_file() {
            if latest != file_path {
                self.current_session_file = Some(latest);
                self.last_file_size = 0;
                self.last_size_change_time = Instant::now();
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

    /// 解析 JSONL 文件末尾的最后一条消息
    fn parse_last_messages(&mut self, path: &Path) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return,
        };

        // 从后向前遍历行，找到最后一条有效消息
        for line in content.lines().rev() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // 尝试解析 JSON
            let msg: CodexMessage = match serde_json::from_str(line) {
                Ok(m) => m,
                Err(_) => continue,
            };

            // 提取角色信息
            // Codex 的消息可能直接有 role 字段，也可能嵌套在 message 中
            let role = msg
                .role
                .clone()
                .or_else(|| msg.message.as_ref().and_then(|m| m.role.clone()));

            if let Some(ref r) = role {
                self.last_message_role = Some(r.clone());

                // 检查错误
                self.check_for_errors(&msg);
                break;
            }

            // 如果有 type 字段但没有 role，也记录
            if msg.msg_type.is_some() {
                break;
            }
        }
    }

    /// 检查消息内容中是否包含错误信息
    fn check_for_errors(&mut self, msg: &CodexMessage) {
        // 提取文本内容
        let content_str = if let serde_json::Value::String(ref s) = msg.content {
            s.clone()
        } else if let Some(ref message) = msg.message {
            match &message.content {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(arr) => {
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
        } else {
            return;
        };

        // 检查错误模式
        let lower = content_str.to_lowercase();
        let error_indicators = [
            "error occurred",
            "failed to",
            "command failed",
            "compilation error",
            "build failed",
            "runtime error",
            "exit code",
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

impl Detector for CodexDetector {
    /// 初始化 Codex 检测器
    fn init(&mut self, _cli_name: &str, _cli_args: &[String]) -> Result<()> {
        // 尝试找到最新的会话文件
        self.current_session_file = self.find_latest_session_file();

        if let Some(ref path) = self.current_session_file {
            if let Ok(meta) = std::fs::metadata(path) {
                self.last_file_size = meta.len();
            }
        }

        Ok(())
    }

    /// 处理输出数据
    ///
    /// 与 Claude Code 适配器类似，主要依赖 JSONL 文件，
    /// 但在收到输出时触发轮询。
    fn feed_output(&mut self, _data: &[u8]) {
        self.poll_session_file();
    }

    /// 查询当前 CLI 状态
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
            // 最后一条消息是 assistant → 空闲
            if let Some(ref role) = self.last_message_role {
                if role == "assistant" {
                    return CliStatus::Idle;
                }
            }

            return CliStatus::Unknown;
        }

        // 文件已稳定但未超过静默阈值
        CliStatus::Busy
    }

    /// 重置检测状态
    fn reset(&mut self) {
        self.error_detected = false;
        self.error_message.clear();
        self.last_message_role = None;
        self.last_size_change_time = Instant::now();
    }

    /// 返回检测器名称
    fn name(&self) -> &str {
        "CodexDetector"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试检测器名称
    #[test]
    fn test_detector_name() {
        let detector = CodexDetector::new();
        assert_eq!(detector.name(), "CodexDetector");
    }

    /// 测试没有会话文件时返回 Unknown
    #[test]
    fn test_no_session_file_returns_unknown() {
        let detector = CodexDetector::new();
        let status = detector.status(Duration::from_secs(60), Duration::from_secs(30));
        match status {
            CliStatus::Unknown => {} // 预期
            _ => panic!("没有会话文件时应返回 Unknown"),
        }
    }

    /// 测试重置
    #[test]
    fn test_reset() {
        let mut detector = CodexDetector::new();
        detector.error_detected = true;
        detector.error_message = "test error".to_string();
        detector.last_message_role = Some("assistant".to_string());

        detector.reset();

        assert!(!detector.error_detected);
        assert!(detector.error_message.is_empty());
        assert!(detector.last_message_role.is_none());
    }
}
