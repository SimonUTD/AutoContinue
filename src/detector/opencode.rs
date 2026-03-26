//! # OpenCode 检测适配器 (detector/opencode.rs)
//!
//! 通过监控 OpenCode 的结构化消息文件检测 CLI 状态。
//!
//! ## OpenCode 消息文件位置
//!
//! OpenCode 的数据目录默认位于：
//! `~/.local/share/opencode`
//!
//! 会话消息位于：
//! `storage/message/ses_*/msg_*.json`
//!
//! 可通过以下环境变量覆盖默认位置：
//! - `OPENCODE_DATA_DIR`
//! - `XDG_DATA_HOME`（将拼接 `opencode`）

use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::{CliStatus, Detector};

/// 消息文件轮询间隔（秒）
const POLL_INTERVAL_SECS: u64 = 3;

/// 文件稳定判定窗口（秒）
const FILE_STABLE_SECS: u64 = 2;

/// OpenCode 消息文件最小解析结构
#[derive(Debug, Deserialize)]
struct OpenCodeMessage {
    /// 消息角色（user / assistant）
    role: Option<String>,

    /// 错误信息（存在时通常表示本轮失败）
    error: Option<OpenCodeError>,
}

/// OpenCode 消息错误结构
#[derive(Debug, Deserialize)]
struct OpenCodeError {
    /// 错误名称
    name: Option<String>,

    /// 错误详情
    data: Option<OpenCodeErrorData>,
}

/// OpenCode 错误详情
#[derive(Debug, Deserialize)]
struct OpenCodeErrorData {
    /// 错误文本
    message: Option<String>,
}

/// OpenCode 检测适配器
pub struct OpenCodeDetector {
    /// OpenCode message 根目录（.../storage/message）
    message_root: PathBuf,

    /// 当前观测的最新消息文件
    current_message_file: Option<PathBuf>,

    /// 最近观测到的消息文件修改时间
    last_observed_modified: Option<SystemTime>,

    /// 上次文件变化时刻
    last_change_time: Instant,

    /// 上次轮询时刻
    last_poll_time: Instant,

    /// 最近一条消息角色
    last_message_role: Option<String>,

    /// 是否检测到错误
    error_detected: bool,

    /// 错误摘要
    error_message: String,
}

impl OpenCodeDetector {
    /// 创建新的 OpenCode 检测器
    pub fn new() -> Self {
        let data_dir = Self::resolve_data_dir();
        let message_root = data_dir.join("storage").join("message");
        Self::with_message_root(message_root)
    }

    /// 使用指定 message 根目录创建检测器
    fn with_message_root(message_root: PathBuf) -> Self {
        OpenCodeDetector {
            message_root,
            current_message_file: None,
            last_observed_modified: None,
            last_change_time: Instant::now(),
            last_poll_time: Instant::now(),
            last_message_role: None,
            error_detected: false,
            error_message: String::new(),
        }
    }

    /// 解析 OpenCode 数据目录
    fn resolve_data_dir() -> PathBuf {
        if let Ok(path) = std::env::var("OPENCODE_DATA_DIR") {
            return PathBuf::from(path);
        }

        if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
            return PathBuf::from(xdg_data_home).join("opencode");
        }

        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".local").join("share").join("opencode")
    }

    /// 获取文件修改时间
    fn file_modified_time(path: &Path) -> Option<SystemTime> {
        let metadata = std::fs::metadata(path).ok()?;
        metadata.modified().ok()
    }

    /// 扫描 message 目录并返回最新消息文件
    fn find_latest_message_file(&self) -> Option<(PathBuf, SystemTime)> {
        if !self.message_root.exists() {
            return None;
        }

        let mut latest_file: Option<PathBuf> = None;
        let mut latest_time = SystemTime::UNIX_EPOCH;

        let sessions = std::fs::read_dir(&self.message_root).ok()?;
        for session in sessions.flatten() {
            let session_path = session.path();
            if !session_path.is_dir() {
                continue;
            }

            let messages = match std::fs::read_dir(&session_path) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for message in messages.flatten() {
                let message_path = message.path();
                let is_json = message_path.extension().and_then(|ext| ext.to_str()) == Some("json");
                if !is_json {
                    continue;
                }

                let modified = match Self::file_modified_time(&message_path) {
                    Some(time) => time,
                    None => continue,
                };

                if modified > latest_time {
                    latest_time = modified;
                    latest_file = Some(message_path);
                }
            }
        }

        latest_file.map(|path| (path, latest_time))
    }

    /// 从最新消息文件更新状态
    fn parse_message_file(&mut self, path: &Path) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return,
        };

        let message: OpenCodeMessage = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(_) => return,
        };

        self.last_message_role = message.role;

        if let Some(error) = message.error {
            self.error_detected = true;
            self.error_message = Self::build_error_message(&error);
            return;
        }

        self.error_detected = false;
        self.error_message.clear();
    }

    /// 构造错误摘要文本
    fn build_error_message(error: &OpenCodeError) -> String {
        if let Some(data) = &error.data {
            if let Some(message) = &data.message {
                if !message.is_empty() {
                    return message.clone();
                }
            }
        }

        error
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "OpenCode message error".to_string())
    }

    /// 轮询消息目录变化并刷新状态
    fn poll_message_file(&mut self) {
        if self.last_poll_time.elapsed() < Duration::from_secs(POLL_INTERVAL_SECS) {
            return;
        }
        self.last_poll_time = Instant::now();

        let (latest_path, latest_modified) = match self.find_latest_message_file() {
            Some(pair) => pair,
            None => return,
        };

        let is_new_file = self.current_message_file.as_ref() != Some(&latest_path);
        let is_newer = self
            .last_observed_modified
            .map(|old| latest_modified > old)
            .unwrap_or(true);

        if !is_new_file && !is_newer {
            return;
        }

        self.current_message_file = Some(latest_path.clone());
        self.last_observed_modified = Some(latest_modified);
        self.last_change_time = Instant::now();
        self.parse_message_file(&latest_path);
    }
}

