# Forge Engine Tracing 系统设计方案 v2

## 1. 设计目标

**核心目的**：记录完整的对话执行过程，用于 Claude Code 分析 engine 实现问题，帮助优化代码。

**使用流程**：
```
forge-engine 执行对话 → 自动写入 trace 文件 → Claude Code 读取分析 → 优化 engine 代码
```

**关键需求**：
1. **完整性**：记录对话内容、工具调用、API 请求、错误详情
2. **自动化**：默认启用，自动写入文件，无需手动导出
3. **易读性**：JSONL 格式，结构化，包含上下文信息
4. **性能**：异步写入，目标性能影响 < 5%

## 2. 设计原则

1. **默认写文件**：不是可选的导出，而是运行时自动写入
2. **完整记录**：对话内容、工具详情、错误信息全部记录（这是开发工具，不考虑隐私）
3. **最小侵入**：复用现有 `AgentEvent` 系统，扩展而非重建
4. **简单直接**：JSONL 格式，每行一个事件，易于处理大文件

## 3. 架构设计

```
┌─────────────────────────────────────────────────────────────────┐
│                        forge-config                              │
│  TracingConfig: 配置文件路径、缓冲区大小                          │
└─────────────────────────────────────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────┐
│                        forge-domain                              │
│  AgentEvent: 扩展事件类型（+对话内容、+会话上下文）               │
└─────────────────────────────────────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────┐
│                        forge-agent                               │
│  TraceWriter: 异步写入 JSONL 文件                                │
└─────────────────────────────────────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────┐
│                         forge-sdk                                │
│  ForgeSDK: 集成 TraceWriter，会话开始时创建文件                  │
└─────────────────────────────────────────────────────────────────┘
```

**简化说明**：
- 不需要复杂的统计聚合（Claude Code 会做）
- 不需要内存查询 API（直接读文件）
- 不需要导出接口（自动写入）

## 4. 模块详细设计

### 4.1 配置 (`forge-config/src/tracing.rs` - 新增)

```rust
//! Tracing configuration for session recording.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Tracing 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    /// 启用 tracing（默认 true）
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// 输出目录（默认 ~/.forge/traces/）
    #[serde(default = "default_trace_dir")]
    pub output_dir: PathBuf,

    /// 文件名模板（支持变量：{session_id}, {timestamp}）
    #[serde(default = "default_filename_template")]
    pub filename_template: String,

    /// 内存缓冲区大小（事件数量，用于批量写入）
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    /// 记录对话内容（默认 true）
    #[serde(default = "default_true")]
    pub record_messages: bool,

    /// 记录工具输入输出（默认 true）
    #[serde(default = "default_true")]
    pub record_tool_details: bool,

    /// 最大保留 trace 文件数量（None = 不限制）
    #[serde(default = "default_max_trace_files")]
    pub max_trace_files: Option<usize>,

    /// trace 文件最大保留天数（None = 不限制）
    #[serde(default = "default_max_trace_age_days")]
    pub max_trace_age_days: Option<u32>,
}

fn default_true() -> bool {
    true
}

fn default_trace_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("forge")
        .join("traces")
}

fn default_filename_template() -> String {
    "{timestamp}_{session_id}.jsonl".to_string()
}

fn default_buffer_size() -> usize {
    100 // 每 100 个事件批量写入一次
}

fn default_max_trace_files() -> Option<usize> {
    Some(100) // 默认保留最新 100 个文件
}

fn default_max_trace_age_days() -> Option<u32> {
    Some(30) // 默认保留 30 天
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            output_dir: default_trace_dir(),
            filename_template: default_filename_template(),
            buffer_size: default_buffer_size(),
            record_messages: true,
            record_tool_details: true,
            max_trace_files: default_max_trace_files(),
            max_trace_age_days: default_max_trace_age_days(),
        }
    }
}

impl TracingConfig {
    /// 生成完整的输出文件路径
    pub fn generate_path(&self, session_id: &str) -> PathBuf {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let filename = self
            .filename_template
            .replace("{session_id}", session_id)
            .replace("{timestamp}", &timestamp.to_string());
        self.output_dir.join(filename)
    }

    /// 从环境变量加载配置（覆盖默认值）
    pub fn from_env(mut self) -> Self {
        if let Ok(val) = std::env::var("FORGE_TRACING_ENABLED") {
            self.enabled = val.parse().unwrap_or(self.enabled);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_OUTPUT_DIR") {
            self.output_dir = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_BUFFER_SIZE") {
            self.buffer_size = val.parse().unwrap_or(self.buffer_size);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_RECORD_MESSAGES") {
            self.record_messages = val.parse().unwrap_or(self.record_messages);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_RECORD_TOOL_DETAILS") {
            self.record_tool_details = val.parse().unwrap_or(self.record_tool_details);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_MAX_FILES") {
            self.max_trace_files = val.parse().ok();
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_MAX_AGE_DAYS") {
            self.max_trace_age_days = val.parse().ok();
        }
        self
    }

    /// 清理旧的 trace 文件
    pub async fn cleanup_old_traces(&self) -> std::io::Result<()> {
        if !self.output_dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&self.output_dir).await?;
        let mut files = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            if let Ok(metadata) = entry.metadata().await {
                if metadata.is_file() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.ends_with(".jsonl") {
                            files.push((entry.path(), metadata));
                        }
                    }
                }
            }
        }

        // 按修改时间排序（最新的在前）
        files.sort_by(|a, b| {
            b.1.modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                .cmp(&a.1.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH))
        });

        let now = std::time::SystemTime::now();

        // 删除超过数量限制的文件
        if let Some(max_files) = self.max_trace_files {
            for (path, _) in files.iter().skip(max_files) {
                let _ = tokio::fs::remove_file(path).await;
            }
        }

        // 删除超过时间限制的文件
        if let Some(max_age_days) = self.max_trace_age_days {
            let max_age = std::time::Duration::from_secs(max_age_days as u64 * 24 * 3600);
            for (path, metadata) in &files {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age > max_age {
                            let _ = tokio::fs::remove_file(path).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
```

