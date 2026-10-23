# Handoff: cox desktop work (for the next agent, Cursor)

State as of 2026-09-29. Read `AGENTS.md`, then `plan.md` (§0 decisions, the task table, A127–A130). This file only says where things stand and how the previous agent worked; `plan.md` stays the source of truth.

## Where the work lives

- Feature branch `p37-desktop`, PR https://github.com/pyrlyn/cox/pull/65. Every task branch merges into `p37-desktop`; the creator merges the PR. Do not merge the PR, enable auto-merge, publish, or tag.
- Worktrees: one per task via the `worktrees` skill (`wt.sh new <task-id>`, `wt.sh done <path>`, `wt.sh clean <path>`) under `_worktrees/cox-<task-id>`. Never `rm -rf` a worktree; never touch a worktree locked by another owner (for example `rtok-*`).
- Git in these worktrees hangs on fsmonitor. Prefix every git command with `export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.fsmonitor GIT_CONFIG_VALUE_0=false;`.
- Disk is tight (a worktree's `target/` is 15–40 GB). Run `wt.sh clean` on a worktree as soon as you stop building in it; never share one cargo target dir between worktrees.

## How a task is claimed and closed

1. Claim: in `p37-desktop`, set the table row to `in progress` with your provider and model, write the execution plan into the card, sync `todo.md`, commit `<id>: claim`, push. Then `wt.sh new <id>` and `git merge --ff-only p37-desktop` in the new worktree.
2. Work in the worktree, one commit per card: `<id>: <title>`. No `Co-Authored-By`, no "Generated with" line — the creator is the only author.
3. Verify only what the change can break (other agents share the machine):
   - `CARGO_BUILD_JOBS=4 mise exec -- cargo nextest run -p <crates>` plus the card's Check;
   - `mise exec -- cargo clippy -p <crates> --all-targets -- -D warnings`, `mise exec -- cargo fmt --check`;
   - `swift test` in each touched package under `desktop/macos/Packages/`;
   - `just desktop-app` when Swift or `cox-ffi` changed.
4. Close: merge the branch into `p37-desktop`, re-run the checks there, move the card entirely from `plan.md` (row and card) and `todo.md` into `done.md` with Result / Deviations / Check / Not done, commit `<ids>: close`, push, add one line to the PR body, then `wt.sh done` and delete the branch.

## Open work

### Left mid-flight by the previous agent (all released to `todo`, free to claim)

Each branch below is pushed to `origin`; its worktree was removed. Make a fresh worktree (`wt.sh new <id>`), then `git merge origin/<branch>` into it (or merge the branch into `p37-desktop` directly when it only needs verifying).

| Cards | Branch | State | What is left |
| --- | --- | --- | --- |
| T58.4.20–25 | already merged into `p37-desktop` | code done; the agent's own checks passed on its branch (`nextest -p cox-app -p cox-ffi` 208/208, CoxModel 128, CoxCore 20, CoxTranscriptText 39, CoxTranscript 55, `just desktop-app` ok) | run the post-merge checks on `p37-desktop` (nextest `-p cox-app -p cox-ffi`, clippy, `swift test` in CoxModel, CoxCore, CoxTranscriptText, CoxTranscript, `just desktop-app`), then close all six using the reports at the end of this file |
| T58.4.4–6, T58.4.14–15 | `t58.4.4` | one commit per card, all five committed (last `fac0c9de` T58.4.5); the agent was stopped before its final check run | merge into `p37-desktop`, run the cards' Checks and the checks above, close |
| T58.4.26 | `t58.4.26` | committed (`5e27b973`), not verified | verify T58.4.26's Check; then do T58.4.27 and T58.4.28 (Swift uses the core's Markdown writer) |
| T52.23.1 | `t52.23.1` | one WIP commit, stopped mid-edit (`cox-app` changes, `patch.rs`, `plugin_ui.rs`, `timeline.rs`, `cox-ffi/src/types.rs`); unbuilt | finish the card from its plan.md text, then T52.23.2 |

Known flaky under load (pass alone): `terminal_close_kills_the_process_group`, CoxTranscript `PinnedDecisionTests`.

A warm build tree for verification is `_worktrees/cox-p51` (branch `p51-roadmap`, locked by the previous agent): the creator may hand it over; otherwise build in your own worktree.

### Waiting on the creator

- T51.23 is merged and verified (CoxUI 238/238) and sits at 95%. One question: on glass, the pane tint is fixed at `glass.fill`'s alpha and the transparency slider moves only the window tint. Should the slider scale the panes again? Close the card or adjust it after the answer.
- T37.29.3.4 (budget cap source), T37.32.2 (Developer ID, notarization, Sparkle — needs the creator's certificates), T53.5 (ABI freeze confirmation), T53.9 (the creator publishes the SDKs), T56.x (Cursor Cloud terms go-ahead, A123), T35.10 and T39.7 (need the creator's own API keys).

### Waiting on an upstream release

- T33.43: an extism release after v1.30.0 that pins wasmtime ≥ 48. It unblocks T33.14 → T33.18, T33.34, T33.36, T33.40.x and then T53.x. The bump itself needs the creator's permission.

### Windows (P57/P58 except T58.4)

Built and tested only by an agent on a Windows host (A130). The CI `windows` job stays `if: false` until that agent has verified the work; never enable it from macOS. T57.1/T57.7 carry "Remaining (A130)" lines for that host.

### Skipped by rule

Comparisons with other agents (e.g. T30.13). Measurement cards were allowed again on 2026-10-08 (T43.6, T61.1); T37.33 is done.

