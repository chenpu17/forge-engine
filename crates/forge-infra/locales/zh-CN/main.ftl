# Application
app-name = Forge
app-version = v{ $version }

# Status
status-ready = 就绪
status-thinking = 思考中...
status-streaming = 接收中...
status-tool-running = 运行中: { $tool }
status-waiting = 等待中...
status-error = 错误

# Input
input-placeholder = 输入消息...
input-send = 发送
input-cancel = 取消
input-history-search = 搜索历史

# Messages
message-user = 用户
message-assistant = Forge

# Tools
tool-executing = 执行中...
tool-completed = 完成 ({ $duration }秒)
tool-failed = 失败
tool-confirm-title = 确认执行?
tool-confirm-yes = 是
tool-confirm-no = 否
tool-confirm-edit = 编辑
tool-confirm-always = 始终允许

# Help
help-title = 快捷键
help-input = 输入
help-navigation = 导航
help-general = 通用
help-tools = 工具
help-press-to-close = 按 [Esc] 关闭

# Footer
footer-tokens = Tokens: { $used } / { $limit }
footer-help = [?] 帮助
footer-stop = [Ctrl+C] 停止

# Errors
error-network = 网络错误: { $message }
error-api = API 错误: { $message }
error-timeout = 请求超时
error-context-limit = 上下文接近限制: { $percent }%
error-tool-not-found = 工具未找到: { $tool }
error-permission-denied = 权限被拒绝: { $message }
error-rate-limited = 请求频率过高，请稍等
error-max-iterations = 已达到最大迭代次数
error-consecutive-failures = 连续失败次数过多

# Recovery
recovery-retry = { $delay }毫秒后重试
recovery-alternative = 尝试替代方案
recovery-continue = 忽略错误继续执行
recovery-stop = 停止: { $reason }

# Welcome
welcome-title = 欢迎使用 Forge!
welcome-description = Forge 是一个 AI 编程助手，通过自然语言对话帮助你编写、理解和重构代码。
welcome-quick-start = 快速开始:
welcome-tip-send = 输入问题并按 Enter 发送
welcome-tip-interrupt = 使用 Ctrl+C 中断
welcome-tip-help = 按 ? 显示帮助
welcome-current-dir = 当前目录: { $dir }
welcome-model = 模型: { $model }

# History Search
history-search-prompt = (反向搜索)
history-search-no-match = (无匹配)
history-search-title = 历史搜索 (Ctrl+R)
