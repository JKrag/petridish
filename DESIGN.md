# System Design Document: Local Project & Agent Radar (`project-radar`)

## 1. System Overview & Objectives

`project-radar` is a lightweight, zero-maintenance local monitoring daemon and frontend suite for macOS. It crawls configured root directories, automatically tracks Git state, senses real-time AI agent activity across scattered projects, and aggregates metadata into a local JSON store (`~/.project-radar/projects.json`).

### Core Goals

* **Zero Manual Upkeep**: Replace static Trello/Kanban boards with automated activity detection.
* **Low Memory Footprint**: Avoid persistent background browsers or heavy databases so system RAM remains available for local LLM inference.
* **Multi-Frontend Capability**: Expose unified JSON state to a fast Raycast launcher, a Terminal UI (TUI), or an optional local Web UI.

---

## 2. Architecture Diagram

```text
               ┌─────────────────────────────────────────┐
               │    macOS System & Working Directory     │
               └────────────────────┬────────────────────┘
                                    │
    ┌───────────────────────────────┼───────────────────────────────┐
    │ Event Triggers                │ Log File Events               │ Process Table
    ▼                               ▼                               ▼
[ Agent Hooks ]            [ File Watcher ]                [ Process Scanner ]
(Claude settings.json)     (FSEvents / watchdog)           (ps aux / lsof)
  - Pre/PostToolUse          - ~/.claude/                    - running CLIs
  - Stop events              - Copilot workspaceStorage      - cwd PID mapping
    │                        - ~/.copilot/session-state/     │
    └───────────────────────────────┼───────────────────────────────┘
                                    │
                                    ▼
                     ┌─────────────────────────────┐
                     │    radar-daemon (Python)    │
                     │  - Git state scanner        │
                     │  - Agent state aggregator   │
                     │  - Category classifier      │
                     └──────────────┬──────────────┘
                                    │
                                    ▼
                      ~/.project-radar/projects.json
                                    │
     ┌──────────────────────────────┼──────────────────────────────┐
     ▼                              ▼                              ▼
[ Raycast Extension ]       [ Terminal TUI ]              [ Optional Web UI ]
(React/TypeScript)          (Python Textual)              (FastAPI / Tailwind)

```

---

## 3. Data Ingestion & State Sensing Engine

The daemon combines **event-driven hooks**, **filesystem watching**, and **passive process scanning** to detect project activity without noticeable CPU overhead.

### A. Repository & Git Scanner

* **Discovery**: Recursively scans configured search roots (e.g., `~/Developer`, `~/Projects`, `~/Work`) for directories containing `.git` or project marker files (`package.json`, `Cargo.toml`, `pyproject.toml`).
* **Git Metrics Collected**:
* Uncommitted changes (`git status --porcelain`).
* Current branch name (`git rev-parse --abbrev-ref HEAD`).
* Last commit date and author (`git log -1 --format=%cd`).
* Remote repository URL (parsed to derive GitHub browser links).



### B. Live AI Agent Detection Strategy

| Agent Tool | Primary Sensing Mechanism | Target Path / Hook Configuration | Data Captured |
| --- | --- | --- | --- |
| **Claude Code** | Native Lifecycle Hooks + File Watcher | Configured in `~/.claude/settings.json` under `PreToolUse`, `PostToolUse`, `Stop`. Backup watcher on `~/.claude/` transcripts. | `session_id`, `cwd`, active tool usage, prompt activity state. |
| **GH Copilot (VS Code)** | Log File Watcher | `~/Library/Application Support/Code/User/workspaceStorage/<hash>/chatSessions/` | File modification timestamps mapped back to VS Code workspace path. |
| **GH Copilot CLI** | Log File Watcher | `~/.copilot/session-state/` | Session creation and last-modified timestamps per workspace. |
| **Local LLM / Custom CLIs** | Process Table Inspection | Shell command: `ps aux` cross-referenced with `lsof -p  | grep cwd` |

#### Claude Code Hook Protocol (Fast Path)

The installer configures a lightweight executable (`radar-hook`) inside `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "radar-hook --agent claude-code --event PreToolUse",
            "async": true
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "radar-hook --agent claude-code --event Stop",
            "async": true
          }
        ]
      }
    ]
  }
}

```

*`radar-hook` reads event JSON from `stdin`, extracts `cwd` and `session_id`, updates `projects.json`, and exits in under 15ms.*

---

## 4. State Storage Schema (`projects.json`)

The single source of truth lives in `~/.project-radar/projects.json`. No relational or document database binary is required.