### 4.2 事件类型扩展 (`forge-domain/src/event.rs`)

```rust
// 在现有 AgentEvent 枚举中新增以下变体：

pub enum AgentEvent {
    // ============ 现有事件（保持不变）============
    // TokenUsage, ToolCallStart, ToolResult, ConfirmationRequired, ...

    // ============ 新增：会话生命周期 ============

    /// 会话开始（包含环境上下文）
    SessionStart {
        session_id: String,
        timestamp: i64,
        context: SessionContext,
    },

    /// 会话结束
    SessionEnd {
        session_id: String,
        timestamp: i64,
        duration_ms: u64,
    },

    // ============ 新增：对话内容 ============

    /// 用户消息
    UserMessage {
        content: String,
        timestamp: i64,
    },

    /// Assistant 响应（流式输出的完整内容）
    AssistantMessage {
        content: String,
        timestamp: i64,
    },

    // ============ 新增：API 调用 ============

    /// API 请求开始
    ApiRequest {
        request_id: String,
        model: String,
        timestamp: i64,
    },

    /// API 响应
    ApiResponse {
        request_id: String,
        duration_ms: u64,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: Option<u32>,
        cache_write_tokens: Option<u32>,
        timestamp: i64,
    },

    /// API 错误
    ApiError {
        request_id: String,
        error: String,
        details: Option<String>,
        timestamp: i64,
    },

    // ============ 新增：工具调用详情（用于 tracing）============

    /// 工具调用详细结果（不修改现有 ToolResult，避免破坏性变更）
    ToolResultDetailed {
        id: String,
        output: serde_json::Value,
        error: Option<String>,
        duration_ms: u64,
        timestamp: i64,
    },
}

/// 会话上下文信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    /// Engine 版本
    pub engine_version: String,
    /// 工作目录
    pub working_dir: String,
    /// Git 分支
    pub git_branch: Option<String>,
    /// Git commit
    pub git_commit: Option<String>,
    /// 模型名称
    pub model: String,
    /// 配置摘要（不包含敏感信息）
    pub config_summary: serde_json::Value,
}
```

**注意**：保持现有 `ToolResult` 不变，使用新的 `ToolResultDetailed` 用于 tracing。在工具执行时，可以同时发送两个事件以保持兼容性。

