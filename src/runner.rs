//! # CLI运行器模块 (runner.rs)
//!
//! 该模块负责启动和管理CLI子进程，使用伪终端（PTY）来保持CLI的完整交互性。
//!
//! ## 功能
//! - 使用portable-pty启动CLI进程
//! - 双向IO转发：stdin -> PTY，PTY -> stdout
//! - 跟踪最后活动时间（输入/输出）用于静默检测
//! - 虚拟终端追踪输出内容，用于错误检测
//! - 确保用户可以正常操作CLI
//!
//! ## 跨平台支持
//! - Windows: 使用ConPTY
//! - Unix/Linux/macOS: 使用传统PTY

use anyhow::{Context, Result};
use crossterm::terminal;
use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::terminal::{SharedTerminal, create_shared_terminal};

/// IO转发线程句柄
///
/// 包含输出和输入转发线程的句柄
#[allow(dead_code)]
pub struct IoHandles {
    /// 输出转发线程句柄（PTY -> stdout）
    pub output_handle: thread::JoinHandle<()>,
    /// 输入转发线程句柄（stdin -> PTY）
    pub input_handle: thread::JoinHandle<()>,
}

/// CLI运行器
///
/// 负责启动CLI进程并管理其生命周期。
/// 使用PTY来保持CLI的完整交互性。
/// 跟踪最后活动时间用于静默检测。
/// 使用虚拟终端追踪输出内容用于错误检测。
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

    /// 虚拟终端，用于追踪输出内容和检测错误
    terminal: SharedTerminal,
}

impl Runner {
    /// 创建并启动CLI运行器
    ///
    /// # 参数
    /// - `cli`: CLI程序名称
    /// - `args`: CLI程序参数
    ///
    /// # 返回值
    /// 成功返回Runner实例，失败返回错误
    ///
    /// # 错误
    /// - 无法创建PTY
    /// - 无法启动CLI进程
    pub fn new(cli: &str, args: &[String]) -> Result<Self> {
        // 获取原生PTY系统
        let pty_system = native_pty_system();

        // 获取终端大小，如果失败则使用默认值
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let terminal_width = cols as usize;
        let terminal_height = rows as usize;

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
        // 在Windows上，直接使用命令名，让系统PATH来解析
        // 对于.cmd/.bat脚本（如npm全局安装的CLI），需要通过cmd.exe来执行
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
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("无法启动CLI进程")?;

        // 获取写入器用于发送输入
        let writer = pair
            .master
            .take_writer()
            .context("无法获取PTY写入器")?;

        let running = Arc::new(AtomicBool::new(true));
        let exit_status = Arc::new(Mutex::new(None));
        let last_activity_time = Arc::new(Mutex::new(Instant::now()));

        // 创建虚拟终端用于追踪输出和检测错误
        let terminal = create_shared_terminal(terminal_width, terminal_height);

        Ok(Runner {
            pty_pair: pair,
            writer: Arc::new(Mutex::new(writer)),
            child,
            running,
            exit_status,
            last_activity_time,
            inject_sender: None,
            terminal,
        })
    }

