# Web Terminal

A browser-based terminal interface that connects to local programs via WebSockets, featuring input/output history and multi-client synchronization.

## Key Features

- 🖥️ **Real Terminal Emulation**: Interact with local CLI programs
- 🔄 **Sync Across Clients**: All connected browsers see the same session
- ⏮️ **Command History**: Full record of inputs and outputs

### Installation

```bash
git clone https://github.com/gyanbu/web-terminal.git
cd web-terminal
```

## Quick Start

```bash
cargo run --release -- <target_executable> [target_args...]
```
