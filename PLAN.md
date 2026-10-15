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
| Decision model | `http://127.0.0.1:9931` | llama.cpp `/v1/systemone` (Cloudflare clef-flash 9B Q6_K, 2 slots), for `--voice system-one`. A request is `{state, questions}`; a question is a `noul` (yes chance) or a `choice` (at most 255 options); the answer has a probability per option. The whole request must fit in the physical batch (`-ub`, 8192 now); ~28 tokens per option, ~0.15-0.5 s per request. |
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
forum.rs                  with a voice only: on arriving in town, sometimes (always with a reply waiting) visits
                          the players' forum: reads, comments, posts, takes notes into Σ
voice/                    --voice llm (free text, the LLM strategist) or system-one (the decision model picks
                          sentences of a catalogue filled with what the bot knows, its share of the goals
                          (strategist/decided.rs), and notes from its own experience)
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
`explore:<map>` (up to 2 unvisited maps within 2 warps, the insides of towns included).

- Curiosity (`behaviors/curiosity.rs`): on a map it went to explore, the bot talks to up to 3 NPCs it does not know;
  on the way anywhere, it stops for an unknown NPC within 12 cells with a 15% chance (one roll per NPC and run, 90 s
  at least between stops, never while following a party member); entering a town, with a 10% chance it goes to one
  unknown NPC anywhere in its part of the town. Where any will do (a town, a map explored), the decision model (or
  the LLM) picks among the 10 nearest, shown with their distance and quest icon; without a voice, a quest icon wins,
  else the nearest. One NPC per name a map (four Guards are one). An NPC the bot could not reach is left alone for
  the run. Shops, Kafras, warpers and the NPCs of the written quests are known already. The talk asks to hear more (`curious_option`: entries like "Tell me about...", "Who are you?")
  and leaves when an entry or the page speaks of money, gear, a job or a trip, or when asked for an amount. What it
  did (sells things, started a quest, sent the bot somewhere, gave or took items or zeny, its first menu) goes to
  `npcs_met` in the bot's memory and to the journal (`met <npc> on <map>: ...`); the forum shares those that did
  more than talk or show a menu. The bot cannot follow a quest it discovers this way: only the written ones.

- Hunting grounds count the warps of a way that crosses no deadly map (`safe_hops`: an aggressive monster with 5 or
  more spawns that kills the bot in two hits), and a ground with no such way is not offered. mjo_dun01 used to show
  "threat 0%" while the only way in crosses mjolnir_02 (Flora, Petite): five deaths in one run. Maps the bot died
  crossing in the last 30 minutes are left out of its routes and grounds (`Bot::deadly_ways`), and dying on the way
  gives the goal up at once.
- A party leader picks grounds for its weakest member in the game: its own fight model scaled to that member's level
  (`party_fighter`).
- First job: a Novice without a `job_target` (and no party role) gets one at random among the jobs its stats still
  suit (any while they are as created; else those of its build, e.g. Swordman, Merchant or Thief for melee), and the
  build follows a job taken while the stats are fresh. Before, every bot kept the default melee build and became a
  Swordman; asked, the decision model picked Swordman 7 times in 10.

### Parties (party/)

A bot knows its party only as a player does: the party window (members, their map, and on the same map their position
and HP bar), party chat, and what is in sight (`SIGHT`, 14 cells). Each bot is its own process; nothing passes between
them another way.

- By default (no `--party`, no `--solo`) a bot finds a party by itself on the game's party booking board
  (`party/recruit.rs`). Without a party it searches the board every 45 s and whispers whoever put up an ad within 10
  levels (`party? level 12/8 Novice`), then takes the invitation that comes back; it also takes invitations from
  someone in sight within 10 levels. Finding nobody, a bot with Basic Skill 7 creates `<name>'s party` and puts up an ad;
  it invites whoever whispers it with a level within 10 (`sorry, the party is full` past 4 members, or `sorry, our
  levels are too far apart`), and takes the ad down once full. Two lone leaders merge: the one with the newer ad leaves
  its party and asks the other. A member leaves when the leader has been out of the game for 10 minutes, or is more
  than 15 levels away. The role follows the job (Acolyte line heals, Swordman line provokes) and pins nothing.
  Verified live: Alfa and Charlie created parties at the same second, Alfa joined Charlie's within a minute, and Bravo
  (who cannot create one) found the ad and joined after its errand.
- `--party <name> --friends A,B,C [--lead] [--role healer|tank|fighter]`. The leader creates the party once it has
  Basic Skill 7 (the server's rule for creating one), invites the friends that are not in it every minute (an
  invitation fails while the friend is out of the game), and turns experience sharing on (refused while levels are more
  than 15 apart). The others accept invitations from friends only.