### 4.3 错误类型 (`forge-agent/src/trace_error.rs` - 新增)

```rust
//! Error types for tracing system.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TraceError {
    #[error("Failed to write trace: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Trace channel is full")]
    ChannelFull,

    #[error("Trace channel is closed")]
    ChannelClosed,

    #[error("Failed to serialize event: {0}")]
    SerializationError(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, TraceError>;
```

### 4.4 Trace 写入器 (`forge-agent/src/trace_writer.rs` - 新增)

```rust
//! Asynchronous trace writer for session recording.

use forge_domain::AgentEvent;
use std::path::PathBuf;
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};
use crate::trace_error::{TraceError, Result};

/// 写入器消息类型
enum TraceWriterMessage {
    /// 记录事件
    Event(AgentEvent),
    /// Flush 并返回确认
    Flush(oneshot::Sender<()>),
}

/// 异步 trace 写入器
pub struct TraceWriter {
    /// 发送通道
    tx: mpsc::Sender<TraceWriterMessage>,
    /// 输出文件路径
    output_path: PathBuf,
}

impl TraceWriter {
    /// 创建并启动写入器
    pub async fn new(
        output_path: PathBuf,
        buffer_size: usize,
        batch_size: usize,
    ) -> Result<Self> {
        // 确保目录存在
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // 打开文件（追加模式）
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&output_path)
            .await?;

        // 创建通道
        let (tx, mut rx) = mpsc::channel::<TraceWriterMessage>(buffer_size);

        // 启动后台写入任务
        tokio::spawn(async move {
            let mut event_buffer = Vec::with_capacity(batch_size);

            loop {
                tokio::select! {
                    // 接收消息
                    msg = rx.recv() => {
                        match msg {
                            Some(TraceWriterMessage::Event(event)) => {
                                event_buffer.push(event);

                                // 批量写入
                                if event_buffer.len() >= batch_size {
                                    if let Err(e) = write_batch(&mut file, &event_buffer).await {
                                        eprintln!("Failed to write trace batch to {:?}: {}", output_path, e);
                                    }
                                    event_buffer.clear();
                                }
                            }
                            Some(TraceWriterMessage::Flush(tx)) => {
                                // 写入剩余事件
                                if !event_buffer.is_empty() {
                                    if let Err(e) = write_batch(&mut file, &event_buffer).await {
                                        eprintln!("Failed to write trace batch to {:?}: {}", output_path, e);
                                    }
                                    event_buffer.clear();
                                }
                                // Flush 文件
                                let _ = file.flush().await;
                                // 发送确认
                                let _ = tx.send(());
                            }
                            None => {
                                // 通道关闭，写入剩余事件并退出
                                if !event_buffer.is_empty() {
                                    if let Err(e) = write_batch(&mut file, &event_buffer).await {
                                        eprintln!("Failed to write final trace batch to {:?}: {}", output_path, e);
                                    }
                                }
                                let _ = file.flush().await;
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(Self { tx, output_path })
    }

    /// 记录事件（非阻塞）
    pub fn record(&self, event: AgentEvent) -> Result<()> {
        self.tx
            .try_send(TraceWriterMessage::Event(event))
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => TraceError::ChannelFull,
                mpsc::error::TrySendError::Closed(_) => TraceError::ChannelClosed,
            })
    }

    /// 获取输出文件路径
    pub fn output_path(&self) -> &PathBuf {
        &self.output_path
    }

    /// 等待所有事件写入完成
    pub async fn flush(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(TraceWriterMessage::Flush(tx))
            .await
            .map_err(|_| TraceError::ChannelClosed)?;
        rx.await.map_err(|_| TraceError::ChannelClosed)?;
        Ok(())
    }
}

/// 批量写入事件
async fn write_batch(file: &mut File, events: &[AgentEvent]) -> std::io::Result<()> {
    let mut buffer = String::new();
    for event in events {
        if let Ok(json) = serde_json::to_string(event) {
            buffer.push_str(&json);
            buffer.push('\n');
        }
    }
    file.write_all(buffer.as_bytes()).await?;
    Ok(())
}
```

### 4.4 SDK 集成 (`forge-sdk/src/sdk/mod.rs`)

