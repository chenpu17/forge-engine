use forge_agent::trace_writer::TraceWriter;
use forge_domain::AgentEvent;
use tempfile::TempDir;

#[tokio::test]
async fn test_trace_writer_basic() {
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("test_trace.jsonl");

    let writer = TraceWriter::new(output_path.clone(), 100, 10).await.unwrap();

    // Record events
    writer.record(AgentEvent::UserMessage {
        content: "Hello".to_string(),
        timestamp: 1000,
    }).unwrap();

    writer.record(AgentEvent::AssistantMessage {
        content: "Hi there!".to_string(),
        timestamp: 2000,
    }).unwrap();

    // Shutdown to flush all events
    writer.shutdown().await.unwrap();

    // Verify file exists
    assert!(output_path.exists(), "Trace file not created");

    // Read and verify content
    let content = std::fs::read_to_string(&output_path).unwrap();
    println!("File content:\n{}", content);

    assert!(!content.is_empty(), "Trace file is empty");
    assert!(content.contains("user_message"), "Missing user_message event");
    assert!(content.contains("Hello"), "Missing user message content");
    assert!(content.contains("assistant_message"), "Missing assistant_message event");
    assert!(content.contains("Hi there!"), "Missing assistant message content");
}
