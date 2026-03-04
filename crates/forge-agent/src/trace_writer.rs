//! Asynchronous trace writer for session recording.

use forge_domain::AgentEvent;
use std::path::PathBuf;
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

use crate::trace_error::{Result, TraceError};

/// Message type for trace writer.
enum TraceWriterMessage {
    /// Record an event.
    Event(AgentEvent),
    /// Flush and return confirmation.
    Flush(oneshot::Sender<()>),
}

/// Asynchronous trace writer.
pub struct TraceWriter {
    /// Send channel.
    tx: mpsc::Sender<TraceWriterMessage>,
    /// Output file path.
    output_path: PathBuf,
}

impl TraceWriter {
    /// Create and start trace writer.
    pub async fn new(
        output_path: PathBuf,
        buffer_size: usize,
        batch_size: usize,
    ) -> Result<Self> {
        // Ensure directory exists
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Open file in append mode
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&output_path)
            .await?;

        // Create channel
        let (tx, mut rx) = mpsc::channel::<TraceWriterMessage>(buffer_size);

        // Clone path for error messages
        let error_path = output_path.clone();

        // Start background writer task
        tokio::spawn(async move {
            let mut event_buffer = Vec::with_capacity(batch_size);

            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Some(TraceWriterMessage::Event(event)) => {
                                event_buffer.push(event);

                                // Batch write when buffer is full
                                if event_buffer.len() >= batch_size {
                                    if let Err(e) = write_batch(&mut file, &event_buffer).await {
                                        eprintln!("Failed to write trace batch to {:?}: {}", error_path, e);
                                    }
                                    event_buffer.clear();
                                }
                            }
                            Some(TraceWriterMessage::Flush(tx)) => {
                                // Write remaining events
                                if !event_buffer.is_empty() {
                                    if let Err(e) = write_batch(&mut file, &event_buffer).await {
                                        eprintln!("Failed to write trace batch to {:?}: {}", error_path, e);
                                    }
                                    event_buffer.clear();
                                }
                                // Flush file
                                let _ = file.flush().await;
                                // Send confirmation
                                let _ = tx.send(());
                            }
                            None => {
                                // Channel closed, write remaining events and exit
                                if !event_buffer.is_empty() {
                                    if let Err(e) = write_batch(&mut file, &event_buffer).await {
                                        eprintln!("Failed to write final trace batch to {:?}: {}", error_path, e);
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

    /// Record an event (non-blocking).
    pub fn record(&self, event: AgentEvent) -> Result<()> {
        self.tx
            .try_send(TraceWriterMessage::Event(event))
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => TraceError::ChannelFull,
                mpsc::error::TrySendError::Closed(_) => TraceError::ChannelClosed,
            })
    }

    /// Get output file path.
    pub fn output_path(&self) -> &PathBuf {
        &self.output_path
    }

    /// Wait for all events to be written.
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

/// Write a batch of events to file.
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