```rust
// 在 ForgeSDK 结构体中添加字段
pub struct ForgeSDK {
    // ... 现有字段

    /// Trace 写入器（可选）
    trace_writer: Option<Arc<TraceWriter>>,
}

impl ForgeSDK {
    pub async fn new(config: ForgeConfig) -> Result<Self> {
        // ... 现有初始化代码

        // 初始化 trace writer
        let trace_writer = if config.tracing.enabled {
            // 清理旧的 trace 文件
            let _ = config.tracing.cleanup_old_traces().await;

            let session_id = generate_session_id();
            let output_path = config.tracing.generate_path(&session_id);

            // 异步获取 Git 信息（避免阻塞）
            let (git_branch, git_commit) = tokio::join!(
                get_git_branch_async(),
                get_git_commit_async()
            );

            match TraceWriter::new(
                output_path,
                config.tracing.buffer_size,
                50, // batch_size
            ).await {
                Ok(writer) => {
                    let writer = Arc::new(writer);

                    // 记录会话开始事件
                    let _ = writer.record(AgentEvent::SessionStart {
                        session_id: session_id.clone(),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        context: SessionContext {
                            engine_version: env!("CARGO_PKG_VERSION").to_string(),
                            working_dir: std::env::current_dir()
                                .ok()
                                .and_then(|p| p.to_str().map(String::from))
                                .unwrap_or_default(),
                            git_branch,
                            git_commit,
                            model: config.model.clone(),
                            config_summary: serde_json::json!({
                                "model": config.model,
                                "max_tokens": config.max_tokens,
                            }),
                        },
                    });

                    Some(writer)
                }
                Err(e) => {
                    eprintln!("Failed to initialize trace writer: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            // ... 现有字段
            trace_writer,
        })
    }

    /// 记录事件（内部方法，带降级策略）
    fn record_event(&self, event: AgentEvent) {
        if let Some(writer) = &self.trace_writer {
            if let Err(e) = writer.record(event) {
                // 降级：只记录错误，不中断主流程
                eprintln!("Trace recording failed: {}", e);
                // 注意：不禁用 trace_writer，继续尝试后续事件
            }
        }
    }

    /// 获取 trace 文件路径
    pub fn trace_path(&self) -> Option<PathBuf> {
        self.trace_writer.as_ref().map(|w| w.output_path().clone())
    }
}

// 在各个方法中调用 record_event
impl ForgeSDK {
    pub async fn chat(&self, message: &str) -> Result<String> {
        // 记录用户消息
        if self.config.tracing.record_messages {
            self.record_event(AgentEvent::UserMessage {
                content: message.to_string(),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
        }

        // ... 执行对话

        // 记录 assistant 响应
        if self.config.tracing.record_messages {
            self.record_event(AgentEvent::AssistantMessage {
                content: response.clone(),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
        }

        Ok(response)
    }
}

// Drop 实现：尽力记录会话结束事件
impl Drop for ForgeSDK {
    fn drop(&mut self) {
        if let Some(writer) = &self.trace_writer {
            // 记录会话结束事件（非阻塞）
            let _ = writer.record(AgentEvent::SessionEnd {
                session_id: self.session_id.clone(),
                timestamp: chrono::Utc::now().timestamp_millis(),
                duration_ms: self.start_time.elapsed().as_millis() as u64,
            });

            // 尝试在 tokio 上下文中异步 flush
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let writer = writer.clone();
                handle.spawn(async move {
                    let _ = writer.flush().await;
                });
            }
            // 注意：如果不在 tokio 上下文，依赖后台任务的 channel 关闭自动 flush
        }
    }
}

// 推荐：提供显式的 shutdown 方法
impl ForgeSDK {
    /// 显式关闭 SDK，确保所有 trace 数据写入完成
    pub async fn shutdown(self) -> Result<()> {
        if let Some(writer) = &self.trace_writer {
            writer.record(AgentEvent::SessionEnd {
                session_id: self.session_id.clone(),
                timestamp: chrono::Utc::now().timestamp_millis(),
                duration_ms: self.start_time.elapsed().as_millis() as u64,
            })?;
            writer.flush().await?;
        }
        Ok(())
    }
}

// 异步辅助函数（带超时）
async fn get_git_branch_async() -> Option<String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::process::Command::new("git")
            .args(&["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
    )
    .await
    .ok()?
    .ok()
    .and_then(|o| String::from_utf8(o.stdout).ok())
    .map(|s| s.trim().to_string())
}

async fn get_git_commit_async() -> Option<String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::process::Command::new("git")
            .args(&["rev-parse", "--short", "HEAD"])
            .output()
    )
    .await
    .ok()?
    .ok()
    .and_then(|o| String::from_utf8(o.stdout).ok())
    .map(|s| s.trim().to_string())
}
```

