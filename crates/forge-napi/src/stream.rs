//! Stream handling utilities for NAPI

#![allow(dead_code)]

use crate::events::JsAgentEvent;
use futures::StreamExt;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use std::pin::Pin;

/// Type alias for the event stream
pub type EventStream = Pin<Box<dyn futures::Stream<Item = forge_sdk::AgentEvent> + Send>>;

/// Process an event stream and call the callback for each event
pub async fn process_stream_with_callback(
    mut stream: EventStream,
    callback: ThreadsafeFunction<JsAgentEvent>,
) -> napi::Result<()> {
    while let Some(event) = stream.next().await {
        let js_event: JsAgentEvent = event.into();
        let is_terminal = js_event.is_terminal();
        callback.call(Ok(js_event), ThreadsafeFunctionCallMode::NonBlocking);
        if is_terminal {
            break;
        }
    }
    Ok(())
}
