# Simulator Runbook

The simulator runs EVE Killmail Publisher against compiled-in synthetic data. It does not
authenticate with EVE or contact ESI, the EVE image service, the system credential store, or
zKillboard. The normal submission rules still apply: nothing is posted until the user or test
agent performs an explicit individual or confirmed bulk action.

## Run the simulator manually

Start one of the bundled scenarios:

```sh
cargo run --features dev-tools -- --scenario mixed
cargo run --features dev-tools -- --scenario errors
```

The application displays a `SIMULATION` banner when isolation is active.

- `mixed` includes an eligible killmail, protected victims, a reported killmail, and a killmail
  shared by two source characters.
- `errors` includes an eligible killmail whose simulated submission fails.

Each run normally starts from the scenario fixture and keeps changes in memory. To exercise
persistence without touching normal application state, provide a dedicated development state
file:

```sh
cargo run --features dev-tools -- \
  --scenario mixed \
  --dev-state target/ekmp-dev-state.json
```

Use the UI normally: refresh killmails, reveal protected killmails, protect additional victims,
connect the synthetic character supplied by the scenario, and post individual or eligible bulk
killmails. Simulated post results appear in the session-only results panel.

## Create a scenario

1. Copy `dev/scenarios/mixed.json` to a new JSON file in the same directory.
2. Replace its records and outcomes with synthetic data. Never copy real character, corporation,
   killmail, or hash data into a fixture.
3. Register the scenario name in the `load` match in `src/integrations/simulation.rs` using
   `include_str!`.
4. Add the scenario name to the `bundled_scenarios_are_valid` test in that file.
5. Run it with `cargo run --features dev-tools -- --scenario <name>`.
6. Run `cargo test --all-features` to validate all bundled scenarios.

Scenario fields:

| Field | Purpose |
| --- | --- |
| `name` | Human-readable name shown by the simulator. |
| `initial_store` | Starting application state, including connected characters, protection settings, and preferences. |
| `connect_characters` | Synthetic characters returned, in order, when the Connect flow is used. |
| `killmails` | Killmails returned by the simulated ESI integration. |
| `resolved_characters` | Results available to manual protected-character lookup. |
| `resolved_corporations` | Results available to manual protected-corporation lookup. |
| `reported_kills` | Map from source character ID to killmail IDs reported by simulated zKillboard history. |
| `reported_losses` | Map from source character ID to loss IDs reported by simulated zKillboard history. |
| `confirmed_unreported_ids` | Killmail IDs pre-cached as confirmed unreported and therefore potentially eligible for bulk posting. |
| `post_results` | Per-killmail simulated submission outcome. |
| `load_error` | Optional error returned while loading killmails. |
| `status_error` | Optional error returned while checking reported status. |

Each `post_results` value has one of these forms:

```json
{ "result": "new" }
{ "result": "existing" }
{ "result": "error", "message": "simulated service unavailable" }
```

Killmail IDs must be unique. Every ID in `post_results` must also exist in `killmails`. Omitted
collections use empty defaults. Use `dev/scenarios/mixed.json` as the canonical complete example.

## Let an agent use the simulator

There are two useful levels of automated UI testing.

### Repeatable headless tests

Run the existing semantic UI workflows without opening a window:

```sh
cargo test --all-features app::ui::tests:: -- --nocapture
```

Add stable regression workflows beside the existing `egui_kittest` tests in `src/app/ui.rs`.
Prefer this for behavior that should run in CI.

### Interactive agent inspection

Install the egui MCP server once:

```sh
cargo install --locked egui_mcp
codex mcp add egui -- egui-mcp
codex mcp list
```

Restart Codex after adding the server. Then launch an inspectable simulator in a separate terminal:

```sh
EGUI_INSPECTION=1 cargo run --features dev-tools -- --scenario mixed
```

Keep the application window visible when screenshots are needed. The semantic tree and input
operations can generally work while the application is in the background. The inspection server
uses the loopback interface; the application refuses to start with `EGUI_INSPECTION` enabled unless
a simulator scenario is selected.

Give the agent a bounded workflow with observable assertions. For example:

> Attach to the running EVE Killmail Publisher simulator. Verify the SIMULATION banner is present.
> Refresh killmails and confirm the eligible killmail is visible, reported killmails are absent,
> and the session results panel has no successful post. Activate `Post killmail 9001`, wait for the
> operation to finish, verify its session result, and capture a screenshot. Do not edit files.

Useful semantic control labels include:

- `Refresh killmails`
- `Post killmail <ID>`
- `Confirm bulk post`
- `Disconnect <character name>`
- `Protected victim input`
- `Remove protected victim <name>`

Prefer semantic labels over screen coordinates. For destructive-looking workflows, tell the agent
which synthetic scenario and exact killmail IDs it may operate on, and require it to verify the
`SIMULATION` banner before clicking a post control.

## Troubleshooting

- If `--scenario` is rejected, include `--features dev-tools`.
- If a scenario is unknown, register its name in `src/integrations/simulation.rs` and rebuild.
- If inspection mode is rejected, include both `--features dev-tools` and `--scenario <name>`.
- If an agent cannot attach, confirm the application is still running and that the `egui` MCP
  server appears in `codex mcp list`, then restart Codex.
- If screenshots are blank or stale, restore the application window and keep it visible.
- Do not add a database or standalone mock HTTP service for ordinary scenarios. The in-process
  backend is the intended lightweight fixture mechanism; localhost HTTP mocks separately test the
  real adapter request and response formats.
