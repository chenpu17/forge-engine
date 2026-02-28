//! Workflow NAPI bindings

use napi_derive::napi;
use forge_workflow::Position;

/// Position in the workflow editor
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsPosition {
    pub x: f64,
    pub y: f64,
}

impl From<Position> for JsPosition {
    fn from(p: Position) -> Self {
        Self { x: p.x, y: p.y }
    }
}

impl From<JsPosition> for Position {
    fn from(p: JsPosition) -> Self {
        Self { x: p.x, y: p.y }
    }
}