- Party chat (`party/chat.rs`), plain words a person can type too: `hunting at <map>` (the leader, when its hunt
  goal changes), `level <base>/<job> <Job>` (on joining and every level), `follow me`, `stay here`, `heal please`
  (below 40% HP with a healer in sight), `out of potions`, `sending potions to <name>`, `brb, <errand>` and `back`;
  whispered: `party? level <base>/<job> <Job>` and `sorry, <reason>`.
- A member follows the leader (or whoever said `follow me`): to its map with the travel behaviour, then within 7 cells.
  It fights, in order: a monster attacking a member, what the leader hits (its sprite shows it), the best target within
  10 cells of the leader. Errands (sell, buy potions, job change, leaving the training grounds) still run as planned,
  announced with `brb`. A leader away on an errand is not followed. Nor is one more than 6 warps away, or only
  reachable through a deadly map, or on a deadly map: the member plays by itself meanwhile (checked every 2 minutes).
  Members used to go 30 warps after a leader in Rachel, and 15 died on the way in one run.
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
- `arrivals` also list NPC transports; only warps the other map lists count as real. `transports` lists the NPCs that
  send a character somewhere, with `checks` when there is a condition: no way back is guessed to a map such an NPC
  leads to (the Culvert). A warper's `to` is a link (`"/db/destinations"`), not a list: parsed strictly, it made every
  town with a warper drop out of the graph.
- The server goes around other warps' cells on a walk its pathfinder finds (100 steps), but walks a longer one in legs
  found over the whole map, through door cells (`way_around_warps` returns `None` when the direct path is empty).
  Out of an `alberta_in` door at (102,153), the walk to the `pay_fild03` exit went back in at once, 2840 times in one
  run. Travel now walks short legs (10 cells) toward the warp while another warp is within 12 cells, and a walk that
  ends on another map than planned counts as a failure (4 a trip at most).
- `@go <town>` is allowed for bots (`bots.command_group`). `@go morroc` lands on `old_moc`, `novice` on the training
  grounds. Travel uses it once a trip, when there is no route or it is stuck, to the town with the fewest real warps
  to the destination (searched backwards through `arrivals`); no such town, no `@go`.
- Mob `mode & 0x4` is aggressive (Mandragora `0x84`). Poring `0x83` and Lunatic `0x81` are passive.
- Observation `weight` uses the same unit as `/db` item weight (tenths). Max weight is about the job base (Novice
  20000) plus 300 per STR.
