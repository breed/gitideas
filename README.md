# gitideas

A REST and MCP server that stores ideas, todos, and memories in a git repository. Entries are appended to plain-text files, committed, and pushed — giving you full version history for free.

## Features

- **REST API** for adding and searching entries (`POST /add`, `POST /search`)
- **MCP interface** (Model Context Protocol) so AI assistants can read and write your ideas
- **OAuth 2.1 with PKCE** on all endpoints — no raw bearer tokens
- **Git-backed storage** with automatic conflict retry (pull, append, commit, push)
- **Three entry types**: IDEA, TODO, MEMORY
- **Full-text search** by subject, body, date range, and type

## Setup

### 1. Build

```bash
cargo build --release
cargo build --release --features client  # also builds gitideas-client
```

### 2. Configure

Create `~/.config/gitideas.ini`:

```ini
port = 8080
token = your-secret-password
repo = /path/to/your/git/repo
```

| Key | Required | Description |
|-----|----------|-------------|
| `port` | yes | HTTP port to listen on |
| `token` | yes | Password for OAuth authorization page |
| `repo` | yes | Path to a git repository for storing entries |
| `host` | no | Bind address (default: `127.0.0.1`) |
| `url` | no | Public URL if behind a reverse proxy |

The `repo` path must be an initialized git repository. To create one:

```bash
mkdir ~/ideas && cd ~/ideas && git init
```

### 3. Run

```bash
gitideas
```

## Authentication

All endpoints require OAuth 2.1 access tokens. The flow:

1. Client discovers endpoints via `/.well-known/oauth-protected-resource`
2. Client registers via `POST /oauth/register`
3. User authorizes in the browser (enters the configured `token` as password)
4. Client exchanges the authorization code for an access token (PKCE S256)

The `gitideas-client` CLI handles this automatically, opening a browser on first use and caching the token in `~/.config/gitideas-oauth-token`.

## CLI Client

```bash
# Add an entry (reads body from stdin)
echo "Details here" | gitideas-client add IDEA "My idea subject"
gitideas-client add TODO "Fix the login bug"

# Search
gitideas-client search                            # all entries
gitideas-client search --text "query"             # search body + subject
gitideas-client search --subject "query"          # search subject only
gitideas-client search --type TODO                # filter by type
gitideas-client search --after 2026-01-01         # date range
gitideas-client search myquery                    # bare arg = text search
```

## REST API

### POST /add

```json
{
  "type": "IDEA",
  "subject": "Use sqlite for caching",
  "text": "We could add an optional sqlite layer for faster lookups."
}
```

Response:

```json
{
  "ok": true,
  "file": "IDEA.2026-04-01-14:30",
  "date": "2026-04-01-14:30"
}
```

### POST /search

All fields are optional:

```json
{
  "subject": "sqlite",
  "text": "caching",
  "type": "IDEA",
  "after": "2026-03-01",
  "before": "2026-04-15-12:00"
}
```

Response:

```json
{
  "entries": [
    {
      "type": "IDEA",
      "date": "2026-04-01-14:30",
      "subject": "Use sqlite for caching",
      "text": "We could add an optional sqlite layer for faster lookups."
    }
  ]
}
```

Returns newest entries first, up to 100KB of results.

## MCP Interface

The MCP endpoint at `POST /mcp` exposes two tools:

| Tool | Parameters | Description |
|------|-----------|-------------|
| `add` | `type`, `subject`, `text` | Add a new entry |
| `search` | `subject?`, `text?`, `type?`, `after?`, `before?` | Search entries |

To connect from an MCP client, point it at `http://host:port/mcp`. The client will be guided through OAuth discovery and authorization automatically.

## Storage Format

Each document type is stored in its own subdirectory (`IDEA/`, `TODO/`, `MEMORY/`). Files are named `TYPE.YYYY-MM-DD-hh:mm` (e.g., `IDEA/IDEA.2026-04-01-14:30`). A new file is created when the current one exceeds 100KB.

Each entry:

```
date: 2026-04-01-14:30
subject: My idea
-----🔥😀🍕🎸
The body text goes here.
Multiple lines supported.
-----🔥😀🍕🎸
```

The emoji delimiter is chosen to not appear in the body.

## Development

```bash
cargo test              # run all tests (18 unit + 11 integration)
cargo build             # build server only
cargo build --features client  # build server + client
```

## License

MIT
