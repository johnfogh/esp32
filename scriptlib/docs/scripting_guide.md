<style>
table tr:nth-child(even) { background-color: rgba(128,128,128,0.04); }
</style>

# Stages Lua

Lua 5.4 stages drive the controller and linked devices from `target-editor`. The runtime is the `scriptlib` crate (`lib/`).

## Run

Stages live in `stages_folder` from `~/.config/target-editor/config.json` (default `~/dev/esp32/course_defs`). Each file must define `init` and `main`.

Selecting a stage in Stages calls `init`. Start calls `main` with the current Stages cards (non-empty def) injected as the `global_targets` table. Start does not re-run `init`. Stop aborts the run. Locals do not persist from `init` to `main`. `init` must not call `process_targets`.

`os` and `io` are not loaded. Safe libraries (`table`, `string`, `utf8`, `math`, `package`, `coroutine`) are available. `require` loads `.lua` files from the stage directory and `lib/` under it. `require("dkjson")` and `require("socket")` are preloaded (`socket.sleep`, `socket.gettime`).

Controller methods run in-process. Hub I/O for hits goes through `process_targets`, which sends `read_buffer` on each card’s SENSOR HUB and waits up to 30 s for the log result (`read_buffer` is silent in the snapshot and completes from the log). The hub must be linked and enabled (display name, advertised name, address, or MAC).

Time strings for controller `random_delay` are `MM:SS.mmm` (minutes at least two digits, seconds two digits, milliseconds three).

SENSOR HUB on a Stages card is a session UI setting: it is kept while the app runs and is used on that card, and it is not saved into the Lua file or target JSON.

```lua
function init()
  define_grid(2, 3)
  define_distance("7y", 1, 2)
  define_target("Target1", "ipsc")
end

function main()
  global_targets.Target1:active(true)
  start_timer("cof", "00:05.000")
end
```

## Controller methods

These are globals. They do not talk to a board. Empty required strings return an error.

### `define_grid`

| Method | Params | Behavior |
| --- | --- | --- |
| `define_grid` | `rows`, `columns` | In `init` only. Both must be `>= 1`. Callable once. Creates `rows * columns` cards in row-major order. Names are `"col,row"` (x,y), 1-based, e.g. `define_grid(2, 3)` yields `"1,1"`, `"2,1"`, `"3,1"`, `"1,2"`, `"2,2"`, `"3,2"`. Def is empty. Each card stores its `row`/`col`. Appends each name to `global_targets.index`. |

```lua
function init()
  define_grid(2, 3)
end
```

### `define_distance`

| Method | Params | Behavior |
| --- | --- | --- |
| `define_distance` | `distance`, `row`, `col` | In `init` only. Stores the `distance` string on the card at that `row`/`col` and removes that cell name from `global_targets.index` (layout-only; not a target). Errors if no card occupies that cell. |

```lua
function init()
  define_grid(2, 3)
  define_distance("7y", 1, 2)
end
```

### `define_target`

| Method | Params | Behavior |
| --- | --- | --- |
| `define_target` | `name`, `def`, `row?`, `col?` | In `init` only. Adds a named Stages card from `<targets_folder>/<def>.json`. `name` and `def` must be non-empty. Optional `row`/`col` must both be set or both omitted; they are position metadata only (do not rename grid cells). Appends `name` to `global_targets.index` and sets `global_targets[name] = { target_name, target_def }`. |

```lua
function init()
  define_target("Target1", "ipsc")
  define_target("Target2", "ipsc")
  define_target("Left", "ipsc", 2, 1)
end
```

### `global_targets`

During `init`, a table with `index` (array of card names filled by `define_grid` / `define_target`). `define_distance` removes that cell from `index` so distance cells stay layout-only. Each `define_target` also sets `global_targets[name] = { target_name, target_def }` so those fields are readable in `init`. At Start, that table is replaced with userdata for cards that have a non-empty def (real targets from `define_target`; empty grid cells and distance cells are omitted), plus `index` rebuilt from those cards.

<table>
<tr><th>Access</th><th>Behavior</th></tr>
<tr style="background-color: rgba(128,128,128,0.04);"><td><code>global_targets.index</code></td><td>1-based array of card names. Filled by <code>define_grid</code> / <code>define_target</code> in <code>init</code>; at Start rebuilt from non-empty-def cards only.</td></tr>
<tr><td><code>global_targets[i]</code></td><td>Start only. 1-based order of non-empty-def cards (userdata).</td></tr>
<tr style="background-color: rgba(128,128,128,0.04);"><td><code>global_targets.Name</code> / <code>global_targets["Name"]</code></td><td>In <code>init</code>: stub table <code>{ target_name, target_def }</code> from <code>define_target</code>. At Start: card userdata named <code>Name</code>. Duplicate names: last card wins the named key; both stay in the array.</td></tr>
</table>

