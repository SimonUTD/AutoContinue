//! # CLI运行器模块 (runner.rs)
//!
//! 该模块负责启动和管理CLI子进程，使用伪终端（PTY）来保持CLI的完整交互性。
//!
//! ## 功能
//! - 使用portable-pty启动CLI进程
//! - 双向IO转发：stdin -> PTY，PTY -> stdout
//! - 跟踪最后活动时间（输入/输出）用于静默检测
//! - 输出数据同时发送给 Detector 进行状态/错误分析
//! - 确保用户可以正常操作CLI
//!
//! ## 跨平台支持
//! - Windows: 使用ConPTY
//! - Unix/Linux/macOS: 使用传统PTY
//!
//! ## 与旧版本的区别
//! - 移除了虚拟终端（VirtualTerminal）ANSI 解析
//! - 输出直接转发到 stdout，不做任何解析/修改
//! - 状态/错误检测委托给 Detector 模块

use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use portable_pty::{CommandBuilder, PtyPair, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::detector::Detector;

/// 共享检测器类型别名
///
/// Detector trait 对象被 Arc<Mutex<>> 包装以支持多线程访问：
/// - 输出线程：调用 feed_output() 投喂数据
/// - 主线程：调用 status() 查询状态、reset() 重置
pub type SharedDetector = Arc<Mutex<Box<dyn Detector>>>;

/// IO转发线程句柄
///
/// 包含输出和输入转发线程的句柄
#[allow(dead_code)]
pub struct IoHandles {
    /// 输出转发线程句柄（PTY -> stdout + Detector）
    pub output_handle: thread::JoinHandle<()>,
    /// 输入转发线程句柄（stdin -> PTY）
    pub input_handle: thread::JoinHandle<()>,
}

/// CLI运行器
///
/// 负责启动CLI进程并管理其生命周期。
/// 使用PTY来保持CLI的完整交互性。
/// 跟踪最后活动时间用于静默检测。
/// 输出数据同时发送给 Detector 进行分析。
pub struct Runner {
    /// PTY pair（主端和从端）
    pty_pair: PtyPair,

    /// PTY写入器，用于向CLI发送输入（共享，用于多线程访问）
    writer: Arc<Mutex<Box<dyn Write + Send>>>,

    /// 子进程句柄
    child: Box<dyn portable_pty::Child + Send + Sync>,

    /// 标志：进程是否正在运行
    running: Arc<AtomicBool>,

    /// 子进程的退出状态
    exit_status: Arc<Mutex<Option<portable_pty::ExitStatus>>>,

    /// 最后活动时间（输入或输出）
    /// 用于检测CLI是否处于静默状态（等待输入）
    last_activity_time: Arc<Mutex<Instant>>,

    /// 用于注入输入到输入线程的 channel（发送端）
    /// 通过这个 channel 发送的数据会被输入线程当作用户输入处理
    inject_sender: Option<Sender<Vec<u8>>>,

    /// 共享检测器，用于接收输出数据并进行状态分析
    detector: SharedDetector,
}

impl Runner {
    /// 创建并启动CLI运行器
    ///
    /// # 参数
    /// - `cli`: CLI程序名称
    /// - `args`: CLI程序参数
    /// - `detector`: 检测器实例（已初始化）
    ///
    /// # 返回值
    /// 成功返回Runner实例，失败返回错误
    ///
    /// # 错误
    /// - 无法创建PTY
    /// - 无法启动CLI进程
    pub fn new(cli: &str, args: &[String], detector: SharedDetector) -> Result<Self> {
        // 获取原生PTY系统
        let pty_system = native_pty_system();

        // 获取终端大小，如果失败则使用默认值
        let (cols, rows) = terminal::size().unwrap_or((80, 24));

        // 创建PTY对，指定终端大小
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("无法创建PTY")?;

        // 获取当前工作目录
        let current_dir = std::env::current_dir().context("无法获取当前工作目录")?;

        // 构建命令
        // 在Windows上，通过cmd.exe执行以支持.cmd/.bat脚本
        #[cfg(target_os = "windows")]
        let cmd = {
            let mut c = CommandBuilder::new("cmd.exe");
            c.arg("/c");
            c.arg(cli);
            c.args(args);
            c.cwd(&current_dir);
            c
        };

        #[cfg(not(target_os = "windows"))]
        let cmd = {
            let mut c = CommandBuilder::new(cli);
            c.args(args);
            c.cwd(&current_dir);
            c
        };

        // 在从端启动子进程
        let child = pair.slave.spawn_command(cmd).context("无法启动CLI进程")?;

        // 获取写入器用于发送输入
        let writer = pair.master.take_writer().context("无法获取PTY写入器")?;

        let running = Arc::new(AtomicBool::new(true));
        let exit_status = Arc::new(Mutex::new(None));
        let last_activity_time = Arc::new(Mutex::new(Instant::now()));

        Ok(Runner {
            pty_pair: pair,
            writer: Arc::new(Mutex::new(writer)),
            child,
            running,
            exit_status,
            last_activity_time,
            inject_sender: None,
            detector,
        })
    }

    /// 启动双向IO转发线程
    ///
    /// 该方法启动两个后台线程：
    /// 1. 输出转发：PTY -> stdout（直接转发，不解析）+ Detector 数据投喂
    /// 2. 输入转发：stdin -> PTY（用户输入到CLI）
    ///
    /// 每次有输入或输出时，都会更新最后活动时间。
    /// 输出数据同时被发送给 Detector 进行状态分析。
    ///
    /// # 返回值
    /// 返回包含两个线程句柄的IoHandles结构
    pub fn start_io_forwarding(&mut self) -> Result<IoHandles> {
        // 启用终端原始模式，以便直接获取用户输入
        let _ = terminal::enable_raw_mode();

        let running_output = self.running.clone();
        let running_input = self.running.clone();

        // 获取最后活动时间的共享引用
        let last_activity_output = self.last_activity_time.clone();
        let last_activity_input = self.last_activity_time.clone();

        // 获取PTY读取器
        let mut reader = self
            .pty_pair
            .master
            .try_clone_reader()
            .context("无法克隆PTY读取器")?;

        // 获取PTY写入器的共享引用
        let writer = self.writer.clone();
        let writer_for_inject = self.writer.clone();

        // 创建用于注入输入的 channel
        let (inject_tx, inject_rx) = mpsc::channel::<Vec<u8>>();
        self.inject_sender = Some(inject_tx);

        // 获取检测器的共享引用
        let detector = self.detector.clone();

        // 启动输出转发线程：PTY -> stdout + Detector
        // 输出数据直接转发到 stdout（不经过 ANSI 解析），
        // 同时发送给 Detector 进行分析
        let output_handle = thread::spawn(move || {
            let mut stdout = std::io::stdout();
            let mut buffer = [0u8; 4096];

            while running_output.load(Ordering::SeqCst) {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        // EOF，进程已结束
                        break;
                    }
                    Ok(n) => {
                        let data = &buffer[..n];

                        // 更新最后活动时间（有输出）
                        if let Ok(mut time) = last_activity_output.lock() {
                            *time = Instant::now();
                        }

                        // 将数据发送给 Detector 进行分析
                        if let Ok(mut det) = detector.lock() {
                            det.feed_output(data);
                        }

                        // 将数据直接写入 stdout（不做任何解析/修改）
                        if stdout.write_all(data).is_err() {
                            break;
                        }
                        let _ = stdout.flush();
                    }
                    Err(e) => {
                        // 检查是否是非阻塞读取导致的暂时性错误
                        if e.kind() != std::io::ErrorKind::WouldBlock {
                            break;
                        }
                        // 短暂休眠避免忙等待
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        });

        // 启动输入转发线程：stdin -> PTY
        // 使用crossterm的event系统正确处理特殊键（方向键、ESC等）
        // 每次有输入时更新最后活动时间
        // 同时检查注入的输入（通过channel）
        let input_handle = thread::spawn(move || {
            while running_input.load(Ordering::SeqCst) {
                // 首先检查是否有注入的输入
                if let Ok(bytes) = inject_rx.try_recv() {
                    // 更新最后活动时间
                    if let Ok(mut time) = last_activity_input.lock() {
                        *time = Instant::now();
                    }
                    // 发送注入的输入
                    if let Ok(mut w) = writer_for_inject.lock() {
                        if w.write_all(&bytes).is_err() {
                            break;
                        }
                        let _ = w.flush();
                    }
                }

                // 使用crossterm的event poll来非阻塞检测输入
                match crossterm::event::poll(Duration::from_millis(50)) {
                    Ok(true) => {
                        // 有事件可读，使用crossterm读取事件
                        match crossterm::event::read() {
                            Ok(event) => {
                                // 将事件转换为字节序列
                                if let Some(bytes) = event_to_bytes(&event) {
                                    // 更新最后活动时间（有输入）
                                    if let Ok(mut time) = last_activity_input.lock() {
                                        *time = Instant::now();
                                    }

                                    // 将数据写入PTY
                                    if let Ok(mut w) = writer.lock() {
                                        if w.write_all(&bytes).is_err() {
                                            break;
                                        }
                                        let _ = w.flush();
                                    }
                                }
                            }
                            Err(_) => {
                                // 读取事件出错，短暂休眠后继续
                                thread::sleep(Duration::from_millis(10));
                            }
                        }
                    }
                    Ok(false) => {
                        // 没有事件，继续循环
                    }
                    Err(_) => {
                        // poll出错，短暂休眠后继续
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        });

        Ok(IoHandles {
            output_handle,
            input_handle,
        })
    }

    /// 向CLI发送输入
    ///
    /// # 参数
    /// - `input`: 要发送的输入字符串
    ///
    /// # 返回值
    /// 成功返回Ok(())，失败返回错误
    pub fn send_input(&mut self, input: &str) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("无法获取写入器锁"))?;

        // 更新最后活动时间（程序发送输入也算活动）
        if let Ok(mut time) = self.last_activity_time.lock() {
            *time = Instant::now();
        }

        // 写入输入
        writer
            .write_all(input.as_bytes())
            .context("无法向CLI发送输入")?;

        // 确保数据被刷新
        writer.flush().context("无法刷新输入缓冲区")?;

        Ok(())
    }

    /// 向CLI发送一行输入（自动添加回车）
    ///
    /// # 参数
    /// - `line`: 要发送的输入行
    ///
    /// # 返回值
    /// 成功返回Ok(())，失败返回错误
    ///
    /// # 实现说明
    /// 某些 CLI（如 codex）有"粘贴突发"检测机制，会把快速输入后的 Enter
    /// 当作文本累积而不是提交。codex 的 PASTE_ENTER_SUPPRESS_WINDOW 为 120ms，
    /// 即粘贴活动结束后 120ms 内 Enter 仍被当作换行。
    /// 因此在发送文本后等待 150ms 再发送回车，确保粘贴窗口完全关闭。
    pub fn send_line(&mut self, line: &str) -> Result<()> {
        // 发送文本内容（直接写入PTY）
        self.send_input(line)?;

        // 等待 150ms，确保超过 codex 的 PASTE_ENTER_SUPPRESS_WINDOW (120ms)
        // 这样 Enter 会被正确识别为提交而不是换行
        thread::sleep(Duration::from_millis(150));

        // 发送回车通过 channel，让输入线程发送
        // 这样回车和用户按Enter走同样的路径
        if let Some(ref sender) = self.inject_sender {
            // 发送 \r（与 key_event_to_bytes 中 Enter 键一致）
            let _ = sender.send(vec![b'\r']);
        }

        Ok(())
    }

    /// 获取自上次活动以来的静默时间
    ///
    /// # 返回值
    /// 返回自上次输入/输出以来经过的时间
    pub fn get_silence_duration(&self) -> Duration {
        if let Ok(time) = self.last_activity_time.lock() {
            time.elapsed()
        } else {
            Duration::ZERO
        }
    }

    /// 检查CLI进程是否仍在运行
    ///
    /// # 返回值
    /// 如果进程仍在运行返回true，否则返回false
    pub fn is_running(&mut self) -> bool {
        // 尝试获取进程状态（非阻塞）
        match self.child.try_wait() {
            Ok(Some(status)) => {
                // 进程已退出
                self.running.store(false, Ordering::SeqCst);
                *self.exit_status.lock().unwrap() = Some(status);
                false
            }
            Ok(None) => {
                // 进程仍在运行
                true
            }
            Err(_) => {
                // 出错，假设进程已结束
                self.running.store(false, Ordering::SeqCst);
                false
            }
        }
    }

    /// 停止运行标志并恢复终端模式
    ///
    /// 设置运行标志为false，通知所有相关线程停止
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        // 恢复终端模式
        let _ = terminal::disable_raw_mode();
    }
}

/// 将crossterm事件转换为PTY可接受的字节序列
///
/// 该函数处理键盘事件和粘贴事件，将它们转换为
/// 相应的ANSI转义序列或UTF-8字节。
/// 鼠标事件暂不支持。
///
/// # 参数
/// - `event`: crossterm事件
///
/// # 返回值
/// 如果是支持的事件类型，返回对应的字节序列；否则返回None
fn event_to_bytes(event: &Event) -> Option<Vec<u8>> {
    match event {
        Event::Key(key_event) => key_event_to_bytes(key_event),
        Event::Paste(text) => {
            // 粘贴事件，直接返回文本的UTF-8字节
            Some(text.as_bytes().to_vec())
        }
        _ => None, // 忽略鼠标事件和其他事件
    }
}

/// 将键盘事件转换为字节序列
///
/// # 参数
/// - `key_event`: 键盘事件
///
/// # 返回值
/// 返回对应的字节序列
///
/// # 注意
/// 只处理 KeyEventKind::Press 事件，忽略 Release 事件
/// 这是因为Windows上crossterm会同时报告按下和释放事件
fn key_event_to_bytes(key_event: &KeyEvent) -> Option<Vec<u8>> {
    let KeyEvent {
        code,
        modifiers,
        kind,
        ..
    } = key_event;

    // 只处理按键按下事件，忽略释放事件（避免重复输入）
    // Windows上crossterm会报告Press和Release两个事件
    if *kind != KeyEventKind::Press && *kind != KeyEventKind::Repeat {
        return None;
    }

    // 处理Ctrl组合键
    if modifiers.contains(KeyModifiers::CONTROL) {
        return match code {
            // 特殊的Ctrl组合键
            KeyCode::Char('[') => Some(vec![0x1B]), // Ctrl+[ = ESC
            KeyCode::Char('\\') => Some(vec![0x1C]),
            KeyCode::Char(']') => Some(vec![0x1D]),
            KeyCode::Char('^') => Some(vec![0x1E]),
            KeyCode::Char('_') => Some(vec![0x1F]),
            // Ctrl+A 到 Ctrl+Z 映射到 0x01 到 0x1A
            KeyCode::Char(c) => {
                let ctrl_char = (*c as u8).to_ascii_lowercase();
                if ctrl_char >= b'a' && ctrl_char <= b'z' {
                    Some(vec![ctrl_char - b'a' + 1])
                } else {
                    None
                }
            }
            _ => None,
        };
    }

    // 处理普通键和特殊键
    match code {
        // 普通字符
        KeyCode::Char(c) => Some(c.to_string().into_bytes()),

        // 回车键 - 发送 \r
        KeyCode::Enter => Some(vec![b'\r']),

        // 退格键
        KeyCode::Backspace => Some(vec![0x7F]), // DEL

        // Tab键
        KeyCode::Tab => Some(vec![b'\t']),

        // ESC键
        KeyCode::Esc => Some(vec![0x1B]),

        // 方向键（ANSI转义序列）
        KeyCode::Up => Some(vec![0x1B, b'[', b'A']),
        KeyCode::Down => Some(vec![0x1B, b'[', b'B']),
        KeyCode::Right => Some(vec![0x1B, b'[', b'C']),
        KeyCode::Left => Some(vec![0x1B, b'[', b'D']),

        // Home/End键
        KeyCode::Home => Some(vec![0x1B, b'[', b'H']),
        KeyCode::End => Some(vec![0x1B, b'[', b'F']),

        // Insert/Delete键
        KeyCode::Insert => Some(vec![0x1B, b'[', b'2', b'~']),
        KeyCode::Delete => Some(vec![0x1B, b'[', b'3', b'~']),

        // Page Up/Down键
        KeyCode::PageUp => Some(vec![0x1B, b'[', b'5', b'~']),
        KeyCode::PageDown => Some(vec![0x1B, b'[', b'6', b'~']),

        // 功能键 F1-F12
        KeyCode::F(1) => Some(vec![0x1B, b'O', b'P']),
        KeyCode::F(2) => Some(vec![0x1B, b'O', b'Q']),
        KeyCode::F(3) => Some(vec![0x1B, b'O', b'R']),
        KeyCode::F(4) => Some(vec![0x1B, b'O', b'S']),
        KeyCode::F(5) => Some(vec![0x1B, b'[', b'1', b'5', b'~']),
        KeyCode::F(6) => Some(vec![0x1B, b'[', b'1', b'7', b'~']),
        KeyCode::F(7) => Some(vec![0x1B, b'[', b'1', b'8', b'~']),
        KeyCode::F(8) => Some(vec![0x1B, b'[', b'1', b'9', b'~']),
        KeyCode::F(9) => Some(vec![0x1B, b'[', b'2', b'0', b'~']),
        KeyCode::F(10) => Some(vec![0x1B, b'[', b'2', b'1', b'~']),
        KeyCode::F(11) => Some(vec![0x1B, b'[', b'2', b'3', b'~']),
        KeyCode::F(12) => Some(vec![0x1B, b'[', b'2', b'4', b'~']),

        // 其他未处理的键
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector;

    #[cfg(target_os = "windows")]
    fn silence_test_command() -> (&'static str, Vec<String>) {
        (
            "cmd",
            vec!["/c".to_string(), "echo".to_string(), "test".to_string()],
        )
    }

    #[cfg(not(target_os = "windows"))]
    fn silence_test_command() -> (&'static str, Vec<String>) {
        ("echo", vec!["test".to_string()])
    }

    /// 测试简单命令执行
    #[test]
    #[cfg(target_os = "windows")]
    fn test_simple_command() -> Result<()> {
        let det = Arc::new(Mutex::new(detector::create_detector("test")));
        let mut runner = Runner::new(
            "cmd",
            &["/c".to_string(), "echo".to_string(), "hello".to_string()],
            det,
        )?;

        let _ = runner.is_running();
        runner.stop();

        Ok(())
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_simple_command() -> Result<()> {
        let det = Arc::new(Mutex::new(detector::create_detector("test")));
        let mut runner = Runner::new("echo", &["hello".to_string()], det)?;

        let _ = runner.is_running();
        runner.stop();

        Ok(())
    }

    /// 测试静默时间检测
    #[test]
    fn test_silence_duration() -> Result<()> {
        let det = Arc::new(Mutex::new(detector::create_detector("test")));
        let (cli, args) = silence_test_command();
        let runner = Runner::new(cli, &args, det)?;

        // 刚创建时静默时间应该很短
        let duration = runner.get_silence_duration();
        assert!(duration < Duration::from_secs(1));

        // 等待一秒
        thread::sleep(Duration::from_secs(1));

        // 静默时间应该增加
        let duration = runner.get_silence_duration();
        assert!(duration >= Duration::from_secs(1));

        Ok(())
    }
}
