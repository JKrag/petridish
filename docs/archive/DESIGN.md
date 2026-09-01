> **ARCHIVED — 2026-09-01.** This was the original pre-implementation pitch/design doc,
> written under the working title `project-radar` before a line of code existed. It
> describes a plan that didn't survive contact with the real build: Python `Textual` for the
> TUI (built in `curses` instead, then reimplemented again in Rust/ratatui as `petri` —
> `petri/SPEC.md`), a FastAPI/Tailwind web UI (never built, still tracked as deferred in
> `ARCHITECTURE.md` §5), and `ps`/`lsof` process scanning as a primary sensing mechanism
> (Claude Code's own JSONL transcripts turned out to need no such thing — `ARCHITECTURE.md`
> F1/F8). §§1–6 below are kept only as a historical record of the original pitch.
>
> **§7 (Distribution & Deployment Plan) was still live** — `src/petridish/installer.py`
> cites its `D1`–`D6` requirements and `§7.1` by number in real docstrings/comments — so
> that content was extracted verbatim to **`ARCHITECTURE.md` §8** before this file was
> archived. Go there for the current, cross-referenced copy; what's below is frozen.

# System Design Document: Local Project & Agent Radar (`petridish`)

## 1. System Overview & Objectives

`petridish` is a lightweight, zero-maintenance local monitoring daemon and frontend suite for macOS. It crawls configured root directories, automatically tracks Git state, senses real-time AI agent activity across scattered projects, and aggregates metadata into a local JSON store (`~/.petridish/projects.json`).

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
                     │   petridish-daemon (Python) │
                     │  - Git state scanner        │
                     │  - Agent state aggregator   │
                     │  - Category classifier      │
                     └──────────────┬──────────────┘
                                    │
                                    ▼
                      ~/.petridish/projects.json
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

The installer configures a lightweight executable (`swab-hook`) inside `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "swab-hook --agent claude-code --event PreToolUse",
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
            "command": "swab-hook --agent claude-code --event Stop",
            "async": true
          }
        ]
      }
    ]
  }
}

```

*`swab-hook` reads event JSON from `stdin`, extracts `cwd` and `session_id`, updates `projects.json`, and exits in under 15ms.*

---

## 4. State Storage Schema (`projects.json`)

The single source of truth lives in `~/.petridish/projects.json`. No relational or document database binary is required.

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
* Launched only when requested (`petri web`) to keep system memory entirely clear during heavy local LLM runs.

---

## 6. Implementation Milestones (Kick-Off Plan)

1. **Phase 1: Core Daemon & Git Discovery**
Create the Python scanner script that discovers project roots, parses Git metrics, and generates `projects.json`. Set up a background `launchd` service or `watchdog` process.
2. **Phase 2: Agent Telemetry Pipeline**
Implement `swab-hook` executable for Claude Code settings, add file watchers for VS Code / Copilot workspace log directories, and integrate PID/CWD scanning for active processes.
3. **Phase 3: Raycast Extension Build**
Scaffold Raycast TypeScript extension, read `projects.json`, format items with status tags, and hook up execution actions (Open Code, Open Terminal, Open GitHub).
4. **Phase 4: TUI & Refinement**
Implement the Textual terminal frontend and refine project categorization thresholds based on usage feedback.

---

## 7. Distribution & Deployment Plan

Intended for eventual public release. Nothing here is built yet; it is recorded now
because it constrains the installer (M9) that *is* about to be built.

### 7.1 Install model

The tool ships as a **Python package with two console scripts** (`swab`,
`swab-hook`), installed into an **isolated virtualenv** with shims placed on the
user's `PATH`. It is deliberately *not* a `pip install` into the system or user
site-packages: it is an application, not a library, so `uv tool` / `pipx` semantics
are the right ones.

**Development installs use the same mechanism as end-user installs:**

```sh
uv tool install --editable ~/repos/JKrag/project-radar   # dev (repo dir name unchanged)
uv tool install <package-name>                           # end user (post-PyPI)
pipx install <package-name>                              # end user, equivalent
```

Both produce shims in `~/.local/bin`. This is not just convenience — it means the
launchd plist, the Claude hook entry, and `swab doctor` are all exercised against
the **real production layout** during development, rather than a dev-only
arrangement that would need re-testing after release.

Rejected alternatives:

| Approach | Why rejected |
| --- | --- |
| Symlink into `/opt/homebrew/bin` | Homebrew-managed directory; `brew doctor` flags foreign files and brew operations can clobber them. |
| Shell alias in `.zshrc` | **Actively broken.** `launchd` jobs and Claude Code hooks run non-interactively and never source shell rc files — the daemon and hook would silently fail while working in an interactive terminal. |
| `pip install --user` | PEP 668 friction on managed Pythons; pollutes user site-packages; no isolation. |

### 7.2 Release channels

1. **PyPI — primary, source of truth.** Enables `uv tool install` / `pipx install`
   immediately and is what any other channel wraps.
2. **Homebrew tap — secondary, optional.** Because the tool is macOS-only by nature
   (launchd, `~/Library` paths), brew is a natural fit for discoverability. Start
   with a personal tap (`brew install <user>/tap/<name>`) rather than
   `homebrew-core`, which imposes notability requirements (stars/forks, release
   history) and ongoing maintenance obligations.

**The zero-dependency rule is a distribution asset, not just a build constraint.**
Homebrew Python formulae normally require a vendored `resource` block per
transitive dependency — often dozens, each needing bumps. A stdlib-only package
needs none, making the formula roughly fifteen lines. This is a strong reason to
keep runtime dependencies at zero permanently, beyond the original testability
rationale.

### 7.3 Design requirements this imposes

Numbered so the installer (M9) can be checked against them:

- **D1 — Discover the binary path at install time; never hardcode it.** The
  installer must resolve `command -v swab-hook` and write the **absolute** result
  into both the launchd plist and `~/.claude/settings.json`. A `uv tool` user has
  `~/.local/bin/swab-hook`; a Homebrew user has `/opt/homebrew/bin/swab-hook`.
  Assuming either breaks the other.
- **D2 — Assume no `PATH` in non-interactive contexts.** `launchd` does not inherit
  the user's shell environment, and neither do Claude Code hooks. Every command
  string written to disk must be absolute.
- **D3 — Runtime dependencies stay at zero.** See 7.2. Adding one is a distribution
  decision, not just an implementation one.
- **D4 — The installer is idempotent, reversible, and marks its own work.** It must
  back up `~/.claude/settings.json` before editing, *append* to existing hook arrays
  (never replace them — real users have other hook consumers), tag every entry it
  adds with a literal marker comment, and ship an `--uninstall` that removes only
  marked entries.
- **D5 — Declare macOS-only.** Add the appropriate classifier and fail early with a
  clear message on other platforms rather than half-installing. **Implemented (M10):**
  `Operating System :: MacOS :: MacOS X` classifier in `pyproject.toml`, plus
  `installer.py`'s `check_platform()` (`sys.platform != "darwin"` raises before touching
  anything) — that's where "half-installing" would actually happen, since launchd is the
  macOS-only surface. The CLI itself (`swab scan`/`list`) is deliberately *not*
  platform-gated: its sensors already degrade to `null`/empty per CLAUDE.md invariant 5
  (e.g. the Copilot sensor finding no `workspaceStorage/` on Linux is that invariant
  working, not a bug), so gating the whole CLI would reject strictly more than necessary.
- **D6 — Respect user-data separation.** Code lives in the tool's venv; user state
  (`config.toml`, `projects.json`, `events.ndjson`) lives in `~/.petridish/` and
  must survive uninstall/reinstall untouched.

### 7.4 Naming — resolved

`project-radar` was a working title, replaced (2026-08-06) with **`petridish`** —
the filesystem-as-Petri-dish metaphor for dozens of small AI-assisted experiments
growing (or sitting dormant) across the machine. The bare name `petri` is already
taken on PyPI; `petridish` was checked and is free, so it is the distribution
name. `petri` is kept in reserve for a possible future dashboard (see below), not
used for anything published. Final mapping:

- PyPI distribution / import package: `petridish`
- CLI console script (scan/list/path/doctor): `swab` — "swabbing" the filesystem
  to see what's alive. Replaces `radar`.
- Hook console script: `swab-hook`. Replaces `radar-hook`.
- Reserved for the not-yet-built frontend/dashboard: `petri` (e.g. a future
  `petri web` command — see §5 Option C, still just a name reservation, not built).
- User-data directory: `~/.petridish/`
- launchd label: `com.petridish.daemon`
- Hook marker string written into `settings.json`: `# petridish` — load-bearing,
  since `swab doctor` and `--uninstall` identify their own entries by matching it.

This lands before M9 (the installer) is written, so no released install exists
yet to orphan.

---

Would you like to start building Phase 1 (the Python background scanner and JSON generator) right away, or modify any specific path configurations first?