<table>
<tr><th>Field / method</th><th>Behavior</th></tr>
<tr style="background-color: rgba(128,128,128,0.04);"><td><code>target_name</code></td><td>Card name (string).</td></tr>
<tr><td><code>target_def</code></td><td>Target-def stem (string; empty if none).</td></tr>
<tr style="background-color: rgba(128,128,128,0.04);"><td><code>zones</code></td><td>Dictionary of zone userdata keyed by name from that card’s current def. Duplicate names: last in def order wins.</td></tr>
<tr><td><code>:active()</code></td><td><code>true</code> if any zone on the card is active. All zones start <code>false</code> at Start.</td></tr>
<tr style="background-color: rgba(128,128,128,0.04);"><td><code>:active(on)</code></td><td>Sets every zone on the card to <code>on</code>. Errors if the card is unknown or has no zones. Returns <code>on</code>.</td></tr>
</table>

Each zone userdata:

<table>
<tr><th>Field / method</th><th>Behavior</th></tr>
<tr style="background-color: rgba(128,128,128,0.04);"><td><code>name</code></td><td>Zone name (string).</td></tr>
<tr><td><code>:active()</code></td><td>This zone’s flag.</td></tr>
<tr style="background-color: rgba(128,128,128,0.04);"><td><code>:active(on)</code></td><td>Sets this zone’s flag. Errors if the zone is not on the card. Returns <code>on</code>.</td></tr>
</table>

```lua
global_targets.Target1:active(true)
global_targets.Target1.zones["A Zone Head"]:active(true)
global_targets[1]:active(false)
text(global_targets.Target1:active())
text(global_targets.Target1.target_name)
text(global_targets.Target1.target_def)
```

### `process_targets`

| Method | Params | Behavior |
| --- | --- | --- |
| `process_targets` | none | Start only. For each Start card in order: reads that card’s SENSOR HUB (`{"action":"read_buffer","params":[]}`), diffs new samples against internal per-card event state, resolves each new sample against active zones (smallest-area visible active zone containing the recovered cm hit; inactive / unlocatable / no-zone → `"Miss"`), and sends one Events-log line per sample: `{"type":"target_hit","ms":…,"target":"<name>","zone":"<zone>\|"Miss","sensors":[TL,TR,BL,BR]}`. Inactive cards are still polled. Empty new buffers are skipped. Errors if a card has no SENSOR HUB or the device is not linked/enabled. Returns nothing. Stages Events renders these as unlabeled three-column rows (`ms` last 5 digits, target, zone); consecutive same-second hits share one card. |

```lua
global_targets.Target1:active(true)
process_targets()
```

### `start_timer`

| Method | Params | Behavior |
| --- | --- | --- |
| `start_timer` | `name`, `duration` | Stores a named deadline at now + `duration` (`MM:SS.mmm`). Restarts overwrite. `name` must be non-empty. Returns `name`. |

```lua
start_timer("window", "00:05.000")
```

### `timer_expired`

| Method | Params | Behavior |
| --- | --- | --- |
| `timer_expired` | `name` | Returns `true` when now is at or past the deadline from `start_timer`. Errors if the timer was never started. |

```lua
if timer_expired("window") then
  text("done")
end
```

### `random_delay`

| Method | Params | Behavior |
| --- | --- | --- |
| `random_delay` | `min`, `max` | Sleeps a random duration in `[min, max)`. Both are `MM:SS.mmm`. `min` must be strictly smaller than `max`. Returns the slept time as `MM:SS.mmm`. |

```lua
local waited = random_delay("00:00.500", "00:01.000")
text(waited)
```

### `text`

| Method | Params | Behavior |
| --- | --- | --- |
| `text` | `txt` | Start only. Sends `{"text":"<txt>"}` on the log channel. Non-string values are converted to text. |

```lua
text("hit A")
text(elapsed)
```

## Returns and errors

Controller methods return a string or raise a Lua error (`mlua` external error).

`process_targets` raises if:

- a card has no SENSOR HUB assigned
- device link is not running
- the matching device is connected but not enabled
- no matching device is linked within 30 s
- the wait for a buffer result exceeds 30 s
- the buffer sample has `error: true` or a result starting with `error:`

## Example

```lua
function init()
  define_target("Target1", "ipsc")
end

function main()
  start_timer("run", "00:30.000")
  global_targets.Target1.zones["A Zone Head"]:active(true)
  random_delay("00:00.500", "00:01.000")
  process_targets()
  if timer_expired("run") then
    text("done")
  end
end
```