```json
{
  "updated_at": "2026-08-05T22:45:00Z",
  "projects": [
    {
      "id": "cat-pedigree-analyzer",
      "name": "cat-pedigree-analyzer",
      "path": "/Users/dev/Projects/cats/pedigree-analyzer",
      "category": "Cats & Breeding",
      "git": {
        "is_repo": true,
        "branch": "main",
        "is_dirty": true,
        "uncommitted_files": 4,
        "last_commit_at": "2026-08-04T14:10:00Z",
        "github_url": "https://github.com/user/cat-pedigree-analyzer"
      },
      "agent": {
        "is_active": true,
        "active_agent": "Claude Code",
        "last_event": "PreToolUse",
        "last_event_at": "2026-08-05T22:44:12Z",
        "session_id": "sess_98765"
      },
      "status_bucket": "active"
    }
  ]
}

```

### Auto-Categorization Logic

Projects are bucketed automatically based on activity timestamps:

1. **Active**: Live running agent session OR Git commit/uncommitted edit within the last 48 hours.
2. **In Flight**: Activity between 2 and 14 days ago.
3. **Stale**: Activity between 14 and 60 days ago.
4. **Cold Storage**: Inactive for over 60 days.

---

## 5. Frontend Interfaces

```text
┌────────────────────────────────────────────────────────────────────────┐
│                          INTERFACE MATRIX                              │
├─────────────────┬──────────────────────┬───────────────────────────────┤
│ Target          │ Primary Tech Stack   │ Key Features & Use Cases      │
├─────────────────┼──────────────────────┼───────────────────────────────┤
│ 1. Raycast Ext. │ React / TypeScript   │ Instant hotkey list, zero RAM  │
│                 │ (Raycast API)        │ usage when closed, actions.   │
├─────────────────┼──────────────────────┼───────────────────────────────┤
│ 2. Terminal UI  │ Python Textual       │ Live workspace overview inside│
│                 │ (or Rust Ratatui)    │ iTerm/Ghostty/Warp.            │
├─────────────────┼──────────────────────┼───────────────────────────────┤
│ 3. Web UI       │ FastAPI + Tailwind   │ Visual Kanban board view      │
│                 │ (Optional local server)│ (Runs on-demand only).        │
└─────────────────┴──────────────────────┴───────────────────────────────┘

```

### Option A: Raycast Extension (Primary / Target Win)

* **Access**: Bound to macOS global key combo (e.g., `Cmd + Shift + P`).
* **Visual Layout**: Searchable list sorted by `status_bucket` with status indicator badges:
* 🟢 **Active Agent** (`Claude Code` / `Copilot`)
* 🟡 **Dirty Git State** (Uncommitted changes)
* ⚪ **Stale / Idle**


* **Native Hotkey Actions**:
* `Enter`: Open directory in VS Code / Cursor (`code <path>`).
* `Cmd + Enter`: Open Terminal tab at project `cwd`.
* `Cmd + G`: Open GitHub repository in default browser.



### Option B: Terminal UI (Secondary Workspace View)

* Built using Python's `Textual` library.
* Keyboard-driven UI rendered inside terminal splits.
* Features dynamic column filtering (`Active`, `In Flight`, `Stale`) with direct keybindings to resume Claude Code CLI sessions (`claude --resume <session_id>`).

### Option C: Web Dashboard (On-Demand)

* Ultra-lightweight local web app (FastAPI + single-page HTML/Tailwind).
* Launched only when requested (`radar web`) to keep system memory entirely clear during heavy local LLM runs.

---

## 6. Implementation Milestones (Kick-Off Plan)

1. **Phase 1: Core Daemon & Git Discovery**
Create the Python scanner script that discovers project roots, parses Git metrics, and generates `projects.json`. Set up a background `launchd` service or `watchdog` process.
2. **Phase 2: Agent Telemetry Pipeline**
Implement `radar-hook` executable for Claude Code settings, add file watchers for VS Code / Copilot workspace log directories, and integrate PID/CWD scanning for active processes.
3. **Phase 3: Raycast Extension Build**
Scaffold Raycast TypeScript extension, read `projects.json`, format items with status tags, and hook up execution actions (Open Code, Open Terminal, Open GitHub).
4. **Phase 4: TUI & Refinement**
Implement the Textual terminal frontend and refine project categorization thresholds based on usage feedback.

---

Would you like to start building Phase 1 (the Python background scanner and JSON generator) right away, or modify any specific path configurations first?