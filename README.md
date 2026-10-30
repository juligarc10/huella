# Huella

**English** · [Español](README.es.md)

Memory of your AI coding agent sessions, anchored to the code.

Huella reads the transcripts of your coding agents, works out which functions,
classes and methods each edit touched, and shows you — on hover, in your
editor — what was asked and what the agent explained when it wrote that code.
No model call, in milliseconds, and it keeps working after you rename or move
the code.

It also guides the **review** of what a session did: turn by turn, following
the flow of the code from its entry point, with the reason for every step and
the lines the agent never explained highlighted. Fast, but complete: every
changed line belongs to some step.

And it extracts the project decisions that don't live in the code ("Rust
instead of Python", "SQLite locally") with a model of your choice, tracks
which ones are still in force, exports them as ADRs, and serves all of this to
any agent over MCP.

Everything stays on your machine.

## How you use it

1. **Install once**: the `huella` binary, the editor extension (VS Code or
   Zed) and the MCP server for your agent. Optionally, a model to extract
   decisions and an embeddings model to search by meaning.
2. **Work as usual with your agent.** Huella indexes its sessions on its own:
   the editor extension every few seconds, the app when it opens.
3. **While coding**, hover a function to see why it exists: the request, the
   agent's explanation, the session, and whether the code changed since.
4. **After a session**, review it in `huella ui` (or in Zed's git diff with
   `huella review zed`): steps in flow order, unexplained lines in amber, a
   verdict per step (fine, change, undo), comments on any line and questions to
   the session. **Request changes** turns the comments into instructions for the
   agent (copy them, or the agent reads them over MCP), and the review then shows
   what changed afterwards.
5. **Now and then**, extract decisions, fix the ones the model got wrong and
   export them as ADRs next to your code.
6. **Your agent reads the same memory over MCP** before touching code: symbol
   history, decisions in force (including your own preferences) and search.

## Install

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/juligarc10/huella/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/juligarc10/huella/main/install.ps1 | iex
```

The installer downloads the binary for your system from the latest
[release](https://github.com/juligarc10/huella/releases), checks it against the
release's `SHA256SUMS` (it refuses to install without it), puts it in
`~/.local/bin` (`%LOCALAPPDATA%\Programs\huella` on Windows) and runs
`huella setup`. `HUELLA_VERSION` picks a version, `HUELLA_INSTALL_DIR` another
folder and `HUELLA_NO_SETUP=1` skips the setup.

With Rust 1.90+ you can also build it yourself:

```sh
cargo install --git https://github.com/juligarc10/huella huella-cli
```

Either way, Huella then sets itself up:

```sh
huella setup        # asks; huella setup --yes takes the defaults
huella doctor       # checks the installation (--online also calls the models)
```

`huella setup` only does what's missing, so running it again after an update is
safe:

- backs up the database before a schema migration;
- configures the model (Claude API, local Ollama or any OpenAI-compatible
  server), tries it, and keeps the key in a separate `chmod 600` file;
- optionally configures embeddings for search by meaning;
- registers the MCP server in the agents it finds (Claude Code, Codex, Copilot
  in VS Code, Copilot CLI) without touching the rest of their settings;
- installs the VS Code extension from the latest release (`--vsix` for a local
  one);
- indexes your sessions.

**Zed** can't install extensions from the command line yet: run
`zed: install dev extension` and pick `extensions/zed` from a clone of this
repository. Zed needs `~/.cargo/bin` on its `PATH`.

## The app

```sh
huella ui
```

The home page is the review inbox: sessions by review status. Review them step
by step (`j`/`k` to move; `r` fine, `c` change, `d` undo), comment on any diff
line, ask the session why it did something, and request changes or approve.
Each step shows its **evidence**: what the agent read and explored before
writing it, its reasoning and plan, the checks it ran afterwards (tests, build,
lint) with their result, and the attempts it discarded; steps it never checked
are flagged. It also shows the step's **impact** (code outside the session that
uses what changed) and, with a model, whether the turn **covers the request**:
requirements done, partial or missing, and what it did without being asked.
**Branch** reviews what you are about to ship: the diff against the base
branch, every line attributed to the agent session that wrote it, to a session
that changed it from the terminal, or to the person who changed it by hand.
Also search the memory, browse and correct decisions, export ADRs, see a symbol's history and
run the long tasks (indexing, embeddings, decision extraction, refactor
benchmark) in the background. `⌘K` jumps anywhere. It listens on `127.0.0.1`
only, with a per-run token.

## Command line

```sh
huella index                          # index what's new in the agents' sessions
huella why src/app.ts:42              # what was said about the symbol on that line
huella search rate limit              # requests, explanations, symbols and decisions
huella review list                    # recent sessions, with their agent
huella review show 84e33d8e           # guided review in the terminal
huella review zed 84e33d8e            # review in Zed's git diff (stage = reviewed)
huella review comment 84e33d8e 3 "Validate the email first" --line "await repo.save(user)"
huella review feedback 84e33d8e       # instructions for the agent, in Markdown
huella review ask 84e33d8e "Why not reuse the validator?" --step 3
huella review coverage 84e33d8e       # does each turn do what was asked?
huella review branch --base main      # the branch, line by line: which session or person
huella decisions extract --provider openai-compatible --base-url http://localhost:11434/v1 --model qwen2.5:7b
huella decisions list --scope user    # your preferences, valid in every project
huella adr                            # export the project's decisions to docs/adr/
huella embed                          # vectors for search by meaning (HUELLA_EMBED_MODEL)
huella refactors                      # how much memory survives renames and moves
```

Decision extraction works with the Claude API (`ANTHROPIC_API_KEY`, default
`claude-haiku-5-5`) or any OpenAI-compatible server (Ollama, LM Studio,
llama.cpp, vLLM; key in `HUELLA_LLM_API_KEY`). The same model answers the
questions you ask a session. Search by meaning works with any OpenAI-compatible
embeddings endpoint; a multilingual model such as `bge-m3` works best if your
sessions aren't in English.

### Configuration

Huella is started from anywhere (your editor, your agent over MCP, a terminal),
so its settings live in one file per user, next to the database:
`~/Library/Application Support/huella/config.toml` on macOS,
`~/.local/share/huella/config.toml` on Linux (or `$HUELLA_CONFIG`).

```toml
[llm]
provider = "openai-compatible"          # or "anthropic" (default)
base_url = "http://localhost:11434/v1"
model = "qwen2.5:7b"
api_key_file = "~/.config/huella/llm-key"   # optional; must be chmod 600

