//! Unix shell tools

mod bash;
mod dangerous;

pub use bash::BashTool;
pub use dangerous::UnixDangerousCommands;

use super::ShellExecutor;

/// Unix shell executor (bash)
pub struct BashExecutor;

impl ShellExecutor for BashExecutor {
    #[allow(clippy::unnecessary_literal_bound)]
    fn program(&self) -> &str {
        "bash"
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn command_arg(&self) -> &str {
        "-c"
    }
}