    /// 启动双向IO转发线程
    ///
    /// 该方法启动后台线程进行双向 IO 转发：
    /// 1. 输出转发：PTY -> stdout（显示CLI输出）+ 虚拟终端处理
    /// 2. 输入转发：stdin -> PTY（用户输入到CLI）
    ///
    /// ## 输入处理策略
    /// 使用原始字节透传方式，而不是 crossterm 的事件系统。
    /// 这样可以正确处理子进程启用的鼠标模式，避免鼠标序列被错误解析。
    ///
    /// 每次有输入或输出时，都会更新最后活动时间。
    /// 输出数据同时被送入虚拟终端进行解析和错误检测。
    ///
    /// # 返回值
    /// 返回包含两个线程句柄的IoHandles结构
    pub fn start_io_forwarding(&mut self) -> Result<IoHandles> {
        // 启用终端原始模式，以便直接获取用户输入
        let _ = terminal::enable_raw_mode();

        let running_output = self.running.clone();
        let running_input = self.running.clone();
        let running_stdin = self.running.clone();

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

        // 创建用于注入输入的 channel
        let (inject_tx, inject_rx) = mpsc::channel::<Vec<u8>>();
        self.inject_sender = Some(inject_tx);

        // 创建用于 stdin 原始字节的 channel
        // 使用单独的线程读取 stdin，实现真正的原始字节透传
        let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>();

        // 获取虚拟终端的共享引用
        let terminal = self.terminal.clone();

        // 启动输出转发线程：PTY -> stdout + 虚拟终端
        // 每次有输出时更新最后活动时间，并送入虚拟终端处理
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
                        // 更新最后活动时间（有输出）
                        if let Ok(mut time) = last_activity_output.lock() {
                            *time = Instant::now();
                        }

                        // 将数据送入虚拟终端处理（用于错误检测）
                        if let Ok(mut term) = terminal.lock() {
                            term.process(&buffer[..n]);
                        }

                        // 将数据写入stdout
                        if stdout.write_all(&buffer[..n]).is_err() {
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

        // 启动 stdin 原始读取线程
        // 这个线程阻塞式读取 stdin，将原始字节通过 channel 发送
        // 这样可以实现真正的透明转发，包括鼠标事件
        thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut handle = stdin.lock();
            let mut buffer = [0u8; 1024];

            // 在 raw mode 下，stdin.read() 会返回原始字节
            // 包括键盘输入、鼠标事件（如果终端发送）、粘贴内容等
            loop {
                // 检查是否应该停止
                if !running_stdin.load(Ordering::SeqCst) {
                    break;
                }

                match handle.read(&mut buffer) {
                    Ok(0) => break,  // EOF，stdin 关闭
                    Ok(n) => {
                        // 发送原始字节到 channel
                        if stdin_tx.send(buffer[..n].to_vec()).is_err() {
                            break;  // channel 关闭
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                        // 被信号中断，继续读取
                        continue;
                    }
                    Err(_) => break,  // 其他错误，退出
                }
            }
        });

        // 启动输入转发线程：合并 stdin 和注入的输入，写入 PTY
        // 每次有输入时更新最后活动时间
        let input_handle = thread::spawn(move || {
            while running_input.load(Ordering::SeqCst) {
                let mut has_activity = false;

                // 检查 stdin 原始字节
                while let Ok(bytes) = stdin_rx.try_recv() {
                    has_activity = true;
                    // 更新最后活动时间
                    if let Ok(mut time) = last_activity_input.lock() {
                        *time = Instant::now();
                    }
                    // 直接写入 PTY（原始字节透传）
                    if let Ok(mut w) = writer.lock() {
                        if w.write_all(&bytes).is_err() {
                            return;
                        }
                        let _ = w.flush();
                    }
                }

                // 检查注入的输入
                while let Ok(bytes) = inject_rx.try_recv() {
                    has_activity = true;
                    // 更新最后活动时间
                    if let Ok(mut time) = last_activity_input.lock() {
                        *time = Instant::now();
                    }
                    // 写入 PTY
                    if let Ok(mut w) = writer.lock() {
                        if w.write_all(&bytes).is_err() {
                            return;
                        }
                        let _ = w.flush();
                    }
                }

                // 如果没有活动，短暂休眠避免忙等待
                if !has_activity {
                    thread::sleep(Duration::from_millis(5));
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
        let mut writer = self.writer.lock().map_err(|_| anyhow::anyhow!("无法获取写入器锁"))?;

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
    pub fn send_line(&mut self, line: &str) -> Result<()> {
        // 发送文本内容（直接写入PTY）
        self.send_input(line)?;

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

    /// 检查输出中是否包含红色文本（错误检测）
    ///
    /// 通过虚拟终端分析自上次检查以来的新增输出，
    /// 判断是否包含红色文本，用于区分正常结束和错误状态。
    ///
    /// # 返回值
    /// 如果检测到红色文本返回 true，表示可能是错误
    pub fn has_error_output(&self) -> bool {
        if let Ok(term) = self.terminal.lock() {
            term.has_red_content()
        } else {
            false
        }
    }

    /// 获取错误输出内容
    ///
    /// 返回虚拟终端中检测到的红色文本内容
    ///
    /// # 返回值
    /// 红色文本内容字符串
    pub fn get_error_content(&self) -> String {
        if let Ok(term) = self.terminal.lock() {
            term.get_red_content()
        } else {
            String::new()
        }
    }

    /// 清除错误检测状态
    ///
    /// 在发送提示词后调用，重置新增内容追踪器，
    /// 准备检测下一轮输出中的错误。
    pub fn clear_error_state(&self) {
        if let Ok(mut term) = self.terminal.lock() {
            term.clear_new_content();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试简单命令执行
    #[test]
    #[cfg(target_os = "windows")]
    fn test_simple_command() -> Result<()> {
        let mut runner = Runner::new("cmd", &["/c".to_string(), "echo".to_string(), "hello".to_string()])?;

        // 等待进程结束
        let status = runner.child.wait().context("等待进程失败")?;
        assert!(status.success());

        Ok(())
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_simple_command() -> Result<()> {
        let mut runner = Runner::new("echo", &["hello".to_string()])?;

        // 等待进程结束
        let status = runner.child.wait().context("等待进程失败")?;
        assert!(status.success());

        Ok(())
    }

    /// 测试静默时间检测
    #[test]
    fn test_silence_duration() -> Result<()> {
        let runner = Runner::new("cmd", &["/c".to_string(), "echo".to_string(), "test".to_string()])?;

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
