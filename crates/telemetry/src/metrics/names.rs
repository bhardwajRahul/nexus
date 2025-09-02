//! Standard metric names following OpenTelemetry semantic conventions
//! See: https://opentelemetry.io/docs/specs/semconv/http/http-metrics/

/// HTTP server request duration in milliseconds
/// Note: Histograms automatically provide count and sum, so a separate counter is not needed
pub const HTTP_SERVER_REQUEST_DURATION: &str = "http.server.request.duration";

/// MCP tool call duration in milliseconds
/// Tracks the duration of MCP tool invocations including both built-in and downstream tools
pub const MCP_TOOL_CALL_DURATION: &str = "mcp.tool.call.duration";

/// MCP tools listing duration in milliseconds
/// Tracks the duration of listing available tools
pub const MCP_TOOLS_LIST_DURATION: &str = "mcp.tools.list.duration";

/// MCP prompt request duration in milliseconds
/// Tracks the duration of prompt-related operations (list/get)
pub const MCP_PROMPT_REQUEST_DURATION: &str = "mcp.prompt.request.duration";

/// MCP resource request duration in milliseconds  
/// Tracks the duration of resource-related operations (list/read)
pub const MCP_RESOURCE_REQUEST_DURATION: &str = "mcp.resource.request.duration";

/// LLM operation duration in milliseconds
/// Tracks the total duration of LLM chat completion operations
/// Follows OpenTelemetry GenAI semantic conventions
pub const GEN_AI_CLIENT_OPERATION_DURATION: &str = "gen_ai.client.operation.duration";

/// LLM input token usage counter
/// Tracks cumulative input token consumption for LLM operations
pub const GEN_AI_CLIENT_INPUT_TOKEN_USAGE: &str = "gen_ai.client.input.token.usage";

/// LLM output token usage counter  
/// Tracks cumulative output token consumption for LLM operations
pub const GEN_AI_CLIENT_OUTPUT_TOKEN_USAGE: &str = "gen_ai.client.output.token.usage";

/// LLM total token usage counter
/// Tracks cumulative total token consumption for LLM operations (input + output)
pub const GEN_AI_CLIENT_TOTAL_TOKEN_USAGE: &str = "gen_ai.client.total.token.usage";

/// Time to first token in milliseconds (streaming only)
/// Tracks the duration until the first token is received in a streaming response
pub const GEN_AI_CLIENT_TIME_TO_FIRST_TOKEN: &str = "gen_ai.client.time_to_first_token";
