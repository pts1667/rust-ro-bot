# rust-ro-bot plan

An open-ended agent that plays a character on a rust-ro server through its Bot HTTP API. It levels, loots, sells, buys
potions, travels, spends its points and changes job. GOAP does the planning and execution, and a small local LLM
picks the goals.

## Environment

| Service | URL | Notes |
|---|---|---|
| Bot API | `http://127.0.0.1:6902` | Docs at `GET /bots/SKILL.md` (source: `rust-ro/server/src/server/bots/SKILL.md`). Key: `D:\downloads\ro-sppack\rust-ro\config\bots_api_key.txt`. |
| Reference | `http://127.0.0.1:6904` | `/db/<category>[/<key>]` (items, mobs, maps, warps, npcs, jobs, quests, …, `?q=&limit=`), `/wiki/<Page>.md`. No key. |
| LLM | `http://127.0.0.1:8080` | llama.cpp, LFM2.5-2.6B Q8 (with a DSpark draft model), 2 slots of 100k each (`-c 200000 -np 2`). Thinking is turned off with `thinking_budget_tokens: 0`, which smolgent sends through `ProviderConfig::extra_body`. `reasoning.enabled=false` is ignored, and `chat_template_kwargs.enable_thinking=false` breaks grammar sampling. |
| Server logs | `D:\downloads\ro-sppack\logs\server.log` | |

The LLM harness is [smolgent](https://github.com/pts1667/smolgent), as a git dependency pinned to `07a6421`
(trim-only compaction, lenient compaction ids, `stop_after_tools`, tool definitions in a fixed order).

## Architecture

```
reflexes (agent.rs)       dead -> respawn, stray dialog -> close, low HP in a fight -> heal,
                          status/skill points -> spend them, chat -> LLM helper reply
GOAP (goap/)              goal + WorldState -> plan (uniform-cost search) -> run the first step -> observe -> plan again
behaviours (behaviors/)   travel (warp graph, fights back or flees on the way, `@go` out of dead ends), exit training, hunt (target choice,
                          search walks, fight, loot), rest, shop (sell, buy), stats, skills, job change, dialog driver
strategist (strategist/)  async task. Builds an option menu in code and picks one with the LLM (a share of the steps,
                          --llm-share) or the deterministic fallback. Sends a Directive {goal, Σ} over a watch channel.
knowledge (knowledge/)    /db client with disk cache (cache/db), warp graph and routes, fight model, hunting grounds,
                          healing (item heal amounts from the /db script, heal skills, VIT/skill bonus)
memory.rs                 state/<name>.json: Σ, counters, visited maps, kills/deaths, potion shop, bad shops, bad warps,
                          journal
party/                    playing in a party (--party): forming it, party chat, following the leader, healer/tank roles,
                          potion trades
```

### Strategist: SKILL.state (arXiv 2608.26263), the default

- Each step is a new smolgent `ChatSession`. It gets P (the fixed system prompt in `strategist/prompt.rs`: task,
  guidelines, a Ragnarok Online primer, and compact worked examples from `strategist/examples.rs`), then a user turn
  with TASK and OPTIONS first, Σ (the `StrategicState` JSON), and O (character, map, healing, current goal, totals,
  last 6 journal lines). No history is sent.
- Tools: `monster_info`, `map_info`, `wiki_page` and `quest_info` (up to 8 calls a round, 8 rounds, answers not cut),
  then `commit {analysis, option, state_patch}`. A wrong option id gets an error and a retry.
- `StrategicState::apply_patch` validates the patch. Unknown keys and bad values are rejected one at a time. `null`
  resets a key. List keys are merged rather than replaced (`"-x"` removes an entry). Notes with numbers and objectives
  that are option ids are rejected; entries copied from the worked examples are scrubbed.
- Triggers: start, a 45 s period, level up, job level up, death, map change, and a goal finished or given up.
- A goal that fails is not offered again for 5 minutes.

### Strategist: history mode (`--context history`)

- One smolgent conversation for the run; each LLM step is a new user turn ("TURN n" + the same user message).
- The turn ends with the first valid `commit` (`stop_after_tools`). Asked for a closing line instead, the 2.6B model
  committed 11 times a turn, ~2.4k tokens a turn and 11 requests.
- Compaction is `CompactionConfig::trim_only(70_000, 35_000)`: the oldest exchanges are dropped by code, no model
  call. The model never made a valid compaction call (all 13 of a replay failed: `"turn_ids": 0`, unseen ids, the
  protected system prompt). smolgent `070a909` fixed those (ids shown, single ids accepted, bad ids skipped) and a
  replay then compacted fine (`history-check --model-compaction`), but trimming costs nothing.
- Each llama.cpp slot holds 100k, so two bots in history mode fit.
- The prompt prefix must not change between turns, or llama.cpp reads the whole conversation again: the tools are
  rendered into the system message. Two things changed it every turn: smolgent listed the tools in `HashMap` order
  (fixed in `07a6421`), and the `commit` tool carried the turn's option ids as an `enum` (history mode now sends it
  without one; the tool checks the id). A turn took 2 s at 3k tokens and 12 s at 57k before, ~1 s at 17k after.