### 4.5 NAPI 绑定 (`forge-napi/src/sdk.rs` - 可选)

```rust
// 只需要暴露一个方法：获取 trace 文件路径

#[napi]
impl ForgeSDK {
    /// 获取当前会话的 trace 文件路径
    #[napi]
    pub fn get_trace_path(&self) -> Option<String> {
        self.inner
            .blocking_read()
            .as_ref()
            .and_then(|sdk| sdk.trace_path())
            .and_then(|p| p.to_str().map(String::from))
    }
}
```

**说明**：不需要复杂的导出 API，因为文件已经自动写入。这个方法只是方便用户知道文件在哪里。

## 5. 文件格式

### 5.1 JSONL 格式

每行一个 JSON 对象，代表一个事件：

```jsonl
{"type":"SessionStart","session_id":"abc123","timestamp":1234567890,"context":{"engine_version":"0.1.0","working_dir":"/path/to/project","git_branch":"main","git_commit":"a1b2c3d","model":"claude-opus-4","config_summary":{}}}
{"type":"UserMessage","content":"帮我实现一个函数","timestamp":1234567891}
{"type":"ApiRequest","request_id":"req_001","model":"claude-opus-4","timestamp":1234567892}
{"type":"ToolCallStart","id":"tool_001","name":"read_file","input":{"path":"src/main.rs"},"timestamp":1234567893}
{"type":"ToolResult","id":"tool_001","output":"fn main() { ... }","error":null,"duration_ms":50}
{"type":"ApiResponse","request_id":"req_001","duration_ms":2000,"input_tokens":100,"output_tokens":200,"cache_read_tokens":50,"cache_write_tokens":0,"timestamp":1234567894}
{"type":"AssistantMessage","content":"这是实现...","timestamp":1234567895}
{"type":"SessionEnd","session_id":"abc123","timestamp":1234567900,"duration_ms":10000}
```

### 5.2 文件命名

默认格式：`~/.forge/traces/20260304_153000_abc123.jsonl`

- `20260304_153000`：时间戳（年月日_时分秒）
- `abc123`：session_id
- `.jsonl`：扩展名

### 5.3 文件结构

```
~/.forge/traces/
├── 20260304_153000_session1.jsonl
├── 20260304_154500_session2.jsonl
└── 20260304_160000_session3.jsonl
```

## 6. 环境变量

支持通过环境变量覆盖配置：

```bash
# 启用/禁用 tracing
FORGE_TRACING_ENABLED=true

# 输出目录
FORGE_TRACING_OUTPUT_DIR=~/.forge/traces

# 文件名模板
FORGE_TRACING_FILENAME_TEMPLATE="{timestamp}_{session_id}.jsonl"

# 缓冲区大小
FORGE_TRACING_BUFFER_SIZE=100

# 记录对话内容
FORGE_TRACING_RECORD_MESSAGES=true

# 记录工具详情
FORGE_TRACING_RECORD_TOOL_DETAILS=true

# 最大保留文件数量
FORGE_TRACING_MAX_FILES=100

# 最大保留天数
FORGE_TRACING_MAX_AGE_DAYS=30
```

## 7. 实现清单

### 7.1 文件变更

