# Application
app-name = Forge
app-version = v{ $version }

# Status
status-ready = Ready
status-thinking = Thinking...
status-streaming = Receiving...
status-tool-running = Running: { $tool }
status-waiting = Waiting...
status-error = Error

# Input
input-placeholder = Type your message...
input-send = Send
input-cancel = Cancel
input-history-search = Search history

# Messages
message-user = You
message-assistant = Forge

# Tools
tool-executing = Executing...
tool-completed = Completed ({ $duration }s)
tool-failed = Failed
tool-confirm-title = Confirm execution?
tool-confirm-yes = Yes
tool-confirm-no = No
tool-confirm-edit = Edit
tool-confirm-always = Always

# Help
help-title = Keyboard Shortcuts
help-input = Input
help-navigation = Navigation
help-general = General
help-tools = Tools
help-press-to-close = Press [Esc] to close

# Footer
footer-tokens = Tokens: { $used } / { $limit }
footer-help = [?] Help
footer-stop = [Ctrl+C] Stop

# Errors
error-network = Network Error: { $message }
error-api = API Error: { $message }
error-timeout = Request timed out
error-context-limit = Context limit approaching: { $percent }%
error-tool-not-found = Tool not found: { $tool }
error-permission-denied = Permission denied: { $message }
error-rate-limited = Rate limited. Please wait.
error-max-iterations = Max iterations exceeded
error-consecutive-failures = Too many consecutive failures

# Recovery
recovery-retry = Retrying after { $delay }ms
recovery-alternative = Trying alternative approach
recovery-continue = Continuing despite error
recovery-stop = Stopping: { $reason }

# Welcome
welcome-title = Welcome to Forge!
welcome-description = Forge is an AI-powered coding assistant that helps you write, understand, and refactor code through natural language conversation.
welcome-quick-start = Quick Start:
welcome-tip-send = Type your question and press Enter
welcome-tip-interrupt = Use Ctrl+C to interrupt
welcome-tip-help = Press ? for help
welcome-current-dir = Current directory: { $dir }
welcome-model = Model: { $model }

# History Search
history-search-prompt = (reverse-i-search)
history-search-no-match = (no match)
history-search-title = History Search (Ctrl+R)