## Rules the previous agent followed

- No version bumps, new dependency only if maintained (reason in the commit, row in `toolchain.md` and `plan.md` §1).
- Tests never touch the real keychain; dev runs of the binary use a scratch `COX_HOME`.
- UI: tokens and components from CoxUI; a visual change re-records the affected snapshots and, for a mockup, re-renders it (`desktop/design/mockups`). Figma writes go only to file `KA9a0R7n6P0QbwDn92e167`.
- Never kill the app by name (`pkill -f Cox` hits other sessions); quit it by PID.
- Anything that needs the creator's decision: stop and ask, do not guess.

## Close reports for T58.4.20–T58.4.24 (merged, post-merge check pending)

### T58.4.20

Result: `cox-app/src/info.rs` `Fact { label, value: Option<String>, detail }`; `Info.facts` (Session, Folder, Worktree, Branch detail — `detached` without a branch —, Rollout; home as `~`) and `config_facts` (`N key(s)` per layer with its file as a detail row); `build` takes `home` (`live.rs` passes `cox_config::load::home_dir()`). `changes.rs`: `worktree_facts` (Branch; Base only with both base and commit) and `turns: Vec<TurnFiles>` (a file in the turn that changed it last, oldest first). `cox-ffi` declares the records. Commit 9de35b37.

Deviations: the worktree size stays in each client (locale byte formatting); `lib.rs`, `live.rs` touched for the call change.

Check (2026-09-29): `nextest -p cox-app info changes` 10 passed incl. `the_home_directory_reads_as_tilde`, `base_needs_both_base_and_commit`, `a_file_sits_in_the_turn_that_changed_it_last`, `a_layer_lists_its_key_count_and_its_file_under_it`; `-p cox-app -p cox-ffi` 207/207; clippy, fmt clean. After the merge into p37-desktop: see T58.4.25's Check.

Not done: nothing.

### T58.4.21

Result: `CoxClient.Fact`, `Info.facts`/`configFacts` converted in `InfoConvert.swift`; `InfoTabState` copies the facts; its `home:` parameter and the Swift `~`/key-count logic removed. Commit dedb8c4d.

Deviations: ConvertTests' Changes record got `worktreeFacts: [], turns: []` here so the commit compiles.

Check (2026-09-29): CoxModel `InfoTabTests` 2 passed; CoxCore ConvertTests 10 passed incl. `anInfoRecordCarriesItsFacts`. After the merge into p37-desktop: see T58.4.25's Check.

Not done: nothing.

### T58.4.22

Result: `Changes.worktreeFacts`, `turns` (`TurnFiles`) converted in `ChangesConvert.swift`; `ChangesTabState` lists the core's facts and appends the localized Size itself. Commit 45b5176c.

Deviations: none.

Check (2026-09-29): CoxModel `ChangesTabTests|InfoTabTests` 5 passed; CoxCore ConvertTests passed. After the merge into p37-desktop: see T58.4.25's Check.

Not done: nothing.

### T58.4.23

Result: `patch::TaskState { Running, Succeeded, Failed }` with `TaskState::ended` (no exit code or 0 is success); `BlockKind::Task.state` set by the timeline on create and completion; re-exported and declared in `cox-ffi`; `done` and `exit_code` kept. Commit 9d091bbf.

Deviations: `crates/cox-app/tests/snapshots/scenarios__subagent_explore.snap` re-recorded (`state` on its two task lines).

Check (2026-09-29): `a_subagent_without_an_exit_code_succeeded` passes; `nextest -p cox-app -p cox-ffi` 208/208 (`terminal_close_kills_the_process_group` failed once under load, passes alone and on re-run); clippy, fmt clean. After the merge into p37-desktop: see T58.4.25's Check.

Not done: nothing.

### T58.4.24

Result: `BlockKind.task` carries `state: TaskState`; fixture decoding reads `"state"`; the conversion is in `TaskConvert.swift`. Commits 956594ab, 68823482.

Deviations: `Convert.swift` would pass SwiftLint's 400 lines, so the conversion went to `TaskConvert.swift`; positional `.task(...)` patterns in `TaskRows.swift`, `TranscriptCard.swift`, `MarkdownCopy.swift` and two tests updated. No fixture JSON re-recorded — none of the four fixture scenarios has a task block; inline tests `aTaskBlockDecodesTheCoresState` and `aTaskStateConvertsCaseForCase` added instead.

Check (2026-09-29): CoxModel SessionStore/TaskRows/decoding 11 passed; CoxCore ConvertTests 11 passed. After the merge into p37-desktop: see T58.4.25's Check.

Not done: fixture re-record (no task block in any fixture).


### T58.4.25

Result: `SessionStore.tasks` copies the block state (`TaskRow.State` is a typealias of `TaskState`); `TranscriptCard` maps it to the header state; `ReviewState` shows `changes.turns` (the Swift grouping removed); a shared `ChangesTabState.File.init(_ ChangedFile)` builds rows for the Changes tab and Review. Commit 1ff30259.

Deviations: `ChangesTabState.swift` touched for that shared helper.

Check (2026-09-29): CoxModel `TaskRowsTests|ReviewStateTests` 4 passed; CoxTranscript `TranscriptSnapshotTests|SessionReviewStateTests` 3 passed; SwiftLint/swift-format clean on changed files except two findings already on HEAD (`TranscriptCard.swift` long `isWholeSecond` line, `DurationFormattingTests.swift:21` identifier_name).

Not done: nothing.