impl Detector for OpenCodeDetector {
    /// 初始化 OpenCode 检测器
    fn init(&mut self, _cli_name: &str, _cli_args: &[String]) -> Result<()> {
        if let Some((latest_path, latest_modified)) = self.find_latest_message_file() {
            self.current_message_file = Some(latest_path.clone());
            self.last_observed_modified = Some(latest_modified);
            self.last_change_time = Instant::now();
            self.parse_message_file(&latest_path);
        }

        Ok(())
    }

    /// 接收输出后触发轮询
    fn feed_output(&mut self, _data: &[u8]) {
        self.poll_message_file();
    }

    /// 返回当前状态
    fn status(&self, silence_duration: Duration, silence_threshold: Duration) -> CliStatus {
        if self.error_detected {
            return CliStatus::Error {
                message: self.error_message.clone(),
            };
        }

        if self.current_message_file.is_none() {
            return CliStatus::Unknown;
        }

        if self.last_change_time.elapsed() < Duration::from_secs(FILE_STABLE_SECS) {
            return CliStatus::Busy;
        }

        if silence_duration < silence_threshold {
            return CliStatus::Busy;
        }

        if self.last_message_role.as_deref() == Some("assistant") {
            return CliStatus::Idle;
        }

        CliStatus::Unknown
    }

    /// 重置状态
    fn reset(&mut self) {
        self.error_detected = false;
        self.error_message.clear();
        self.last_message_role = None;
        self.last_change_time = Instant::now();
    }

    /// 检测器名称
    fn name(&self) -> &str {
        "OpenCodeDetector"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_message_file(
        message_root: &Path,
        session_id: &str,
        file_name: &str,
        role: &str,
        error_message: Option<&str>,
    ) -> PathBuf {
        let session_dir = message_root.join(session_id);
        fs::create_dir_all(&session_dir).unwrap();

        let file_path = session_dir.join(file_name);
        let json = if let Some(message) = error_message {
            format!(
                "{{\"role\":\"{}\",\"error\":{{\"name\":\"MessageError\",\"data\":{{\"message\":\"{}\"}}}}}}",
                role, message
            )
        } else {
            format!("{{\"role\":\"{}\"}}", role)
        };

        fs::write(&file_path, json).unwrap();
        file_path
    }

    #[test]
    fn test_detector_name() {
        let detector = OpenCodeDetector::with_message_root(PathBuf::from("/tmp/not-used"));
        assert_eq!(detector.name(), "OpenCodeDetector");
    }

    #[test]
    fn test_no_message_file_returns_unknown() {
        let temp_dir = tempfile::tempdir().unwrap();
        let missing_root = temp_dir.path().join("storage").join("message");
        let detector = OpenCodeDetector::with_message_root(missing_root);

        let status = detector.status(Duration::from_secs(60), Duration::from_secs(30));
        assert!(matches!(status, CliStatus::Unknown));
    }

    #[test]
    fn test_error_message_detected() {
        let temp_dir = tempfile::tempdir().unwrap();
        let message_root = temp_dir.path().join("storage").join("message");

        write_message_file(
            &message_root,
            "ses_test",
            "msg_error.json",
            "assistant",
            Some("The operation was aborted."),
        );

        let mut detector = OpenCodeDetector::with_message_root(message_root);
        detector.init("opencode", &[]).unwrap();

        let status = detector.status(Duration::from_secs(5), Duration::from_secs(30));
        match status {
            CliStatus::Error { message } => assert_eq!(message, "The operation was aborted."),
            _ => panic!("应检测到 Error 状态"),
        }
    }

    #[test]
    fn test_assistant_message_can_be_idle() {
        let temp_dir = tempfile::tempdir().unwrap();
        let message_root = temp_dir.path().join("storage").join("message");

        write_message_file(&message_root, "ses_test", "msg_ok.json", "assistant", None);

        let mut detector = OpenCodeDetector::with_message_root(message_root);
        detector.init("opencode", &[]).unwrap();
        detector.last_change_time = Instant::now() - Duration::from_secs(FILE_STABLE_SECS + 1);

        let status = detector.status(Duration::from_secs(40), Duration::from_secs(30));
        assert!(matches!(status, CliStatus::Idle));
    }
}