| 文件 | 操作 | 说明 |
|------|------|------|
| `forge-config/src/tracing.rs` | 新增 | `TracingConfig` 配置（含环境变量支持） |
| `forge-config/src/lib.rs` | 修改 | 导出 `tracing` 模块 |
| `forge-domain/src/event.rs` | 修改 | 新增事件类型（`ToolResultDetailed` 等） |
| `forge-domain/src/lib.rs` | 修改 | 导出 `SessionContext` |
| `forge-agent/src/trace_error.rs` | 新增 | `TraceError` 错误类型 |
| `forge-agent/src/trace_writer.rs` | 新增 | `TraceWriter` 实现（批量写入） |
| `forge-agent/src/lib.rs` | 修改 | 导出 `trace_writer` 和 `trace_error` 模块 |
| `forge-sdk/src/sdk/mod.rs` | 修改 | 集成 `TraceWriter`（异步 Git 命令） |
| `forge-napi/src/sdk.rs` | 修改 | 暴露 `get_trace_path()` |

### 7.2 依赖添加

```toml
# Cargo.toml [workspace.dependencies]
chrono = "0.4"       # 时间戳生成
thiserror = "2.0"    # 错误处理（已有）
dirs = "5.0"         # 目录路径
```

## 8. 实现优先级

### Phase 1（核心功能 - 必须完成）
1. ✅ `TracingConfig` 配置模块（含环境变量支持）
2. ✅ `TraceError` 错误类型定义
3. ✅ 扩展 `AgentEvent`（新增 `ToolResultDetailed` 等）
4. ✅ `TraceWriter` 异步写入器（批量写入、正确的并发控制）
5. ✅ SDK 集成（自动记录、异步 Git 命令）
6. ✅ 基本单元测试（`TraceWriter` 并发安全性）

### Phase 2（完善功能）
1. NAPI 绑定（`get_trace_path()`）
2. 集成测试（完整会话的 trace 正确性）
3. 错误处理优化（降级策略）
4. 文档和使用示例

### Phase 3（优化和扩展）
1. 性能基准测试（验证 < 5% 影响）
2. 文件轮转机制（避免单文件过大）
3. 压缩支持（可选）
4. 采样模式（高频场景）

## 9. 使用示例

### 9.1 配置文件

```json
{
  "tracing": {
    "enabled": true,
    "output_dir": "~/.forge/traces",
    "filename_template": "{timestamp}_{session_id}.jsonl",
    "buffer_size": 100,
    "record_messages": true,
    "record_tool_details": true
  }
}
```

### 9.2 Node.js 使用

```javascript
const { ForgeSDK } = require('@forge/sdk');

// 初始化（自动开始记录）
const sdk = new ForgeSDK({
  tracing: {
    enabled: true
  }
});

// 执行对话（自动记录所有事件）
await sdk.chat("帮我实现一个函数");

// 获取 trace 文件路径
const tracePath = sdk.getTracePath();
console.log(`Trace saved to: ${tracePath}`);

// 会话结束时自动 flush
```

### 9.3 Claude Code 分析

```bash
# 1. 找到 trace 文件
ls ~/.forge/traces/

# 2. 用 Claude Code 分析
# 在 Claude Code 中：
# "请分析这个 trace 文件，找出 forge-engine 的性能瓶颈和潜在问题"
# 然后拖入 trace 文件
```

## 10. 验收标准

1. ✅ 默认启用时，自动创建 trace 文件
2. ✅ 记录完整对话内容（user + assistant）
3. ✅ 记录所有工具调用的输入输出（使用 `ToolResultDetailed`）
4. ✅ 记录所有 API 请求和响应
5. ✅ 记录错误详情和上下文
6. ✅ JSONL 格式，每行一个事件
7. ✅ 异步写入，性能影响 < 5%
8. ✅ 会话结束时自动 flush（Drop 实现）
9. ✅ 支持环境变量配置
10. ✅ Claude Code 可以直接读取分析
11. ✅ 错误降级策略（trace 失败不中断主流程）
12. ✅ 自动清理旧文件（按数量和时间）
13. ✅ 并发安全（多个 SDK 实例独立文件）
14. ✅ 完整的单元测试和集成测试

## 11. 与原方案对比

| 特性 | 原方案 | 新方案 |
|------|--------|--------|
| 文件输出 | 可选（`Option<PathBuf>`） | 默认启用 |
| 导出接口 | 需要手动调用 `export_trace()` | 自动写入，无需导出 |
| 对话内容 | 不记录（隐私考虑） | 默认记录 |
| 工具输出 | 可配置关闭 | 默认记录完整输出 |
| 内存缓冲 | 环形缓冲（10000 事件） | 批量写入缓冲（100 事件） |
| 统计聚合 | `SessionStats` 模块 | 移除（Claude Code 分析） |
| 查询 API | `get_session_stats()` 等 | 移除（直接读文件） |
| 文件格式 | 单个 JSON | JSONL（流式） |
| 复杂度 | 高（多个模块） | 低（最小化） |