### Option menu (strategist/options.rs)

`leave_training` (the only option while on `new_*`), `job_change:<job>` (job level 10 for a first job, 40 for a second;
only the `job_target` option when one is set), `hunt:<map>` (top 5 within 4 warps, from `knowledge::grounds`, with the
exp/s ratio to the current map), `sell_loot`, `buy_potions` (healing as a multiple of max HP), `rest`, and
`explore:<map>` (up to 2 unvisited maps within 2 warps).

### Parties (party/)

A bot knows its party only as a player does: the party window (members, their map, and on the same map their position
and HP bar), party chat, and what is in sight (`SIGHT`, 14 cells). Each bot is its own process; nothing passes between
them another way.

- `--party <name> --friends A,B,C [--lead] [--role healer|tank|fighter]`. The leader creates the party once it has
  Basic Skill 7 (the server's rule for creating one), invites the friends that are not in it every minute (an
  invitation fails while the friend is out of the game), and turns experience sharing on (refused while levels are more
  than 15 apart). The others accept invitations from friends only.
- Party chat (`party/chat.rs`), plain words a person can type too: `hunting at <map>` (the leader, when its hunt
  goal changes), `level <base>/<job> <Job>` (on joining and every level), `follow me`, `stay here`, `heal please`
  (below 40% HP with a healer in sight), `out of potions`, `sending potions to <name>`, `brb, <errand>` and `back`.
- A member follows the leader (or whoever said `follow me`): to its map with the travel behaviour, then within 7 cells.
  It fights, in order: a monster attacking a member, what the leader hits (its sprite shows it), the best target within
  10 cells of the leader. Errands (sell, buy potions, job change, leaving the training grounds) still run as planned,
  announced with `brb`. A leader away on an errand is not followed.
- Every bot, solo or not, leaves monsters to other players: one that attacks an outsider or that an outsider hits is
  not a target. A monster that attacks a member is worth 4x as a target. The leader does not pull more while a member
  in sight is below 30% HP with nothing attacking it.
- Roles: healer (Acolyte, then Priest; caster build) heals the member in sight with the lowest HP bar under 60% (or
  who asked) and keeps Blessing and Increase AGI up (cast times remembered, every 60 s, with half its SP or more);
  tank (Swordman, then Knight; tank build) provokes monsters that attack another member. A role pins `job_target` and
  `stat_build`: the strategist's patches to them are refused.
- Potions: a bot with more than 8 healing items gives what it has over 8 (5 at most) to a member in sight who said
  `out of potions`, through the trade window (request, offer, lock, confirm). The nearest member goes first (it waits
  0.4 s a cell, plus a bit by name for ties) and says `sending potions to <name>`; the others hearing it leave it.
  A bot takes trade requests from members only, locks once the partner locked, then confirms.

## Verified facts

- Arrivals often land a cell off the warp back (`prt_fild07` -> `prt_fild05` lands at (142,18), next to the warp at
  (134,14) that spans 14x3 cells). A drop or a monster on the warp cells used to draw the bot onto it, back, and again,
  every 1.5 s for minutes. Targets, loot, search walks and follow walks now keep 2 cells off warps
  (`Observation::near_warp`).
- New characters spawn on `new_1-1` at (53,111). The receptionist on `new_1-2` has the option "Direct access to
  Ragnarok Online.", which sends the character to a random town. This exit is one-way.
- NPC names in the observation drop the `#suffix` that `/db` keeps (`Tool Dealer` vs `Tool Dealer#prt`). Match shops
  by position. Some shops stand behind a counter (Tool Dealer on `alberta_in`) and every `use` is refused: they go to
  `bad_shops`.
- `/db/items/501.sold_by` lists the potion shops with map, x, y and price. Maps have a `town` flag. Prontera has a
  `Job Master`. Item heal amounts are in the item `script` (`itemheal rand(45,65),0;`).
