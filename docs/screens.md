# Screens

One SVG per TUI state, rendered from the same frames the snapshot tests compare (`crates/cox-tui/tests/screenshots.rs`; `just screenshots` regenerates them).

![Fresh session with a prompt typed](screenshots/fresh_session.svg)
Fresh session with a prompt typed.

![Streaming reply after a read tool](screenshots/streaming_reply.svg)
Streaming reply after a read tool.

![Finished turn with thinking collapsed](screenshots/finished_turn.svg)
Finished turn with thinking collapsed and a follow-up.

![Bash approval modal](screenshots/approval_modal.svg)
Bash approval modal.

![Slash command palette](screenshots/slash_palette.svg)
Slash command palette.

![File picker on @-mention](screenshots/file_picker.svg)
File picker on @-mention.

![Diff view](screenshots/diff_view.svg)
Diff view against `HEAD`.

![Todo panel](screenshots/todo_panel.svg)
Todo panel after the `todo` tool.

![Running bash tool with background tasks](screenshots/running_tool.svg)
Running bash tool with background tasks.

![Security banner with a warning and an error](screenshots/banner_and_error.svg)
Security banner with a warning and an error.

![Light theme reply](screenshots/light_theme.svg)
Light theme reply.

![Theme picker over the built-ins](screenshots/theme_picker.svg)
Theme picker (`/theme`) over the built-ins.

![Pending tool card](screenshots/tool_card_pending.svg)
Pending tool card with the spinner rail and elapsed time.

![Folded tool card](screenshots/tool_card_folded.svg)
Folded tool card with the head/tail output and the expand hint.

![Error tool card](screenshots/tool_card_error.svg)
Error tool card rendering a failed call whole.

![Side-by-side diff on a wide terminal](screenshots/diff_view_side_by_side.svg)
Side-by-side diff on a wide terminal.

![Question modal](screenshots/question_modal.svg)
`ask_user` question modal over the running turn.

![Queued messages](screenshots/queued_messages.svg)
Queued messages typed while a turn runs.

![Rewind timeline](screenshots/rewind_timeline.svg)
Rewind timeline with checkpointed files per turn.

![Help overlay](screenshots/help_overlay.svg)
`/help`: the keymap table from the one `COMMANDS` source.
