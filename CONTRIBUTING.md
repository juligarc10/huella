# Contributing to Huella

Thanks for your interest! Issues and pull requests are welcome, in English or
Spanish.

## Setup

- Rust 1.90+ (`rustup` installs the stable toolchain).
- Node 22+ and pnpm, only if you change the `huella ui` interface or the VS
  Code extension.

```sh
cargo test                      # every crate, including the LSP protocol tests
cargo clippy --all-targets -- -D warnings   # pedantic lints are on for the workspace
cargo fmt
```

The interface in `ui/` is compiled into `ui/dist`, which is committed because
the binary embeds it (so installing Huella never needs Node). After changing
the interface:

```sh
pnpm --dir ui install
pnpm --dir ui build             # commit ui/dist together with your change
```

To work on it with hot reload, run `huella ui --port 4874 --no-browser --dev`
and `pnpm --dir ui dev`, then open the URL the first command prints.

## Adding an agent

Implement `huella_core::Agent` in a new crate (see `huella-copilot` or
`huella-codex`): find the agent's session files, turn each edit into a
`FileEdit` (with the full file after the edit when the agent records it, or
just the inserted text otherwise) and return the dialogue for decision
extraction. Register it in `Sources::detect` (`huella-engine`). Please test it
with real sessions and describe in the pull request what you validated.

## Adding a language

Add the tree-sitter grammar to `huella-anchoring` and declare which nodes are
symbols in `language.rs`; the rules are data. The test
`every_rule_names_a_real_node_kind` catches typos in node names; add a test in
`symbols.rs` with a small sample of the language.

## Pull requests

- One logical change per commit, with a message that says what it does.
- Tests for new behaviour; `cargo clippy` and `cargo test` must pass.
- Never include real transcripts, secrets or personal paths in fixtures.
