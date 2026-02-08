# HTTP API

Duragent exposes an HTTP API for programmatic access to agents and sessions.

## Authentication

Non-admin API routes require token authentication. Pass the token via the `Authorization` header:

```bash
curl -H "Authorization: Bearer YOUR_TOKEN" http://localhost:8080/api/v1/agents
```

## Response Format

### Success

Success responses return data directly as JSON:

```json
{
  "session_id": "session_abc123",
  "agent": "my-assistant",
  "status": "active"
}
```

### Errors (RFC 7807)

Errors use [RFC 7807 Problem Details](https://datatracker.ietf.org/doc/html/rfc7807) with `Content-Type: application/problem+json`:

```json
{
  "type": "urn:duragent:problem:not-found",
  "title": "Not Found",
  "status": 404,
  "detail": "Agent 'my-agent' not found"
}
```

## Public API

### Agents

```
GET  /api/v1/agents                         # List loaded agents
GET  /api/v1/agents/{name}                  # Get agent details (includes full spec)
```

### Sessions

```
GET    /api/v1/sessions                       # List all sessions
POST   /api/v1/sessions                       # Create new session
GET    /api/v1/sessions/{session_id}          # Get session details
DELETE /api/v1/sessions/{session_id}          # End session

GET    /api/v1/sessions/{session_id}/messages # Get message history
POST   /api/v1/sessions/{session_id}/messages # Send message
POST   /api/v1/sessions/{session_id}/stream   # SSE stream

POST   /api/v1/sessions/{session_id}/messages/{message_id}/approve  # Approve tool execution
```

### Health

```
GET  /livez                                 # Liveness check
GET  /readyz                                # Readiness check
GET  /version                               # Version info
```

## Admin API

The Admin API requires authentication via `admin_token` in the server config.

```
POST   /api/admin/v1/shutdown                 # Graceful server shutdown
```

## SSE Streaming

Send a message and stream the response token-by-token:

```bash
curl -N -X POST http://localhost:8080/api/v1/sessions/{session_id}/stream \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"content": "Your message here"}'
```

### Event Types

**token** — Partial response token:
```
event: token
data: {"content": "Hello"}
```

**done** — Stream completed:
```
event: done
data: {"usage": {"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18}}
```

**error** — Error occurred:
```
event: error
data: {"message": "LLM request failed: ..."}
```

**approval_required** — Tool needs user approval:
```
event: approval_required
data: {"call_id": "call_abc123", "command": "npm install sqlite3"}
```

## Examples

### Create and Use a Session

```bash
# Create a session
curl -X POST http://localhost:8080/api/v1/sessions \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"agent": "my-assistant"}'

# Send a message
curl -X POST http://localhost:8080/api/v1/sessions/session_abc123/messages \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"content": "Hello, how are you?"}'

# Stream a response
curl -N -X POST http://localhost:8080/api/v1/sessions/session_abc123/stream \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"content": "Write a short poem"}'
```

## Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `not_found` | 404 | Resource not found |
| `invalid_request` | 400 | Malformed request |
| `unauthorized` | 401 | Missing or invalid auth |
| `forbidden` | 403 | Insufficient permissions |
| `conflict` | 409 | Resource conflict |
| `internal_error` | 500 | Server error |
| `session_not_found` | 404 | Session does not exist |
| `session_ended` | 410 | Session has been ended |
| `agent_not_found` | 404 | Agent does not exist |
