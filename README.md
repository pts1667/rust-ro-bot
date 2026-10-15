# rust-ro-bot

Bots that play [rust-ro](https://github.com/pts1667/rust-ro) characters on their own. A bot leaves the training grounds,
hunts, levels up, spends its stat and skill points, sells its loot, buys potions, changes jobs, joins parties and talks to
NPCs and other players. It plays through the server's Bot API, so it is a character like any other in the world.

A planner (GOAP) does the playing. Goals come from built-in rules, or from a small local language model when one is
running. **The model is optional:** without one, bots play with the rules alone.

## What you need

- A running rust-ro server. Its Bot API (port 6902) and reference (port 6904) are on by default.
- Rust 1.88 or newer, and git: `cargo` fetches one dependency from GitHub.
- Optional: a [llama.cpp](https://github.com/ggml-org/llama.cpp) server for the goals and the chat (see [Language models](#language-models)).

## Quick start

1. **Start the server once.** On its first start it writes a random API key to `config/bots_api_key.txt`, in the folder
   the server runs from. Bots need that key, so keep it private.

2. **Build the bot.**

   ```sh
   git clone https://github.com/pts1667/rust-ro-bot.git
   cd rust-ro-bot
   cargo build --release
   ```

3. **Create a character and look at it.** `probe` creates the character if it does not exist, brings it into the game,
   and prints what it sees without acting.

   ```sh
   ./target/release/rust-ro-bot --key-file ../rust-ro/config/bots_api_key.txt --name Alfie probe
   ```

4. **Let it play.**

   ```sh
   ./target/release/rust-ro-bot --key-file ../rust-ro/config/bots_api_key.txt --name Alfie run --no-llm --minutes 30
   ```

   The log shows each goal and plan step, and a report comes at the end: levels, kills, deaths and zeny. Stop earlier
   with Ctrl+C: the bot finishes its current step, saves its memory, prints the report and leaves the game.

A new character starts as a level 1 Novice in the training grounds, exactly like a new player.

Change `../rust-ro/config/bots_api_key.txt` to wherever your server keeps its key. Instead of `--key-file`, you can set
`BOT_API_KEY_FILE`.

## Running several bots

Run one process per bot, each with its own `--name`:

```sh
KEY=../rust-ro/config/bots_api_key.txt
for name in Alfie Brannoc Cormac Delyth; do
  ./target/release/rust-ro-bot --key-file $KEY --name $name run --no-llm --minutes 60 > $name.log 2>&1 &
done
wait
```

- The server lets 20 bots in at once by default. For more, raise `bots.max_bots` in the server's `config.json` and
  restart the server:

  ```json
  { "bots": { "max_bots": 60 } }
  ```

- Names need at least 4 characters (`char_server.char_name_min_length`) and at most 19.
- A bot without `--party` looks for a party on the booking board and joins one, as a player would. Use `--solo` to keep
  it alone.
- To form a party yourself, give the leader `--lead`. Give every member the same `--party` and `--friends`:

  ```sh
  rust-ro-bot --key-file $KEY --name Alfie run --party Wayfarers --friends Alfie,Brannoc --lead
  rust-ro-bot --key-file $KEY --name Brannoc run --party Wayfarers --friends Alfie,Brannoc --role healer
  ```

  `--role healer` heads for Acolyte and then Priest. `--role tank` heads for Swordman and then Knight.

## Commands

| Command | What it does |
|---|---|
| `probe` | Creates and connects the character, prints a summary of what it sees. |
| `run` | Plays. See the options below. |
| `travel <map>...` | Walks the character to each map in turn, the way a goal's trip goes, and prints how each trip ended. |
| `db <path>` | Prints a `/db` entry of the reference, such as `db maps/prt_fild08`. |
| `db grounds <map>` | Lists the hunting grounds the bot would consider from a map. |
| `db route <from>:<to>` | Prints the route between two maps. |
| `llm-check` | Runs one goal choice with the language model and prints the prompt and the answer. |

Options of `run`:

| Option | Meaning |
|---|---|
| `--minutes N` | Stop after N minutes. |
| `--until-level N` | Stop at base level N. |
| `--no-llm` | No language model: the rules pick every goal, and the bot doesn't chat or visit the forum. |
| `--llm-share F` | Share of goal choices given to the model, from 0 to 1 (default 1). |
| `--voice llm\|system-one` | Which model talks and picks goals (see [Language models](#language-models)). |
| `--job <Job>` | Head for this job, first or second (`Hunter` goes through Archer). It also sets the stat build. |
| `--party`, `--friends`, `--lead`, `--role`, `--solo` | Party play (see above). |
| `--no-forum` | Never visit the players' forum. |

Global options work with every command:

| Option | Default |
|---|---|
| `--name` | `Rusty` |
| `--url`, or `BOT_API_URL` | `http://127.0.0.1:6902` |
| `--db-url`, or `BOT_DB_URL` | `http://127.0.0.1:6904` |
| `--llm-url`, or `BOT_LLM_URL` | `http://127.0.0.1:8080` |
| `--s1-url`, or `BOT_S1_URL` | `http://127.0.0.1:9931` |
| `--data-dir` | `.` |

## Language models

**`--voice llm` (the default)** uses an OpenAI-compatible llama.cpp server at `--llm-url`. The model picks a share of
the goals, answers players in chat and writes on the forum. Small models do: the bot was tuned with LFM2.5-2.6B. The
bot limits the model's thinking to 2048 tokens (`--llm-think`, 0 turns it off). If the server doesn't answer, the bot
says so in its log and plays with the rules.

**`--voice system-one`** uses a decision model, served by a llama.cpp build with the `/v1/systemone` endpoint at
`--s1-url`. It picks options and catalogue sentences instead of writing free text. Use it with `--llm-share 0.25`.

## What the bot keeps

| Path | What |
|---|---|
| `state/<name>.json` | The bot's memory: what it learned, its journal, counters, warps that failed. A character keeps playing where it left off. |
| `cache/db/` | Answers of the server's reference. **Delete it when the server's data changes** (maps, NPC scripts, items), or bots plan with the old data. |

Both live under `--data-dir` and are git-ignored. Deleting `state/<name>.json` makes the bot forget what it learned;
the character keeps its level and items.

A character stays on the server after the bot stops. To remove one for good, call `DELETE /bots/<name>` on the Bot API.
This can't be undone.

## Server settings that matter

All of these go in the server's `config.json`. Restart the server after changing them.

| Setting | Effect |
|---|---|
| `bots.max_bots` | Bots in the game at once (default 20). |
| `bots.start_map`, `start_x`, `start_y` | Where new bot characters start. Empty, the default, uses the normal start points. |
| `bots.idle_logout_secs` | A bot nobody sends requests for leaves the game after this long (default 1200). |
| `bots.command_group` | Permission group for the bots' `@` commands. Bots use `@go` to get out of places with no warp out, so the group needs it. Group 1, the default, has it. |
| `game.base_exp_rate`, `game.job_exp_rate` | Experience rates, for faster test runs. |
| `game.sim_speed` | Runs the game clock faster than real time, for test runs. |

## Troubleshooting

- **401 Unauthorized:** the key doesn't match the server's. Pass the `config/bots_api_key.txt` of the server that is
  running. A copy elsewhere may be stale.
- **400 on every request:** check the port. The Bot API is on 6902 (`bots.port`).
- **"bad character name":** a name from a file can carry a Windows line ending. Save name lists with Unix line endings.
- **A connect is refused:** the server is at `bots.max_bots`.
- **Bots try routes that no longer exist:** delete `cache/db/`.
- **In Git Bash, `db /db/maps/...` fails:** Git Bash rewrites a leading `/db/...` into a Windows path. Write
  `db maps/...` instead.
- **rustc crashes with `STATUS_STACK_BUFFER_OVERRUN` on Windows:** set `RUST_MIN_STACK=67108864` and build again.

## More

- The Bot API itself, for writing your own bot: `GET http://127.0.0.1:6902/bots/SKILL.md` on a running server.
- Design notes and status: [PLAN.md](PLAN.md).
