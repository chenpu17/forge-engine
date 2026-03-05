use forge_config::TracingConfig;
use std::env;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_from_env() {
    env::set_var("FORGE_TRACING_ENABLED", "false");
    env::set_var("FORGE_TRACING_BUFFER_SIZE", "200");

    let config = TracingConfig::default().from_env();

    assert!(!config.enabled);
    assert_eq!(config.buffer_size, 200);

    env::remove_var("FORGE_TRACING_ENABLED");
    env::remove_var("FORGE_TRACING_BUFFER_SIZE");
}

#[test]
fn test_generate_path_sanitization() {
    let temp_dir = TempDir::new().unwrap();
    let mut config = TracingConfig::default();
    config.output_dir = temp_dir.path().to_path_buf();

    let path = config.generate_path("test/../../../etc/passwd");
    let filename = path.file_name().unwrap().to_str().unwrap();

    assert!(!filename.contains(".."));
    assert!(!filename.contains("/"));
}

#[test]
fn test_generate_path_empty_session_id() {
    let temp_dir = TempDir::new().unwrap();
    let mut config = TracingConfig::default();
    config.output_dir = temp_dir.path().to_path_buf();

    let path = config.generate_path("@#$%^&*()");
    let filename = path.file_name().unwrap().to_str().unwrap();

    assert!(filename.contains("unknown"));
}

#[tokio::test]
async fn test_cleanup_old_traces() {
    let temp_dir = TempDir::new().unwrap();
    let mut config = TracingConfig::default();
    config.output_dir = temp_dir.path().to_path_buf();
    config.max_trace_files = Some(2);

    fs::create_dir_all(&config.output_dir).unwrap();
    fs::write(config.output_dir.join("old1.jsonl"), "test").unwrap();
    fs::write(config.output_dir.join("old2.jsonl"), "test").unwrap();
    fs::write(config.output_dir.join("old3.jsonl"), "test").unwrap();

    let _ = config.cleanup_old_traces().await;

    let files: Vec<_> = fs::read_dir(&config.output_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();

    assert!(files.len() <= 2);
}