- Server rates: 1x base and job exp (`base_exp_rate`, `job_exp_rate` in the server's `config.json`), 1% base exp lost
  on death.
- llama.cpp tool calls do not enforce the `enum` of a tool argument: the model can commit option ids that were not
  offered.
- Trading needs Basic Skill 1 on both sides (`Trading needs Basic Skill level 1`), and the range is 2 cells. The server
  silently drops a trade request to someone who has a party invitation to answer: invite after the trade, or let them
  answer first.
- A second party invitation to someone who has one pending is refused (`… already has an invitation to answer`).

## Commands

```
cargo run -- --key-file "D:/downloads/ro-sppack/rust-ro/config/bots_api_key.txt" --name Goapy probe
cargo run -- --key-file ... --name Goapy run [--minutes N] [--until-level N] [--no-llm] [--llm-share 0.5]
    [--context skill-state|history] [--order task-first|data-first] [--strategist-period 45]
    [--party Wayfarers --friends Alfa,Bravo,Charlie [--lead] [--role healer|tank|fighter] | --solo]
cargo run -- --key-file ... --name Goapy llm-check                  # one SKILL.state step on the live state
cargo run -- --key-file ... --name Goapy history-check --turns 10 [--compact-at N --compact-to N] [--model-compaction]
cargo run -- llm-eval [--n 5] [--order ...] [--no-examples]         # 8 fixed scenarios, no game server needed
cargo run -- db maps/prt_fild08 | db grounds [map] | db route geffen:prt_fild05 | db farm 507@prontera
cargo run -- --key-file ... --name Nessa run --voice system-one [--s1-url http://127.0.0.1:9931]
cargo test forum_decided -- --ignored --nocapture                  # a decided forum visit on the live decision model
cargo test chat_on_the_decision -- --ignored --nocapture           # chat answers and dialog choices on it
cargo test lessons_on -- --ignored --nocapture                     # own-experience notes on it
```

Sampling: `--llm-temp` (default 0.3), `--llm-top-p`, `--llm-top-k`, `--llm-min-p`, `--llm-repeat-penalty`,
`--llm-presence-penalty`, `--llm-frequency-penalty`, `--llm-think` (thinking budget, default 2048; dialog and chat answers, which are grammar-constrained, always send 0).

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

- 2026-10-13, 5 h 20 min, code-only strategist (`--no-llm --solo --job …`), seven bots from base ~16, server at 10x
  for the run: Charlie (Swordsman) 45/33, Lancel (Swordsman) 44/32, Anvil (Merchant) 41/35, Ember (Mage) 28/21,
  Fletch (Archer) 28/22, Mercy (Acolyte) 23/16, Shade (Thief) 21/15. Deaths: Mercy 53, Charlie and Fletch 6,
  Ember 4, Lancel 3, Anvil 1, Shade 0. Gear buying works (Katana, Guard, Hat, Jacket, Sandals, Bandana). This run
  used the binary from before the chat work and the next-job gear filter (Mercy still bought an Axe).

### Forum (forum.rs, api/forum.rs)

The server has a text forum under `/forum` on the bot API port (docs: `rust-ro/server/src/server/forum/SKILL.md`). A
bot signs in with the bot API key and `X-Forum-As: <its name>`. Limits: one thread per 15 minutes and one comment per
60 s per character (5 a minute since 2026-10-13, server change), `429` with `retry_after`; 300 threads kept.

- Only a bot that plays with the LLM uses it (`--no-forum` turns it off). The task runs beside the agent on the
  published snapshots and never blocks it. Its memory is `state/<name>.forum.json`: threads read (by comment count),
  comments written per thread, threads started, findings shared or waiting, replies not seen yet, the last reply `seq`.
- When: on arriving in a town from outside (a map change onto a `town` map, a `GO_TOWNS` one, or a house of a town: no
  monsters and a warp out to a town, like `prt_in`), with a 25% chance and at least 10 minutes after the last visit;
  always when a reply is waiting. The forum sees the maps of the agent's snapshots, so a town passed through inside one
  travel step does not count. Replies come from `/forum/ws?after=<seq>`
  (tokio-tungstenite), kept open and reopened after 30 s.
- A visit is a smolgent session with the tools `read_thread`, `comment`, `post_thread`, `take_note` and `leave`
  (8 model turns at most). It opens with the replies to the bot, the 12 latest active threads without those it read
  that have no new comments since (marked new / new comments, and yours), the bot's notes, its own experience (kills, deaths, monsters that keep killing it, last journal lines) and
  findings others could use (journal lines: a monster that keeps killing it, a job change, a quest finished or stuck,
  a shop that would not open, every 5th base level on a field or dungeon map).
- Code limits: 4 reads and 5 writes and 4 notes per visit, within the server's limit of 5 comments a minute; a comment only in a thread read this visit, not when the
  bot wrote the last word, at most 3 per thread; a thread at most once an hour and not when a listed title is the same
  subject. A write or note that names a map or a proper name the bot did not read or see this visit is refused, and so
  is a sentence where the bot says it did something ("I tried", "I hunted", ...) with a place or monster its own
  experience does not name. A call refused once is answered "refused already" when repeated.
- Notes go to the strategist as `Event::ForumNotes`, prefixed `forum: `, and through `apply_patch` into Σ `notes`
  (6 at most, the oldest go). Notes may now hold map names (`prt_fild05`); other digits are still refused.
- With the 2.6B model on a mock forum (`cargo test forum_visit -- --ignored --nocapture`, and `forum_quiet_visit`
  without a reply). Thinking off: it answered the reply in about half the visits, sometimes took a note, often read
  and left. Thinking on (2048, the default now): 500-1300 characters of reasoning a turn, 4-20 s a visit; after
  reading, it writes its comment as plain text, which posts nothing, so the first plain answer gets a reminder and a
  forced tool call. Then it commented in every visit, took a note now and then, never started a thread. The claim check
  refused 0-3 comments a visit ("I tried the fields around Payon"); what it cannot see still gets through ("I've been
  grinding Porings there", hearsay stated as fact).
- With Tiel-Coder-35B-A3B (Q4_K_M, 2026-10-13), 5 visits with thinking at 2048: 45-105 s a visit, up to ~7700
  characters of reasoning a turn. It read every thread, called tools without the reminder, answered both questions
  with its own facts (34 deaths on prt_fild05, base 15 on prt_fild08), said "haven't been to Payon, can't vouch for
  it" instead of passing on hearsay, reworded a note refused for a number, and took a note in 2 visits. In 3 visits it
  tried to start a thread on the Ammunition Dealer shop, refused because the two writes were used up. The claim check
  refused "I haven't been around Payon yet" (a denial, not a claim). Small inventions remain ("before I switched
  tactics"), and once it spelled "Ammunishment".
- The same model with lower reasoning effort and a faster server configuration, 5 visits: 31-59 s a visit, mostly
  100-1800 characters of reasoning a turn (one turn ~4900). The quality was the same: it answered both questions with
  its own facts in every visit and took one note. Once it posted the Ammunition Dealer thread (the cap then refused its
  second comment); once more the cap refused a thread. The claim check refused nothing. One stretch: "I've been stuck
  on prt_fild05 so far" after it had moved to prt_fild08.
- First live run, 2026-10-13: 10 new characters at `--llm-share 0.2`, llama.cpp with 2 slots, stopped after 6 min.
  The forum worked: Elmwood started a thread on arrival in alberta ("new Novice looking for tips") and another bot
  answered it from izlude. But the LLM was the bottleneck: requests waited in llama.cpp's queue and the 90 s timeout
  dropped most of them (4 of 6 strategist steps, 2 forum visits; llama.cpp logs `stop: cancel task`). `LLM_TIMEOUT`
  is now 300 s, which counts the time waiting for a slot. Character names need at least 4 letters ("Elm", "Ivy" were
  refused).
- Second run, 2026-10-13 21:59-22:30 UTC: 10 new characters (Alder ... Jasper), `--llm-share 0.2`, llama.cpp with 5
  slots of 40k. 295 strategist steps, 49 by the LLM, one timeout at 300 s (all 5 slots busy most of the run). Base
  5-8, job 4-6 at the end, 2 deaths (Drosera, Stapo crossing `ra_fild08`). The server answered `503 The game loop did
  not answer in time` 26 times; no server errors or panics.
  - **Warp loop around prt_gld (most of the lost time).** 9 of 10 bots chose `prt_fild01` (prontera -> prt_gld ->
    prt_fild01) and bounced prontera -> prt_castle -> prt_gld -> prontera until "too many warps", 1188 hops and 82 give-ups
    in all, then the fallback chose `prt_fild01` again. Two server warp overlaps cause it: `prontera,155,360` (1x1, to
    `prt_gld`) lies inside `prt005` at `prontera,156,360` (4x2, to `prt_castle`), which wins; and `prt_castle,102,181`
    lands on `prt_gld,159,28`, inside the `prt_gld,159,29` warp back to prontera. The only real way into prt_gld from
    there is from `prt_fild01`'s south edge. Fix in the route graph: drop a warp whose area lies inside another's, and
    follow a landing that falls on a warp (done: `live_warps`, `covers` in `knowledge/graph.rs`; of 378 cached maps
    only these two warps change). A guessed way back (from `arrivals`) is not made up for a destination the map has a
    warp to, even a hidden one: without that, run 3 went prontera -> prt_gld by the guess at (155,355) and travel took
    the hidden warp again. prontera -> prt_fild01 now goes prt_fild05 -> mjolnir_09 (3 warps). The fallback chose the map again because `give_up` sent `GoalFailed` before the next
    snapshot carried the failure; now `give_up` puts it in the published snapshot first, and the agent ignores a
    directive for a goal it gave up in the last 5 minutes.
  - The forum: one visit in 30 minutes (Garnet, a welcome on Elmwood's thread). Bots entered a town about 19 times in
    all (25% chance each); after the first hunt they stay on fields, and the warp loop passes through prontera too fast
    to count. Houses of towns count as town now.
  - Since this run a strategist step does not queue at llama.cpp: when `/slots` shows every slot busy, the fallback
    decides (`Llm::slot_free`). Forum visits and dialog or chat answers still queue.
- Run 4, 2026-10-13 22:49-23:19 UTC, server at 5x base and job exp: 10 new characters (Aldous ... Joss),
  `--llm-share 0.2`, 5 slots. Base 6-12, job 4-8. 217 strategist steps, 25 by the LLM; 32 steps went to the fallback
  because every slot was busy; no LLM timeouts. 4 deaths, all on the way somewhere (Drosera x2 on ra_fild05, Argos on
  prt_fild01, Flora on mjolnir_10). Server: 7 `503`s, and 3 observation requests of Isla timed out after 60 s.
  - The prt_gld loop is gone (1 "too many warps" against 82). One new loop: Hollis bounced prt_in -> prontera -> prt_in
    19 times, as the walk from the house door to the west gate stepped on a door warp. The server warps a character
    on any warp cell it walks onto (`game_loop.rs:424`) and its walking paths do not avoid warp cells. (Also
    `set_warp_cells` in `model/map.rs` marks `x - x_size .. x + x_size` with an exclusive end, one cell short of the
    rectangle on the right and bottom, and nothing at all for a size of 0.) Fixed in the server (`../rust-ro`,
    `bots/command.rs`): when the walk of `move` or `approach` would step on a warp cell other than the target's, the bot
    API walks a way around it on the cells with those warps blocked (`around_warps`, a leg the pathfinder takes without
    a warp, then the caller asks again as for long walks). Only bot walks change; players and monsters path as before.
    Unit test only, not tried live yet. The cell marking is fixed too: `set_warp_cells` now marks the whole rectangle `x ± x_size`, `y ± y_size`
    (inclusive, cut at the map's edges), as `get_warp_at` already read it. Checked on the 2912 warps of
    `scripts_warps.conf`: no landing falls newly inside a warp and no two warps to different maps newly overlap; the one
    warp of size 0 (`in_hunter` 100,15 to pay_fild10, the Hunter guild's way out) gets its cell and now works.
  - Strategist LLM steps took 67-666 s (median ~2 min): the timeout is per model turn, and a step with lookups
    (quest_info, map_info) is several turns, each waiting for a slot. The slot check only gates the start.
  - The First Aid quest's herbs (3 Red Herb, 3 Clover) sent base 6 Novices to yuno_fild01 through mjolnir_09/10:
    two of the deaths, and Aldous, Gwen and Hollis ended at base 6. Cause: `farm_ground` looked only at the 8
    monsters with the best drop rate (Permeter, Goblin, ...), which live far away; the Lunatic next to every town was
    never considered. Now every dropper counts, on the maps within reach (one breadth-first search), a way there that
    crosses a map with a two-hit killer does not count (`safe_hops`, the check travel makes), and distance weighs more
    (value / (1 + warps)). Red Herb from prontera, izlude, payon, alberta: Lunatic one field away (pay_fild03 from
    payon); from morocc, Picky on moc_fild12. `db farm <item id>[@map]` (with `--name`) prints the choice. The graph
    now keeps each map's warps once read: a farm lookup went from 30-47 s to under a second, and every route is
    faster.
  - prt_fild01 is now 3 warps away (prt_fild05, mjolnir_09) and still chosen for its exp/s: Hollis spent 10 minutes
    getting there.
  - Isla's agent stood still 3 minutes twice (23:12:50-23:16:03): its observation requests timed out at 60 s while
    the server answered other bots in milliseconds. Not explained yet.
  - The forum: 9 visits, all comments in the one thread (Elmwood's "new Novice looking for tips", 11 comments now,
    most of them welcomes); no new threads, 1 note. Five comments checked against the journals were true; one mixed in
    a monster from another comment ("farmed the Lunatics" on pay_fild03): a name read in the thread passes the claim
    check.
  - The second half of the run went mostly to the First Aid quest (job level 4: Payon -> Prontera on foot for the
    Payon starters, ~10 minutes).

### Decision-model voice (voice/, forum/decided.rs, strategist/decided.rs)

`--voice system-one` replaces the LLM everywhere: chat, NPC dialog choices, the forum and the forum notes, its share
of the goal choices, and notes. `--voice llm` is the default and unchanged. One of them is used per run.

- Goals: with `--llm-share` (0 to 1) a strategist step goes to the decision model: one choice among the option ids,
  each described by its option line, against guidelines (those of the LLM prompt, shortened), objective, job target,
  notes and the LLM's observation text. Drawn close to the model's pick (0.3). A failure falls back to the
  deterministic choice. The objective and job target stay with the code and the lessons.
- Own notes: before a step, when the journal or the deaths changed since the last look and 5 minutes have passed, the
  model sees the lessons the bot's own experience suggests and the notes do not hold yet (`Known::own_lessons`: avoid
  a monster that killed it twice or beats it, careful on a map it died on, a shop that would not open, a map where it
  levelled near its level, a monster it killed 20 times without dying to it), and a yes/no ("would one of these, not
  yet in the notes, help?"). On yes it picks one, then maybe a second (or "nothing more"), a call each. They go to Σ
  like the forum lessons, without the "forum: " mark, with the same effects. The first phrasing ("did something
  happen lately worth a lesson...?") gave 0.27 with no notes at all; listing the lessons and asking whether one helps
  gave 0.95 with none noted and 0.10 with the main one noted. Live probe: two deaths to Poison Spore on prt_fild05
  gave "Avoid Poison Spore while weak" and "Be careful on prt_fild05", 0.8 s for the three calls.

- What the bot says comes from a catalogue (`voice/phrase.rs`), as in the Dark Souls messages: templates with slots in
  categories (greeting, reply, lore, warning, news, request, farewell) and conjunctions that join two of them, at most
  two sentences per message. `lesson` templates are notes for Σ, never said.
- The slots are filled only with what the bot knows (`voice/known.rs`): monsters in sight, kills and deaths, the
  journal (job changes, levels on a map, quests, shops that would not open), the goal's map, and what the reference
  says about the names in what it heard or read (`social::facts::lore`: where a monster lives, what it drops, who drops
  an item, where it is sold). Each template has its conditions (no "Haven't seen Fabre" after killing Fabres), so
  whatever the model picks is true, and the text checks of the LLM forum are not needed.
- A message (`voice::compose`) is one or two walks through a menu (`voice/menu.rs`), the way a player would use
  one: the topics, then the sentences of one, where Back returns to the topics (at most twice per walk). Each request
  shows the whole menu below the situation: every topic with its description (the open one marked; while in a topic
  they are shown but are not choices), then the history of this menu, numbered, with each Back spelled out ("3.
  Selected Back: none of the sentences in react fit. Returned to the topics.") and what that topic offered, then
  where the model is now and how many Backs are left. A second sentence: one request says whether one is needed and
  about what (never a bare reply), a walk starting in that topic picks it, and last the conjunction is picked with
  both in view. Answers are drawn from the probabilities (the model gives the same answer to the same request),
  sharper for the topic and the conjunction. A yes/no gate alone was unreliable in chat (silence to questions, an
  answer to "bye"); there "say nothing" is a choice next to the topics instead. Walks are logged at `voice=debug`
  (`steps=bye > back > items > "Fabre drops Fluff"`).
- Topics (`phrase::TOPICS`, 22, each with a description for the model) group the spoken templates more finely than
  the kinds (greet, accept, decline, cheer, comfort, thank, clarify, where monsters are, how strong, items, danger, my level,
  my maps, my fights, shopping, errands, status, plans, party, ask for information, ask for help, bye); the kinds stay for the code's rules (no greeting, reply or
  farewell as a second sentence, `COMMENT_KINDS`). Compared live on 24 chat lines with their right answers
  (`cargo test voice_eval -- --ignored --nocapture`, 63 sentences, 2026-10-14, the server busy with a 50-bot run):

  | First pick | Expected right (probability mass) | Top path right | ms per message |
  |---|---|---|---|
  | one flat choice over every sentence | 56% | 22/24 | 835 |
  | the 7 kinds, then a sentence | 51% | 18/24 | 717 |
  | the 14 topics, then a sentence | 70% | 23/24 | 714 |

  The kinds lost on "news" (23 templates): "I just reached level 10!" and "I love this game" went to it. The topic
  descriptions were sharpened against this set (sympathy under react, "the ones around you now", what you bought
  or wear, ask_help as "a request of your own, not a reply to theirs"), so the topic numbers are a little
  optimistic. "What's your job level?" got "I'm a level 9 Novice" everywhere until the state said the job level
  (`Known::about`: "level 9 Novice (job level 7)"): every fact a sentence states must be in the state, or the model
  will not pick it. More templates can go into the topics: the second call only sees its topic's sentences (12 at
  most now).

  The menu with Back, in the same eval at the model's best picks: 24/24 end right (~1 s per message), and no Back
  was taken from a topic that fit. Started inside a wrong topic on purpose (6 cases: "what does Fabre drop?" in bye,
  "what level are you?" in danger, ...), it selected Back in 6/6 and ended right in 6/6 (`bye > back > items >
  "Fabre drops Fluff"`). With Back described as "none of these fits, return to the topics" it stayed in my_maps for
  "ok, I have to go now, bye" (5/6); "none of these sentences fits the conversation; return to the topics and pick
  another" fixed it. Echoes are the menu's typical miss: "can you wait for me?" answered with "Wait for me" (the
  request topic) until ask_help said it is the bot's own request.

  Then 32 templates were added at once (sympathy, cheering, "what about you?", busy, clarify, a monster near or far
  above the bot's level, "no X here", what it fights now, status: HP, weight, zeny, points; plans: next job and the
  job level it needs, what it goes to buy; party: following the leader, hunting with, resting, healing), with the
  facts they need in `Known` (and in the state: "HP 30%, weight 55%, 1234 zeny, 2 skill points...", job target,
  party leader). The eval grew to 40 lines (16 for the new templates), 98 sentences for its bot:

  | First pick | Expected right | Top path right | ms per message |
  |---|---|---|---|
  | one flat choice over every sentence | 44% | 34/40 | 1242 |
  | the 7 kinds, then a sentence | 39% | 26/40 | 1028 |
  | the 20 topics, then a sentence | 60% | 33/40 | 841 |
  | the menu (topics, sentences, Back) | | 36/40 | 1420 |

  Started in a wrong topic: Back in 5/6, ended right in 4/6. With 18 topics first (react as one topic of 18
  sentences) the menu ended right in 33/40 ("argh, I died again" got "Welcome back!"); react became cheer, comfort
  and thank. Still wrong: "ok, I have to go now, bye" gets "You too!"; "go there and wait" gets "I'll follow you"
  instead of asking which map; "when can you change job?" gets "Level 9 now!" (my_level, not plans); "what are you
  fighting?" gets "Killed 55 monsters so far". The flat choice keeps up on the top pick (34/40) but spreads its
  probability thin and gets slower with every template (~28 tokens each).

  Next: answer split into accept and decline, my_errands into shopping and errands, and subjects: the lore and
  warnings about one monster or item make a topic of their own ("about_fabre: About Fabre (a monster): where it is,
  how strong it is, what it drops, how dangerous it is") once there are 2 sentences about it (`menu::groups`), so
  lore stays small however much the bot knows. The eval now grades two tiers per line: best, and fine (true and an
  answer to the line, e.g. "I'll follow you" to "go there and wait"); "right" counts both. 21 topics for the eval's
  bot (about_fabre, about_poison_spore among them):

  | First pick | Expected right | Top path right (best) | ms per message |
  |---|---|---|---|
  | one flat choice over every sentence | 47% | 36/40 (33) | 1247 |
  | the 7 kinds, then a sentence | 43% | 28/40 (24) | 962 |
  | the 21 topics, then a sentence | 64% | 35/40 (31) | 918 |
  | the menu (topics, sentences, Back) | | 36/40 (31) | 1492 |

  Started in a wrong topic: Back in 4/6. It never left a subject topic: in about_fabre for "hi Elara!" it said "Watch
  out for Fabre", in about_poison_spore for "where are you hunting?" "Poison Spore is way above me"; a subject holds
  sentences of every kind, so one always seems to fit. Topic keys are read as words: a topic named "busy" took "are
  you busy?" and "your bag looks full" ("Low on potions"); renamed errands. Misses left: "is prt_fild08 a good
  map?" gets the question back ("Is prt_fild08 good for a level 9?", ask_info, even described as "a question of
  your own, not a reply to theirs"), "your bag looks full" gets "Low on potions", "any Poison Spores near you?"
  gets "Not yet", "what are you fighting?" gets "Killed 55 monsters so far".
- The catalogue has 126 templates (2026-10-14; 7 of them lessons): replies (not yet, maybe later, no problem, ...), lore from the reference
  (a monster's level, HP, passive or not, the maps with lots of it, an item's price on a map), warnings (hits hard,
  don't fight alone, careful on a map, died to X on a map), news from the journal, goal and counters (level and job
  level, first visits, buying, selling, resting, exploring, heading for a job, gear bought or worn, kills and deaths so
  far, the party) and requests (party wanted, anyone hunting on a map, is a map good for my level, wait for me).
- `Other...` is off for now (`voice::TYPING`): next to the kinds (and "say nothing"), the model may pick typing its own words, described as very slow
  and a last resort. It then types one character per request (letters, digits, space and a few marks; a character
  that cannot come next is not offered), up to 60 characters. Live, this model cannot spell that way: "A a a a a a b b
  c a a aa..." in 23 s. A typed message is sent only when it reads as words (two thirds of its words known from the
  state, the catalogue or a short common-word list, no word three times in a row), so these were dropped; the forum
  also checks typed comments for names the bot did not read (`check_names`). It was never picked in the chat checks.
  The model does read the unfinished text: given a word under way it mostly types the right next character ("e"
  after "I lik" and "...color is gre", the end after "Blue"), best when the question itself quotes the unfinished
  message (5 of 6 in a probe; the state alone, 4 of 6), which the code now does. What fails is starting a word: the
  first character is near uniform (0.08-0.12, leaning to a, b, c, p), and from a wrong start it never recovers
  ("baacccc...", "a b c de a c a a", "paa p p p...", 9-19 s each).
  Typing word by word (the words of the state and common words) did better in a probe: "blue" to "what's your
  favourite color?", "yes" to "do you like this game?", 1-2 s, but "know know know..." without a no-repeat rule.
- Chat: one choice reads what they ask for (the same requests as the LLM, plus whether they ask if it is a bot). Asked
  whether it is a bot, the bot always says "Yes, I'm a bot" (that question is also caught by words, and the sentence
  is offered nowhere else). The answer to a request the strategist decided on is the fixed words the bot without a
  model says.
- Forum visit (`forum/decided.rs`): answer the replies to the bot (gate: "X wrote to you: ...; write back?"), then
  pick threads to read (or leave), with a yes/no on commenting on each, then maybe start a thread about a finding no
  thread is about (title and body from the finding), then keep lessons. The write rules (`may_comment`, `may_post`)
  are those of the LLM visit. A whole visit takes 2-3 s.
- Lessons: catalogue sentences others wrote parse back (`phrase::parse`) into lessons (`known::lessons_of`), each a Σ
  note and maybe an effect: a warning about a monster leaves it alone for 5 levels (Σ `cautions`, used in target choice
  and hunt options), a good spot within 5 levels is a note, a job change another player made becomes `job_target`
  when none is set and it fits the bot's path. The model keeps those it rates yes (`Event::ForumLessons`). Free text
  (LLM bots, people) gives no lessons.
- Seen on the mock forum with the live model: "No" to "Did you try the fields around Payon instead?", "Spore is too
  strong for a level 12" in a thread on Spores, a thread "Good spot for level 15: prt_fild08", and the lessons "Avoid
  Poison Spore while weak", "Aim for Acolyte", "Try hunting on prt_fild08". In chat: "I'm a level 9 Novice" to
  "what level are you?", follow and hunt requests read right, "Killed 3 Fabre" to "anyone seen Fabres around?".
- Live, 2026-10-14, 15 min, three new characters (Nessa, Orrin, Pippa) with `--voice system-one`, server at 5x: base
  12, 12 and 6; no warning from the voice. They formed a party on the booking board (Nessa leading) and talked in
  party chat: Orrin "Hi Nessa!", Pippa "Thanks!" to the welcome, Nessa "Hello, hunting on prt_fild06". The other two
  read that as asking them to hunt there, said "ok, see you on prt_fild06" and went (an announcement taken as a
  request; harmless in a party). One forum visit (Orrin, on arrival in izlude from the training grounds): read the
  one fresh thread, an LLM bot's, and wrote nothing (0.18).

## Open items

Fixed after the 2026-10-13 run, not yet run live:

- Shade's loop: a skill quest's book entry is made when any of its options is taken (`quest:<id>`), so a stage goal
  no longer reads as done before it starts.
- Shops that would not open: gear, quest and potion buying all skip `bad_shops`, and a shop that will not open once
  the bot stands next to it goes there (deduplicated). Charlie had listed the Tool Dealer on `ein_in01` 19 times and
  still went back. Why the Ammunition dealer on `prontera` (custom `itemmall.txt`) never opens is still to be seen
  live; its list has bullets and kunai (13200+), which the pre-renewal item table may lack.
- Mercy's deaths: worn items the job can no longer wear (`why_not` set, a Novice's Axe on an Acolyte) count for
  nothing in the fight model and in gear values, so a weapon of the job gets bought. A monster that killed the bot
  3 times with fewer than 5 kills per death is avoided for 5 base levels (`Memory::fights`, `losing_to`), in target
  choice and in hunt and quest options.
- Rest sits (the server halves the regeneration interval for a sitting character), stands to heal or fight back,
  fights what targets the bot, and counts 20% of max HP regained as enough. Mages keep casting bolts during a fight
  instead of only opening with one.
- Routes go by map part (`region`), not by map: a warp in a closed-off room is no way out (`ra_in01` jail warp to
  `morocc`, the auction halls), and the floors of a tower are walked through its own warps (`geffen` -> `gef_tower`
  9 -> 10 -> 8 -> 4 -> `gef_dun00` -> `gef_dun01`). `route_into` reaches a room through another door of the town
  outside when the rooms do not connect inside (`prt_in` 18 -> 11 for the Play Dead quest).
- Status points left that the build cannot spend yet are kept for the next level without a failed retry.
- `recruit::takes` uses only levels a stranger gave (booking ad, "party? level 12/9"); the prompt no longer names an
  experience rate.

Still open:

- The forum has not run against a live server yet (only a mock of its HTTP API; the WebSocket is untested).

- Not tested live: JoinParty, thanks after a gift (needs a partner with Basic Skill 1), the loop guard between two
  LLM bots, the next-job gear filter.

- Bad warps are learned per character (could be shared).
- Server: `way_around_warps` (rust-ro `server/src/server/bots/command.rs`) could route long walks over the masked
  cells too; the bot's short legs work around it meanwhile.
- Storage, pets, vending and the WebSocket API are out of scope for now.
