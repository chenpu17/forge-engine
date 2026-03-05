use forge_agent::trace_writer::TraceWriter;
use forge_domain::AgentEvent;
use tempfile::TempDir;

#[tokio::test]
async fn test_trace_writer_basic() {
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("test_trace.jsonl");

    let writer = TraceWriter::new(output_path.clone(), 100, 10).await.unwrap();

    writer.record(AgentEvent::UserMessage {
        content: "Hello".to_string(),
        timestamp: 1000,
    }).unwrap();

    writer.record(AgentEvent::AssistantMessage {
        content: "Hi there!".to_string(),
        timestamp: 2000,
    }).unwrap();

    writer.shutdown().await.unwrap();

    let content = std::fs::read_to_string(&output_path).unwrap();
    assert!(content.contains("user_message"));
    assert!(content.contains("Hello"));
    assert!(content.contains("assistant_message"));
    assert!(content.contains("Hi there!"));
}

#[tokio::test]
async fn test_channel_full() {
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("test_full.jsonl");

    let writer = TraceWriter::new(output_path.clone(), 2, 10).await.unwrap();

    // Fill channel
    writer.record(AgentEvent::UserMessage {
        content: "1".to_string(),
        timestamp: 1000,
    }).unwrap();
    writer.record(AgentEvent::UserMessage {
        content: "2".to_string(),
        timestamp: 2000,
    }).unwrap();

    // Should fail when full
    let result = writer.record(AgentEvent::UserMessage {
        content: "3".to_string(),
        timestamp: 3000,
    });
    assert!(result.is_err());

    writer.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_record_async_waits() {
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("test_async.jsonl");

    let writer = TraceWriter::new(output_path.clone(), 2, 10).await.unwrap();

    // record_async should wait instead of failing
    writer.record_async(AgentEvent::UserMessage {
        content: "1".to_string(),
        timestamp: 1000,
    }).await.unwrap();
    writer.record_async(AgentEvent::UserMessage {
        content: "2".to_string(),
        timestamp: 2000,
    }).await.unwrap();
    writer.record_async(AgentEvent::UserMessage {
        content: "3".to_string(),
        timestamp: 3000,
    }).await.unwrap();

    writer.shutdown().await.unwrap();

    let content = std::fs::read_to_string(&output_path).unwrap();
    assert!(content.contains("\"1\""));
    assert!(content.contains("\"2\""));
    assert!(content.contains("\"3\""));
}
