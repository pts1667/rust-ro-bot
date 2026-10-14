# CLAUDE.md

Guidance for working on the rust-ro-bot repo.

## What this repo is

A Rust bot that plays a character on a rust-ro server through its Bot HTTP API, open-ended. GOAP (`src/goap/`) plans
and runs the steps. A small local LLM picks the goals through smolgent, using SKILL.state context management
(`src/strategist/`). Design, verified facts and status: `PLAN.md`.

## Related repos and locations

| Repo | Location | Role |
|---|---|---|
| rust-ro-bot (this repo) | `C:\Users\Thoma\Documents\sources\rust-ro-bot` | The bot. Rust crate. |
| smolgent | `C:\Users\Thoma\Documents\sources\smolgent`, https://github.com/pts1667/smolgent | LLM harness. The bot depends on the GitHub repo pinned to a commit (`Cargo.toml`). `ProviderConfig::extra_body` exists for llama.cpp's `thinking_budget_tokens`. |
| rust-ro (source) | `C:\Users\Thoma\Documents\sources\rust-ro` | The game server source. Bot API code is in `server/src/server/bots/` (`mod.rs`, `http.rs`, `auth.rs`, `controller.rs`, `command.rs`, `lifecycle.rs`, `observation.rs`), and the API docs are in `server/src/server/bots/SKILL.md`. Config defaults are in `lib/configuration/src/configuration.rs`. Git repo, branch `main`. |
| rust-ro (live test server) | `D:\downloads\ro-sppack\rust-ro` | The copy the test server actually runs from. Its key file, config, and compiled NPC scripts are here. Git repo. |

### Which copy to trust

- **Bot API key:** use `D:\downloads\ro-sppack\rust-ro\config\bots_api_key.txt`. The copy at
  `Documents\sources\rust-ro\config\bots_api_key.txt` is not the one the running server reads, and it returns 401.
  Never print the key or paste it into files, logs, or chat.
- **Server config:** `D:\downloads\ro-sppack\rust-ro\config.json` is the live config. The `bots` section is absent there,
  so the defaults apply (port 6902, `start_map` empty, start at 156,191).
- **Scripts (NPC logic):** the live copy is `D:\downloads\ro-sppack\rust-ro\scripts\src\generated\`. The most useful file
  for the training grounds is `m_pre_re_jobs_novice_novice.rs`. NPC placements and names are in
  `D:\downloads\ro-sppack\rust-ro\config\wasm\npcs.json` (entries like `Receptionist#nv1` map to script entry `13818`).
  The `config\npc` text files in the `Documents` checkout do not cover the training grounds, so do not rely on them
  for NPC behaviour.

## Services

- Reference: `http://127.0.0.1:6904/db/...` and `/wiki/<Page>.md`, no key. Responses are cached in `cache/db/`
  (gitignored). Delete the cache when the server data changes.
- LLM: llama.cpp at `http://127.0.0.1:8080` (LFM2.5-2.6B, 2 slots). Thinking defaults to a budget of 2048 tokens (`--llm-think`); without `thinking_budget_tokens` it spends
  ~1000 tokens thinking. Do not use `chat_template_kwargs.enable_thinking=false`, which breaks grammar sampling.
- Decision model: llama.cpp `/v1/systemone` at `http://127.0.0.1:9931` (`--s1-url`), used with `--voice system-one`
  for chat, dialogs, the forum, notes and its `--llm-share` of the goals. A request must fit in the server's physical
  batch (`-ub`).
- Server logs: `D:\downloads\ro-sppack\logs\server.log`.

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
cargo run -- --key-file "D:/downloads/ro-sppack/rust-ro/config/bots_api_key.txt" --name Goapy probe
cargo run -- --key-file ... --name Goapy run --minutes 10      # add --no-llm for the deterministic strategist
cargo run -- --key-file ... --name Goapy run --voice system-one --llm-share 0.25  # the decision model talks, takes notes, picks 25% of goals
cargo run -- --key-file ... --name Goapy llm-check             # one strategist step, prints prompt and decision
cargo run -- --key-file ... --name Alfa run --party Wayfarers --friends Alfa,Bravo --lead   # one process per member
# without --party a bot finds a party on the booking board by itself; --solo plays alone
cargo run -- db maps/prt_fild08                                # also: db grounds [map], db route from:to, db farm <item id>[@map]
```

`probe` creates the character if it does not exist, connects it if it does not, and prints a summary. In Git Bash, a
leading `/db/...` argument is rewritten into a Windows path, so pass `maps/...` without the slash.
On this machine rustc sometimes crashes with STATUS_STACK_BUFFER_OVERRUN. If it does, set `RUST_MIN_STACK=67108864`.

## Working rules for this repo

- Test against the live server at `127.0.0.1:6902`. Deleting a bot (`DELETE /bots/{name}`) is irreversible, so only do
  it when asked.
- Treat `Rusty`'s state on the server as shared test state. The training-area exit is one-way.
- Do not change files in `D:\downloads\ro-sppack\rust-ro` (config, scripts, or the key) without asking. Changing the server
  config needs a restart of the server, which only the user can do.
- Do not commit unless asked. Commit messages should end with the attribution line the harness provides.
- Parse observations defensively (`#[serde(default)]`). The server adds fields over time.
