//! NAPI Error handling

/// NAPI-specific error wrapper
pub struct NapiError(String);

impl NapiError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl From<forge_sdk::ForgeError> for NapiError {
    fn from(err: forge_sdk::ForgeError) -> Self {
        let mut payload = err.machine_payload();
        if let serde_json::Value::Object(obj) = &mut payload {
            let event_type = if err.is_rate_limited() {
                "rate_limited"
            } else if err.is_retryable() {
                "retryable"
            } else {
                "sdk_error"
            };
            obj.insert("type".to_string(), serde_json::Value::String(event_type.to_string()));
        }
        NapiError(payload.to_string())
    }
}

impl From<NapiError> for napi::Error {
    fn from(err: NapiError) -> Self {
        napi::Error::from_reason(err.0)
    }
}