- Some maps list no warps of their own (`moc_fild20`); the warps that land on them are in `arrivals`, and the graph
  adds the way back from those as guessed edges. Guesses can be wrong: `moc_fild20` is a guarded zone whose exits are
  Continental Guard NPCs that refuse to let anyone out (one offers a warp to Morocc), and the sewer exit landing on
  `prt_fild05` (274,208) has no warp back (the entrance is the Culvert Guardian quest). A warp that 5 unhindered walks
  could not take goes to `bad_warps` (`from:x,y>to`); a failed guess rules out every guess of that map (`from:*`).
  Routes, hunt and explore options leave them out.
- Interior maps hold several rooms (`geffen_in`), and some fields have closed-off parts (`prt_fild04`): a warp the bot
  cannot get to is refused with `No reachable cell from where target … can be used`. That warp is skipped for the trip
  only, not recorded. Into the shop's map, travel takes the warp landing nearest to the shop.
- `arrivals` also list NPC transports; only warps the other map lists count as real.
- `@go <town>` is allowed for bots (`bots.command_group`). `@go morroc` lands on `old_moc`, `novice` on the training
  grounds. Travel uses it once a trip, when there is no route or it is stuck, to the town with the fewest real warps
  to the destination (searched backwards through `arrivals`); no such town, no `@go`.
- Mob `mode & 0x4` is aggressive (Mandragora `0x84`). Poring `0x83` and Lunatic `0x81` are passive.
- Observation `weight` uses the same unit as `/db` item weight (tenths). Max weight is about the job base (Novice
  20000) plus 300 per STR.
- Server rates: 5x base and job exp, 1% base exp lost on death.
- llama.cpp tool calls do not enforce the `enum` of a tool argument: the model can commit option ids that were not
  offered.

## Commands

```
cargo run -- --key-file "D:/downloads/ro-sppack/rust-ro/config/bots_api_key.txt" --name Goapy probe
cargo run -- --key-file ... --name Goapy run [--minutes N] [--until-level N] [--no-llm] [--llm-share 0.5]
    [--context skill-state|history] [--order task-first|data-first] [--strategist-period 45]
    [--party Wayfarers --friends Alfa,Bravo,Charlie [--lead] [--role healer|tank|fighter]]
cargo run -- --key-file ... --name Goapy llm-check                  # one SKILL.state step on the live state
cargo run -- --key-file ... --name Goapy history-check --turns 10 [--compact-at N --compact-to N] [--model-compaction]
cargo run -- llm-eval [--n 5] [--order ...] [--no-examples]         # 8 fixed scenarios, no game server needed
cargo run -- db maps/prt_fild08 | db grounds [map] | db route geffen:prt_fild05
```

Sampling: `--llm-temp` (default 0.3), `--llm-top-p`, `--llm-top-k`, `--llm-min-p`, `--llm-repeat-penalty`,
`--llm-presence-penalty`, `--llm-frequency-penalty`, `--llm-think` (thinking budget, default 0).

## Status

- Rewrite done and tested live. Best runs from a fresh character, 20 minutes: base 15 / job 9-10 (SKILL.state at 50%
  LLM, and at 100% with temp 0.7). The code-only strategist is about as good.
- The 2.6B model anchors on the current goal: on the offline eval it scores 38-56% whatever the sampling settings.
  A bigger model is the next thing to try.
- 2026-10-12, fresh characters, 20 min at 50% LLM: SKILL.state (Golf) base 15 / job 10 and the first job change of the
  rewrite (Swordsman); history mode (Foxtrot) base 13 / job 9. Then 45 min more each: Golf base 26 / job 19, Foxtrot
  (100% LLM, history) base 24 / job 19. History mode commits once a turn, but still picks the current map ~40 turns
  in a row.
- Travel fights back what the fight model says it can beat (with 20% HP to spare) and otherwise walks on to the warp,
  healing below 50%. Seen fleeing a Scorpion on `moc_fild20`. A lv 13 Novice died crossing `gef_fild02` (Choco lv 43), a lv 31
  Swordsman crossing `ra_fild08` (Drosera lv 46). Now a map with an aggressive monster whose two hardest hits kill the
  bot from full HP is not offered for hunting, and routes go around it when another way exists.
- A reflex loop (status points no stat of the build could take) stopped all three bots for 20-45 minutes; fixed.
- 60 min from base ~28, 2026-10-12: history mode at 100% LLM 28 -> 33, SKILL.state at 50% 28 -> 32, code only
  28 -> 33. The strategists are still about even.

## Open items

- Equipment upgrades (buying or equipping better gear) are not implemented.
- Bad warps are learned per character (could be shared).
- Weapons stay the starting ones: a lv 31 Swordsman flees a lv 24 Scorpion. Equipment is the next limit.
- Storage, pets, vending and the WebSocket API are out of scope for now.