## 12. 关键设计决策

1. **默认写文件 vs 可选导出**
   - 选择：默认写文件
   - 理由：使用场景是开发分析，应该自动记录

2. **JSONL vs JSON**
   - 选择：JSONL
   - 理由：大文件友好，流式处理，易于追加

3. **完整记录 vs 隐私过滤**
   - 选择：完整记录
   - 理由：开发工具，不是生产环境

4. **内存聚合 vs 文件直读**
   - 选择：文件直读
   - 理由：Claude Code 会做分析，不需要内置聚合

5. **同步 vs 异步写入**
   - 选择：异步写入
   - 理由：性能要求，避免阻塞主流程

6. **修改现有事件 vs 新增事件类型**
   - 选择：新增 `ToolResultDetailed`
   - 理由：避免破坏性变更，保持向后兼容

7. **单独写入 vs 批量写入**
   - 选择：批量写入（50 事件/批）
   - 理由：减少 I/O 次数，提升性能

8. **eprintln! vs Result**
   - 选择：返回 `Result<(), TraceError>`
   - 理由：符合项目规范，便于错误处理

9. **同步 Git 命令 vs 异步**
   - 选择：异步 `tokio::process::Command`
   - 理由：避免阻塞 SDK 初始化

10. **环境变量支持**
    - 选择：通过 `from_env()` 方法覆盖配置
    - 理由：灵活性，便于 CI/CD 环境配置

## 13. 注意事项

1. **文件大小**：长会话可能产生大文件（数 MB），需要考虑清理策略
2. **敏感信息**：开发环境可能包含敏感数据，提醒用户不要分享 trace 文件
3. **性能影响**：虽然异步写入，但序列化仍有开销，需要实测
4. **错误处理**：文件写入失败时应该降级（记录错误但不中断主流程）
5. **并发安全**：多个 SDK 实例不应写入同一文件（通过 session_id 区分）
6. **资源清理**：SDK Drop 时应该调用 `flush()` 确保数据写入完成

## 14. 测试策略

### 14.1 单元测试

```rust
// forge-agent/tests/trace_writer_test.rs

#[tokio::test]
async fn test_trace_writer_basic() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("test.jsonl");

    let writer = TraceWriter::new(path.clone(), 10, 5).await.unwrap();

    // 写入事件
    writer.record(AgentEvent::UserMessage {
        content: "test".to_string(),
        timestamp: 0,
    }).unwrap();

    // Flush
    writer.flush().await.unwrap();

    // 验证文件内容
    let content = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(content.contains("test"));
}

#[tokio::test]
async fn test_trace_writer_batch() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("test.jsonl");

    let writer = TraceWriter::new(path.clone(), 100, 5).await.unwrap();

    // 写入多个事件（触发批量写入）
    for i in 0..10 {
        writer.record(AgentEvent::UserMessage {
            content: format!("message_{}", i),
            timestamp: i as i64,
        }).unwrap();
    }

    writer.flush().await.unwrap();

    // 验证所有事件都写入
    let content = tokio::fs::read_to_string(&path).await.unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 10);
}

#[tokio::test]
async fn test_trace_writer_concurrent() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("test.jsonl");

    let writer = Arc::new(TraceWriter::new(path.clone(), 100, 10).await.unwrap());

    // 并发写入
    let mut handles = vec![];
    for i in 0..10 {
        let writer = writer.clone();
        let handle = tokio::spawn(async move {
            for j in 0..10 {
                let _ = writer.record(AgentEvent::UserMessage {
                    content: format!("thread_{}_msg_{}", i, j),
                    timestamp: 0,
                });
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }

    writer.flush().await.unwrap();

    // 验证所有事件都写入
    let content = tokio::fs::read_to_string(&path).await.unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 100);
}

#[tokio::test]
async fn test_channel_full() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("test.jsonl");

    // 小缓冲区
    let writer = TraceWriter::new(path.clone(), 1, 10).await.unwrap();

    // 快速写入多个事件
    let mut errors = 0;
    for i in 0..100 {
        if writer.record(AgentEvent::UserMessage {
            content: format!("msg_{}", i),
            timestamp: 0,
        }).is_err() {
            errors += 1;
        }
    }

    // 应该有一些失败（channel full）
    assert!(errors > 0);
}

#[tokio::test]
async fn test_writer_drop_before_flush() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("test.jsonl");

    {
        let writer = TraceWriter::new(path.clone(), 100, 5).await.unwrap();
        writer.record(AgentEvent::UserMessage {
            content: "test".to_string(),
            timestamp: 0,
        }).unwrap();
        // Drop 时应该自动 flush
    }

    // 等待异步任务完成
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 验证文件内容
    let content = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(content.contains("test"));
}
```

