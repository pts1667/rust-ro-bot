# CLAUDE.md

Guidance for working on the rust-ro-bot repo.

## What this repo is

A Rust bot that plays a character on a rust-ro server through its Bot HTTP API, open-ended. GOAP (`src/goap/`) plans
and runs the steps. A small local LLM picks the goals through smolgent, using SKILL.state context management
(`src/strategist/`). Design, verified facts and status: `PLAN.md`.

## Related repos and locations

The repos are checked out side by side; paths are relative to this repo.

| Repo | Location | Role |
|---|---|---|
| rust-ro-bot (this repo) | `.` | The bot. Rust crate. |
| smolgent | `../smolgent`, https://github.com/pts1667/smolgent | LLM harness. The bot depends on the GitHub repo pinned to a commit (`Cargo.toml`). `ProviderConfig::extra_body` exists for llama.cpp's `thinking_budget_tokens`. |
| rust-ro | `../rust-ro`, https://github.com/pts1667/rust-ro | The game server source. Bot API code is in `server/src/server/bots/` (`mod.rs`, `http.rs`, `auth.rs`, `controller.rs`, `command.rs`, `lifecycle.rs`, `observation.rs`), and the API docs are in `server/src/server/bots/SKILL.md`. Config defaults are in `lib/configuration/src/configuration.rs`. Git repo, branch `main`. |
| rathena | `../rathena` | rAthena, the origin of the server's NPC scripts. The server is pre-renewal: use `npc/pre-re/` and the shared `npc/` folders, not `npc/re/`. |

### Which copy to trust

The test server has run from `../rust-ro` (`./target/release/server.exe --host 127.0.0.1`), and at times from a separate
deployed copy of rust-ro. Check the working directory of the running `server.exe` and use that copy's files:

- **Bot API key:** `config/bots_api_key.txt` of the running copy. Another copy's key returns 401. Never print the key or
  paste it into files, logs, or chat.
- **Server config:** `config.json` of the running copy (git-ignored). Missing sections use the defaults in
  `lib/configuration/src/configuration.rs` (bots on port 6902, `start_map` empty, start at 156,191).
- **Scripts (NPC logic):** the Rust NPC scripts are in `scripts/` (one crate per module, built into
  `config/wasm/modules/`). NPC placements and names are in `config/wasm/npcs.json`, where each NPC names its module and
  entry. NPC warp destinations, as the reference lists them, are in `config/npc_warps.json`.

## Services

- Reference: `http://127.0.0.1:6904/db/...` and `/wiki/<Page>.md`, no key. Responses are cached in `cache/db/`
  (gitignored). Delete the cache when the server data changes.
- LLM: llama.cpp at `http://127.0.0.1:8080` (LFM2.5-2.6B, 2 slots). Thinking defaults to a budget of 2048 tokens (`--llm-think`); without `thinking_budget_tokens` it spends
  ~1000 tokens thinking. Do not use `chat_template_kwargs.enable_thinking=false`, which breaks grammar sampling.
- Decision model: llama.cpp `/v1/systemone` at `http://127.0.0.1:9931` (`--s1-url`), used with `--voice system-one`
  for chat, dialogs, the forum, notes and its `--llm-share` of the goals. A request must fit in the server's physical
  batch (`-ub`).
- Server logs: wherever the running server's output goes.

## Bot API

- Base URL: `http://127.0.0.1:6902`. Port 6901 returned 400 for every bot request, so do not use it.
- Auth: `Authorization: Bearer <key>` or `X-API-Key: <key>`. Every request except `GET /bots/SKILL.md` needs it.
- Docs: `GET /bots/SKILL.md` on the running server, or the source file above.
- Live test characters so far: `Rusty` (char 150003, in Geffen, unused by the rewrite) and `Goapy` (char 150002, the
  rewrite's test bot). Bot memory is in `state/<name>.json` (gitignored).
- Actions go to `POST /bots/{name}/actions`. Errors are `{"error": "..."}`.
- Forum: `/forum/...` on the same port, docs at `GET /forum/SKILL.md` (source: `server/src/server/forum/SKILL.md`). A bot
  sends the bot key with `X-Forum-As: <name>`. Rate limits are per character and reset when the server restarts.

## Running the bot

```
cargo run --release                                            # 50 bots from names.txt, decision model voice, llm-share 0.25
                                                               # each logs off in a town after 30-60 min (--session), a resting one of 100 (--pool) takes its place
cargo run --release -- --bots 10 --minutes 25 --log-dir logs  # one log per bot in logs/<name>.log
cargo run --release -- --server-dir ../rust-ro --stop-on-stdin  # key and addresses from the server's config.json; "stop" on stdin ends the run
cargo run -- --key-file ../rust-ro/config/bots_api_key.txt --name Goapy probe
cargo run -- --name Goapy run --minutes 10                    # one bot; add --no-llm for the deterministic strategist
cargo run -- --name Goapy run --voice llm                     # the LLM talks and picks every goal (--llm-share 1)
cargo run -- --name Goapy llm-check                           # one strategist step, prints prompt and decision
cargo run -- --key-file ... --name Alfa run --party Wayfarers --friends Alfa,Bravo --lead   # one process per member
# without --party a bot finds a party on the booking board by itself; --solo plays alone
cargo run -- db maps/prt_fild08                                # also: db grounds [map], db route from:to, db farm <item id>[@map]
```

Without `--key-file` or `--key`, the key is read from `../rust-ro/config/bots_api_key.txt`. `run` makes the character
of a name that has none and logs in the others. `names.txt` keeps the names of earlier runs first, so their characters
play on; `python tools/names.py <count>` adds more. `probe` creates the character if it does not exist, connects it if
it does not, and prints a summary. In Git Bash, a
leading `/db/...` argument is rewritten into a Windows path, so pass `maps/...` without the slash.
On this machine rustc sometimes crashes with STATUS_STACK_BUFFER_OVERRUN. If it does, set `RUST_MIN_STACK=67108864`.

## Working rules for this repo

- Test against the live server at `127.0.0.1:6902`. Deleting a bot (`DELETE /bots/{name}`) is irreversible, so only do
  it when asked.
- Treat `Rusty`'s state on the server as shared test state. The training-area exit is one-way.
- Do not change the running server's config, scripts, or key without asking. Changing the server config needs a restart
  of the server.
- Do not commit unless asked. Commit messages should end with the attribution line the harness provides.
- Parse observations defensively (`#[serde(default)]`). The server adds fields over time.