[embed]
model = "bge-m3"
base_url = "http://localhost:11434/v1"

[ui]
editor = "vscode"                       # or "zed" (default)
```

Command-line options win over environment variables (`HUELLA_LLM_PROVIDER`,
`HUELLA_LLM_BASE_URL`, `HUELLA_LLM_MODEL`, `HUELLA_EMBED_MODEL`,
`HUELLA_EMBED_URL`), which win over the file. The key is never written in
`config.toml`: it comes from `HUELLA_LLM_API_KEY` (`ANTHROPIC_API_KEY` for
Claude) or from `api_key_file`, which Huella refuses to read if other users can.
Unknown keys are an error, so a typo doesn't go unnoticed.

## Agents and languages

| Agent | Reads from | Anchoring |
| --- | --- | --- |
| Claude Code | `~/.claude/projects` | Exact: it stores the original file of every edit |
| Copilot in VS Code (chat and agent) | `…/Code/User/workspaceStorage/*/chatSessions` | Exact for files the session created; approximate otherwise |
| Copilot CLI | `~/.copilot/session-state` | Exact on create; approximate on edit |
| Codex | `~/.codex/sessions` (or `$CODEX_HOME`) | Exact on create; approximate on edit. Tested with sample transcripts only |

*Approximate* means the agent doesn't store the whole file: the text it wrote
is located in the current file, and if that code changed afterwards the edit
stays unanchored.

Symbols in TypeScript, TSX, JavaScript, Python, Rust, Go, Java, C#, Ruby, PHP,
C and C++ (tree-sitter).

## Privacy

Nothing leaves your machine unless you configure a remote model. Before
anything is stored or sent to a model, **secrets are redacted**: provider keys
(OpenAI, Anthropic, AWS, GitHub, Google, Stripe, Slack), tokens, JWTs, private
keys, connection-string passwords and random-looking values assigned to names
such as `API_KEY` or `password`. See [SECURITY.md](SECURITY.md).

## How it works

Agent adapters turn each session into normalized edits. The anchoring
compares each file before and after the edit and assigns every changed line to
the innermost symbol (tree-sitter), storing a hash of its normalized body and a
name-independent fingerprint. The store is SQLite (WAL, FTS5). The engine
resolves a symbol's history — also when it was renamed or moved, by matching
fingerprints of anchors that disappeared — and serves it over LSP (hover,
CodeLens, review diagnostics), MCP, the CLI and the local app.

| Crate | Role |
| --- | --- |
| `huella-core` | Domain model and the `Agent` trait |
| `huella-claude-code`, `huella-copilot`, `huella-codex` | Agent adapters |
| `huella-anchoring` | Symbol extraction and edit anchoring |
| `huella-store` | SQLite persistence, search, secret redaction of stored data |
| `huella-engine` | Indexing, queries, renames, decisions, ADRs, embeddings |
| `huella-review` | Guided review: net diff, flow plan, attention points |
| `huella-decisions` | Decision extraction with pluggable models |
| `huella-lsp` | Language server |
| `huella-cli` | The `huella` binary, MCP server and `huella ui` server |
| `ui` | The `huella ui` interface (React, shadcn/ui, Magic UI), embedded in the binary |
| `extensions/vscode`, `extensions/zed` | Thin editor extensions that launch the binary |

Architecture decisions live in [docs/adr](docs/adr) (in Spanish).

## Performance

Hover on a 1,205-line file: p95 9.2 ms. Indexing ~140 MB of transcripts from
scratch: about 1 s; incremental: instant. 97.5 % of the memory survives
synthetic refactors applied to 506 real anchors (rename, rename the class,
move the file, move the function, rename and edit).

## Known limitations

- A symbol's history only includes edits made through the agent's tools;
  in a review, changes made from a shell show up as unexplained.
- Copilot and Codex don't store the original file, so their edits to existing
  files are anchored approximately. Cursor, Aider and Windsurf have no adapter
  yet.
- Secret redaction is pattern-based: a secret with no recognisable shape and
  an ordinary name can slip through.
- Decision quality depends on the model: review them (`decisions set` fixes
  them).

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
