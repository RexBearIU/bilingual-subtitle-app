# Docs index

Start here, read one section of one file, stop. Nothing in `docs/` is meant to
be read end to end — together they are ~85 KB, and almost every question is
answered by a single section.

| File | Size | Answers | Read it when |
|------|------|---------|--------------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | 14 KB | How the system works **now** | Changing behaviour, or checking what something currently does |
| [IPC-CONTRACT.md](IPC-CONTRACT.md) | 10 KB | The exact Rust ↔ webview shapes | Adding a command, event, or settings field |
| [DECISIONS.md](DECISIONS.md) | 33 KB | Why it is built this way | About to change something that looks wrong — it may already have been tried |
| [SETUP.md](SETUP.md) | 10 KB | Getting it to run | First build, sidecar env, API keys, model downloads |
| [MILESTONES.md](MILESTONES.md) | 14 KB | What was built when, and what was measured | Rarely — historical record, not current truth |

**Where they disagree, ARCHITECTURE.md wins.** It describes the code as it
stands; MILESTONES records what was true at the time and is not rewritten when
behaviour changes later.

## Find it by topic

| Looking for | Go to |
|-------------|-------|
| Where audio goes after capture | ARCHITECTURE § Pipeline, § Thread / channel model |
| How a chunk's length is decided | ARCHITECTURE § Chunking · [ADR-0009](DECISIONS.md), [ADR-0015](DECISIONS.md) |
| Why a subtitle was dropped | ARCHITECTURE § Hallucination filtering · [ADR-0021](DECISIONS.md) |
| Whisper model, beam size, prompts | ARCHITECTURE § ASR worker · [ADR-0006](DECISIONS.md), [ADR-0010](DECISIONS.md) |
| Translation prompt, context, retries, cache | ARCHITECTURE § Translation worker · [ADR-0011](DECISIONS.md), [ADR-0020](DECISIONS.md) |
| Which provider is used and who owns the list | [ADR-0013](DECISIONS.md), [ADR-0017](DECISIONS.md), [ADR-0018](DECISIONS.md), [ADR-0019](DECISIONS.md) |
| Why clicks do / do not reach the overlay | ARCHITECTURE § Copying a subtitle · [ADR-0012](DECISIONS.md), [ADR-0016](DECISIONS.md) |
| Control bar and caption sizing | ARCHITECTURE § Sizing |
| Capturing one app instead of all audio | ARCHITECTURE § Per-process capture · [ADR-0008](DECISIONS.md) |
| A command's arguments, an event's payload | IPC-CONTRACT § Commands, § Events |
| A field in `settings.json` | IPC-CONTRACT § Settings shape |
| Which file a module lives in | ARCHITECTURE § Backend module layout, § Frontend layout |
| Installing, first run, model downloads | SETUP |

## Conventions

- **ADRs are append-only.** A decision that no longer holds gets a new ADR that
  supersedes it; the old one stays as written. Never edit an accepted ADR to
  match current behaviour — the record of what was believed at the time is the
  point. [DECISIONS.md](DECISIONS.md) opens with an index, so you can read the
  one ADR you need rather than the file.
- **Behaviour changes go in ARCHITECTURE.md**, in the section that already
  covers that area, not appended as a changelog.
- **MILESTONES is not updated for later changes.** If code drifts from what a
  milestone described, that is expected; add a superseding note only when the
  old text would actively mislead someone reading it as current.
