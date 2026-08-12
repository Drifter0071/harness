# Harness

Ultra-lightweight AI Agent Harness — Desktop & Web.

## Architecture

- **Frontend**: React 18 + TypeScript + Tailwind CSS + Vite
- **Backend**: Tauri v2 + Rust
- **State**: Zustand
- **Design**: Ultra-minimal dark theme (#000000)

## Quick Start

```bash
# Install dependencies
npm install

# Run in browser (dev mode)
npm run dev

# Run as desktop app
npm run tauri:dev
```

## Project Structure

```
harness/
├── src/                    # React frontend
│   ├── components/         # UI components
│   │   ├── Chat.tsx        # Default single-column chat
│   │   ├── DynamicCanvas.tsx   # Sub-canvas tiling
│   │   ├── ActiveJobBar.tsx    # Floating status bar
│   │   ├── FileExplorer.tsx    # Workspace file browser
│   │   ├── MessageRenderer.tsx # Markdown/code rendering
│   │   └── Settings.tsx    # BYOK key management
│   ├── lib/
│   │   ├── ipc.ts          # Tauri IPC bridge types
│   │   └── store.ts        # Zustand state store
│   ├── App.tsx             # App shell + layout
│   ├── main.tsx            # Entry point
│   └── index.css           # Global styles + Tailwind
├── src-tauri/              # Rust backend
│   ├── src/
│   │   ├── lib.rs          # Tauri builder + plugin registration
│   │   ├── commands.rs     # All IPC command handlers
│   │   ├── agent.rs        # Agent type definitions + logic
│   │   ├── web_search.rs   # DuckDuckGo search integration
│   │   └── main.rs         # Binary entry point
│   ├── Cargo.toml          # Rust dependencies
│   └── tauri.conf.json     # Tauri configuration
├── DESIGN.md               # Design system documentation
├── package.json
├── tsconfig.json
├── vite.config.ts
├── tailwind.config.js
└── postcss.config.js
```

## IPC Commands

### Workspace
- `open_folder_dialog` → Native folder picker
- `read_file(path)` → File content as string
- `write_file(path, content)` → Write/create file
- `list_directory(path)` → Directory listing
- `search_files(path, pattern)` → Recursive file search
- `create_directory(path)` → Create directory tree

### CLI
- `execute_command(command, args, cwd?)` → Run command, get stdout/stderr/exit_code
- `kill_process(pid)` → Terminate process

### Agents
- `spawn_agent(name, role, systemPrompt)` → Create agent, returns ID
- `send_agent_message(agentId, content)` → Send message, get response
- `get_agent_status(agentId)` → Agent info
- `list_active_agents()` → All agents
- `stop_agent(agentId)` → Terminate agent
- `health_check()` → Agent health status map

### Search
- `web_search(query)` → DuckDuckGo results

### Settings
- `save_api_key(provider, key)` → Store key
- `get_api_keys()` → List masked keys
- `delete_api_key(provider)` → Remove key

## Commands (in Chat)

- `/search <query>` — Web search via DuckDuckGo
- `/open <path>` — Set workspace directory
- `/clear` — Clear conversation and canvas
