# Security and privacy

Agent transcripts contain proprietary code and, sometimes, secrets. Huella is
built so that nothing leaves your machine unless you choose a remote model.

## What Huella stores

A SQLite database (`~/Library/Application Support/huella/huella.db` on macOS,
`~/.local/share/huella/huella.db` on Linux, or `$HUELLA_DB`) with, for every
edit: the file path, the anchored symbols and hashes of their bodies, the
request that led to it and the agent's explanation. It does not copy your
source files; the guided review reads them from the agents' own transcripts.

## What is sent to a model, and only if you ask for it

- `huella decisions extract`: the dialogue of each session (without tool calls
  or their results) to the provider you choose.
- `huella review narrate`: the diff of a turn and its explanations.
- `huella embed` and search by meaning: requests, explanations and decisions.

With a local server (Ollama, llama.cpp, LM Studio) none of this leaves your
network.

## Secret redaction

Before storing anything or sending it to a model, Huella replaces secrets with
`[secreto…]`: provider keys (OpenAI, Anthropic, AWS, GitHub, Google, Stripe,
Slack), bearer tokens, JWTs, private key blocks, passwords in connection
strings and random-looking values assigned to names such as `API_KEY`,
`client_secret` or `password`. Data stored by an older version is redacted the
next time the database is opened. Redaction is pattern-based and cannot catch
every secret, so keep secrets out of your prompts when you can.

## The local app

`huella ui` listens on `127.0.0.1` only, rejects any `Host` header other than
its own (against DNS rebinding) and requires the per-run token in an
`X-Huella-Token` header on every API call.

## Reporting a vulnerability

Please open a [private security advisory](https://github.com/juligarc10/huella/security/advisories/new)
rather than a public issue.