### 14.2 集成测试

```rust
// forge-sdk/tests/tracing_integration_test.rs

#[tokio::test]
async fn test_full_session_trace() {
    let temp_dir = tempfile::tempdir().unwrap();

    let config = ForgeConfig {
        tracing: TracingConfig {
            enabled: true,
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };

    let sdk = ForgeSDK::new(config).await.unwrap();

    // 执行对话
    let _ = sdk.chat("test message").await;

    // 获取 trace 路径
    let trace_path = sdk.trace_path().unwrap();

    // 显式 flush
    drop(sdk);

    // 等待异步写入完成
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 验证文件存在且包含预期事件
    let content = tokio::fs::read_to_string(&trace_path).await.unwrap();
    assert!(content.contains("SessionStart"));
    assert!(content.contains("UserMessage"));
    assert!(content.contains("SessionEnd"));
}

#[tokio::test]
async fn test_concurrent_sdk_instances() {
    let temp_dir = tempfile::tempdir().unwrap();

    let mut handles = vec![];

    for i in 0..5 {
        let dir = temp_dir.path().to_path_buf();
        let handle = tokio::spawn(async move {
            let config = ForgeConfig {
                tracing: TracingConfig {
                    enabled: true,
                    output_dir: dir,
                    ..Default::default()
                },
                ..Default::default()
            };

            let sdk = ForgeSDK::new(config).await.unwrap();
            let _ = sdk.chat(&format!("message {}", i)).await;
            sdk.trace_path()
        });
        handles.push(handle);
    }

    let mut paths = vec![];
    for handle in handles {
        if let Ok(Some(path)) = handle.await {
            paths.push(path);
        }
    }

    // 验证每个实例都有独立的文件
    assert_eq!(paths.len(), 5);
    let unique_paths: std::collections::HashSet<_> = paths.into_iter().collect();
    assert_eq!(unique_paths.len(), 5);
}

#[tokio::test]
async fn test_trace_cleanup() {
    let temp_dir = tempfile::tempdir().unwrap();

    // 创建一些旧文件
    for i in 0..10 {
        let path = temp_dir.path().join(format!("old_{}.jsonl", i));
        tokio::fs::write(&path, "test").await.unwrap();
    }

    let config = TracingConfig {
        enabled: true,
        output_dir: temp_dir.path().to_path_buf(),
        max_trace_files: Some(5),
        ..Default::default()
    };

    // 清理
    config.cleanup_old_traces().await.unwrap();

    // 验证只保留 5 个文件
    let mut count = 0;
    let mut entries = tokio::fs::read_dir(temp_dir.path()).await.unwrap();
    while let Some(_) = entries.next_entry().await.unwrap() {
        count += 1;
    }
    assert_eq!(count, 5);
}
```

### 14.3 性能测试

```rust
// forge-agent/benches/trace_writer_bench.rs

use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_trace_writer(c: &mut Criterion) {
    c.bench_function("trace_writer_record", |b| {
        // 测试记录性能
    });
}
```

## 14. 未来扩展

1. **采样模式**：高频场景下只记录部分事件
2. **过滤规则**：配置哪些事件类型要记录
3. **压缩支持**：自动压缩旧文件（.jsonl.gz）
4. **远程上传**：可选的自动上传到分析服务
5. **实时查看**：提供 CLI 工具实时查看 trace


