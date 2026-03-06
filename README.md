# signal-cli-rust

A lightweight, cross-platform Signal messenger CLI built in Rust on [libsignal](https://github.com/signalapp/libsignal) + [presage](https://github.com/whisperfish/presage). Includes an MCP server for AI integration.

## Install

### One-liner (macOS / Linux)

```bash
curl -sSL https://raw.githubusercontent.com/didhd/signal-cli-rust/main/install.sh | bash
```

> Private repo? Use: `GITHUB_TOKEN=ghp_xxx curl -sSL ... | bash`

### macOS (DMG)

Download the `.dmg` from [Releases](https://github.com/didhd/signal-cli-rust/releases), open it, and run `install.sh` inside.

### Linux (Debian / Ubuntu)

```bash
# Download .deb from releases
sudo dpkg -i signal-cli-rust_*.deb
```

### Linux (Fedora / RHEL)

```bash
# Download .rpm from releases
sudo rpm -i signal-cli-rust-*.rpm
```

### Windows

Download the `.zip` from [Releases](https://github.com/didhd/signal-cli-rust/releases), extract, and run `install.ps1` in PowerShell.

### From source

```bash
# Prerequisites: rust, cmake, protobuf
cargo install --git https://github.com/didhd/signal-cli-rust.git
```

## Login (Connecting to your Signal account)

There are **two ways** to authenticate:

### Option 1: Link as secondary device (recommended)

This links your CLI as a secondary device to your existing Signal mobile app, just like Signal Desktop.

```bash
signal-cli link-device -n "my-cli"
```

This will:
1. Display a **QR code** in your terminal
2. Open **Signal on your phone** > Settings > Linked Devices > Link New Device
3. **Scan the QR code** with your phone
4. Done! Your CLI is now linked

### Option 2: Register a new account

Register a new phone number directly (requires a captcha and SMS/voice verification):

```bash
# 1. Get a captcha token from: https://signalcaptchas.org/registration/generate.html
# 2. Register
signal-cli register \
  --phone-number +14154380765 \
  --captcha "signalcaptcha://signal-recaptcha-v2.xxxxx"

# 3. Enter the SMS verification code when prompted
```

## Usage

### Send a message

```bash
# By phone number (auto-resolves UUID via CDSI)
signal-cli send -n +14154380765 -m "Hello from Rust!"

# By UUID
signal-cli send -u xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx -m "Hello!"

# With attachment
signal-cli send -n +14154380765 -m "Check this out" --attach photo.jpg
```

### Receive messages

```bash
# Stream messages (stay connected)
signal-cli receive

# Receive and exit when queue is empty
signal-cli receive -q
```

### List contacts and groups

```bash
signal-cli list-contacts
signal-cli list-groups

# JSON output
signal-cli -o json list-contacts
```

### Resolve phone number to UUID

```bash
signal-cli resolve -n +14154380765
```

### More commands

```bash
signal-cli whoami           # Show account info
signal-cli list-devices     # List linked devices
signal-cli sync-contacts    # Sync contacts from phone
signal-cli stats            # Protocol statistics
signal-cli --help           # Full help
```

## MCP Server (for AI integration)

The `signal-mcp` binary provides an MCP (Model Context Protocol) server over stdin/stdout.

### Configure in Claude Desktop / Claude Code

Add to your MCP config:

```json
{
  "mcpServers": {
    "signal": {
      "command": "signal-mcp",
      "env": {
        "SIGNAL_CLI_DB": "/path/to/signal.db3"
      }
    }
  }
}
```

### Available MCP Tools

| Tool | Description |
|------|-------------|
| `send_message` | Send message by UUID |
| `send_message_by_phone` | Send message by phone number |
| `list_contacts` | List all contacts |
| `list_groups` | List all groups |
| `receive_messages` | Receive pending messages |
| `whoami` | Get account info |
| `resolve_phone` | Resolve phone to UUID |

## Build from source

```bash
# Prerequisites
brew install protobuf cmake   # macOS
# or
sudo apt install protobuf-compiler cmake libssl-dev  # Linux

# Build
cargo build --release

# Binaries at:
#   target/release/signal-cli
#   target/release/signal-mcp
```

## Cross-platform support

| Platform | Architecture | Status |
|----------|-------------|--------|
| macOS | aarch64 (Apple Silicon) | Supported |
| macOS | x86_64 (Intel) | Supported |
| Linux | x86_64 | Supported |
| Linux | aarch64 | Supported |
| Windows | x86_64 | Supported |
