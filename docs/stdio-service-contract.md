# STDIO Service Contract

## Overview

STDIO services are stateless external processes that communicate via JSON on stdin/stdout. The engine spawns a fresh child process per call, writes a JSON request to stdin, closes stdin to signal EOF, waits for the process to exit, and reads JSON from stdout.

This contract is used for format conversion, text preprocessing, entity extraction, and other transforms that don't require persistent state. For services that need persistent connections (model inference, long-running servers), use HTTP transport instead.

## Health Check

Every STDIO service must respond to a health check ping.

**Input:**
```json
{"ping": true}
```

**Output:**
```json
{
  "pong": true,
  "name": "my-service",
  "version": "1.0.0"
}
```

The response may include additional fields (e.g., `languages`, `capabilities`). The engine only checks that the output is valid JSON and the process exits with code 0.

The engine calls this health check during startup validation via `ServiceRegistry.validate_all()`. If it fails, the service is disabled with an ERROR log. The engine does not silently degrade.

## Extraction Contract

The primary use case is entity extraction from source code. The contract is as follows.

### Input

```json
{
  "source_code": "fn main() { println!(\"hello\"); }",
  "language": "rust",
  "file_path": "src/main.rs"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `source_code` | string | yes | The source code to extract from |
| `language` | string | no | Language hint (e.g. `"rust"`, `"python"`, `"go"`). If absent, detected from `file_path` extension |
| `file_path` | string | no | Original file path. Used for language detection and metadata |

### Output

```json
{
  "entities": [
    {
      "name": "main",
      "entity_type": "function",
      "description": "fn main()",
      "confidence": 1.0,
      "metadata": {"ast_hash": "a1b2c3"}
    }
  ],
  "relationships": [
    {
      "source": "main",
      "target": "println",
      "rel_type": "calls",
      "description": "Function call",
      "confidence": 1.0
    }
  ],
  "language": "rust",
  "file_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
}
```

#### Entity Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Entity name as extracted |
| `entity_type` | string | yes | Type identifier (e.g. `"function"`, `"struct"`, `"class"`) |
| `description` | string | no | Human-readable description (signature, fields, etc.) |
| `confidence` | float | yes | Extraction confidence, 0.0 to 1.0. AST extraction is typically 1.0 |
| `metadata` | object | no | Arbitrary metadata (e.g. `ast_hash` for incremental detection) |

#### Relationship Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `source` | string | yes | Name of the source entity |
| `target` | string | yes | Name of the target entity |
| `rel_type` | string | yes | Relationship type identifier (e.g. `"calls"`, `"implements"`, `"imports"`) |
| `description` | string | no | Optional description |
| `confidence` | float | yes | Extraction confidence, 0.0 to 1.0 |

#### Top-Level Response Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `entities` | array | yes | List of extracted entities |
| `relationships` | array | yes | List of extracted relationships |
| `language` | string | yes | Detected or specified language |
| `file_hash` | string | yes | SHA-256 hash of the input source code |

## Error Handling

- **Non-zero exit code** = failure. The engine treats any non-zero exit as an error and reads stderr for diagnostics.
- **stderr** is for diagnostics only. Log messages, warnings, and error details go to stderr. The engine captures stderr and includes it in error messages.
- **stdout** must be valid JSON or empty. If the process exits with code 0 but stdout is not valid JSON, the engine treats it as an error.
- On error, the service may write a JSON error object to stdout before exiting non-zero:

```json
{"error": "unsupported language: brainfuck"}
```

This is optional -- the engine does not parse error JSON. It uses the exit code and stderr for error reporting.

## Timeout

- Default timeout: **30 seconds**.
- Configurable per transport instance via `StdioTransport::with_timeout()`.
- If the child process does not exit within the timeout, the engine kills it and reports a timeout error.

## Process Lifecycle

- A **fresh process is spawned per call**. There is no process pooling or reuse.
- Stdin is written to and then closed (EOF) before waiting for the process.
- The engine waits for the process to exit, then reads all of stdout.
- If the process hangs (no exit within timeout), it is killed.

This stateless model means services do not need to handle multiple requests, manage connections, or maintain state between calls. Each invocation is independent.

## Example

Test a STDIO service manually:

```bash
# Health check
echo '{"ping": true}' | covalence-ast-extractor
# Output: {"pong":true,"name":"covalence-ast-extractor","version":"0.1.0","languages":["rust","python","go"]}

# Extract entities from Rust code
echo '{"source_code": "pub fn add(a: i32, b: i32) -> i32 { a + b }", "language": "rust"}' \
  | covalence-ast-extractor
# Output: {"entities":[{"name":"add","entity_type":"function",...}],"relationships":[],...}
```

## Implementing a New STDIO Service

1. Read all of stdin as a single JSON object.
2. If the input contains `"ping": true`, respond with a pong and exit 0.
3. Otherwise, process the input and write a JSON response to stdout.
4. Exit 0 on success, non-zero on error.
5. Write diagnostic messages to stderr, never to stdout.
6. Register the service in your extension manifest:

```yaml
service:
  name: my-extractor
  transport: stdio
  command: my-extractor-binary
  args: ["--format", "json"]
```

The engine validates the service at startup by sending a ping. If the binary is not found or the ping fails, the service is disabled with an error log.
