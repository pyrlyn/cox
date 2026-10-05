// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Composer` (DS§6.4 row `Composer`, the mockup's `.composer`; DT§5.3; mockup screens 1, 5–7):
// where the next message is written — the text, the files it mentions and attaches, shell mode with its
// "share output" switch, the prompts queued behind the running turn, the completion rows for
// the `@` or `/` token at the caret, why the last send failed, the permission mode, the model
// with its effort and the think toggle, and Send. Separate so the transcript column shows it from one value and
// reports every key and click as an intent; the store behind it decides what each one sends.

import AppKit
import SwiftUI

/// The editor over a row of chips and Send, on readable window glass at e3 — the one thing that
/// floats highest in a pane (DS§3.4). The completion rows float above it; why the last send
/// failed stands above it as a `NoticeRow`. ⏎ sends (or picks the selected row while rows
/// show), ⇧⏎ breaks the line, ⌘⏎ sends now, ↑ ↓ ⇥ and ⎋ drive the rows, ↑ in an empty composer
/// walks the earlier prompts, ⌫ in an empty shell line leaves shell mode, ⇧⇥ asks for the next
/// permission mode, and ⌘V of files or an image attaches them. The draft is the caller's: the
/// editor's copy reports every change.
public struct Composer: View {
  public struct State: Equatable, Sendable {
    public var text = ""
    /// The selection in `text`, in UTF-16 offsets; empty is the caret, `nil` the caret at the end.
    public var selectedRange: Range<Int>?
    /// A leading `!` turned the line into a shell command (DT§5.3).
    public var isShell = false
    /// The shell command's output goes to the agent too (`UserShell{share}`).
    public var shareOutput = true
    /// Files picked from the `@` rows.
    public var mentions: [Mention] = []
    /// Files and images pasted, dropped or picked, sent with the message.
    public var attachments: [Attachment] = []
    /// Rows for the token being typed, or `nil`.
    public var completion: CompletionList.State?
    /// A turn runs, so ⏎ queues the message.
    public var isRunning = false
    /// Prompts queued behind the running turn.
    public var queued = 0
    /// Something to send: text or an attachment.
    public var canSend = false
    /// The text is an earlier prompt ↑ brought back, so ↑ and ↓ keep walking the prompts.
    public var isRecalling = false
    /// The token meter beside Send (T37.25); `nil` before the core's first usage.
    public var meter: TokenMeter.State?
    /// The token popover over the meter while it is open.
    public var tokens: TokenPopover.State?
    /// Why the last send failed; the draft is still there to send again.
    public var failure: String?
    /// The permission mode in force; `nil` hides its chip until the core reports it.
    public var mode: SessionMode?
    /// The model and its effort as the core names them, `claude-sonnet-5 · high`; `nil` hides it.
    public var model: String?
    /// The next turn goes to the think tier (A103); its chip stands beside the model's.
    public var think = false

    public init() {}
  }

  /// A mentioned file: its `@path`, as the core's completion inserted it, and its label.
  public struct Mention: Equatable, Sendable, Identifiable {
    public var id: String
    public var label: String

    public init(id: String, label: String) {
      self.id = id
      self.label = label
    }
  }

  /// An attached file; an image carries its bytes for the thumbnail.
  public struct Attachment: Equatable, Sendable, Identifiable {
    public var id: String
    public var name: String
    public var image: Data?

    public init(id: String, name: String, image: Data? = nil) {
      self.id = id
      self.name = name
      self.image = image
    }
  }

  public enum Intent: Equatable, Sendable {
    /// The text as typed.
    case edit(String)
    /// The selection moved, in UTF-16 offsets into the text; the token at the caret is completed.
    case select(Range<Int>)
    /// ⏎ or Send: sends, or queues while a turn runs.
    case submit
    /// ⌘⏎: interrupts the running turn and sends.
    case submitNow
    /// ↑ (-1) or ↓ (+1) through the completion rows.
    case moveSelection(Int)
    /// ↑ (-1) to an earlier prompt or ↓ (+1) back, from an empty composer.
    case recall(Int)
    /// A completion row, by index.
    case pick(Int)
    /// ⎋ while the rows show.
    case dismissCompletion
    case removeMention(String)
    case leaveShell
    case shareOutput(Bool)
    /// The paperclip: pick files to attach.
    case attach
    /// Files dropped on the composer, or pasted into it with ⌘V.
    case drop([URL])
    /// An image pasted with ⌘V that has no file behind it, as PNG.
    case pasteImage(Data)
    case removeAttachment(String)
    /// The token meter: opens or closes its popover.
    case toggleTokens
    /// ⇧⇥ or the mode chip: the next permission mode, which the core picks.
    case cycleMode
    /// The think chip: the think tier for the next turn, or not.
    case toggleThink
  }

  let state: State
  let send: (Intent) -> Void

  public init(state: State, send: @escaping (Intent) -> Void) {
    self.state = state
    self.send = send
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.s) {
      if let failure = state.failure { ComposerFailure(text: failure) }
      pane
    }
    .frame(maxWidth: Size.readingWidth)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Composer")
  }

  private var pane: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.pane, style: .continuous)
    return VStack(alignment: .leading, spacing: 0) {
      ComposerEditor(state: state, send: send)
        .padding(EdgeInsets(top: Space.l, leading: Space.l, bottom: Space.s, trailing: Space.l))
        // The mockup's `.ta` min-height counts its insets, so it goes on after them.
        .frame(minHeight: Size.composerTextMinHeight, alignment: .topLeading)
      if !state.attachments.isEmpty {
        ComposerAttachments(attachments: state.attachments) { send(.removeAttachment($0)) }
          .padding(.horizontal, Space.xl)
          .padding(.vertical, Space.xs)
      }
      ComposerChipRow(state: state, send: send)
        .padding(.horizontal, Space.ml)
        .padding(.top, Space.s)
        .padding(.bottom, Space.ml)
    }
    .frame(maxWidth: Size.readingWidth)
    .glassPane(shape, surface: Color(.surfaceWindow), role: .readable)
    .hairline(in: shape)
    .elevation(.e3, cornerRadius: Radius.pane)
    .overlay(alignment: .topLeading) {
      // A line along the composer's top edge that the rows stand on, so they grow upwards.
      Color.clear.frame(height: 0).overlay(alignment: .bottomLeading) {
        if let completion = state.completion {
          CompletionList(state: completion) { send(.pick($0)) }
            .fixedSize()
            .padding(.leading, Space.l)
            .padding(.bottom, Space.m)
        }
      }
    }
    .overlay(alignment: .topTrailing) {
      // The token popover stands on the same edge, over the meter at the trailing end.
      Color.clear.frame(height: 0).overlay(alignment: .bottomTrailing) {
        if let tokens = state.tokens {
          TokenPopover(state: tokens).fixedSize().padding(.bottom, Space.m)
        }
      }
    }
    .dropDestination(for: URL.self) { urls, _ in
      send(.drop(urls))
      return !urls.isEmpty
    }
  }
}

/// Why the last send failed, on the composer's readable glass so it reads over any backdrop.
private struct ComposerFailure: View {
  let text: String

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.xxl, style: .continuous)
    NoticeRow(text, kind: .error)
      .padding(.horizontal, Space.l)
      .padding(.vertical, Space.s)
      .glassPane(shape, surface: Color(.surfaceWindow), role: .readable)
      .hairline(in: shape)
      .elevation(.e3, cornerRadius: Radius.xxl)
  }
}

/// The text, in `font.transcript` — or `font.mono.code` for a shell line — growing with what is
/// typed up to `maxHeight`, then scrolling; the hint shows while it is empty.
private struct ComposerEditor: View {
  let state: Composer.State
  let send: (Composer.Intent) -> Void
  @Environment(\.composerPasteboard) private var pasteboard
  @FocusState private var isFocused: Bool
  /// The editor's own copy of the text and selection, kept in step with `state`.
  @State private var draft: ComposerDraft

  init(state: Composer.State, send: @escaping (Composer.Intent) -> Void) {
    self.state = state
    self.send = send
    _draft = State(initialValue: ComposerDraft(state))
  }

  /// About ten lines of `font.transcript`; a longer message scrolls inside the editor.
  private static let maxHeight: CGFloat = 220

  var body: some View {
    let font: FontToken = state.isShell ? .monoCode : .transcript
    ZStack(alignment: .topLeading) {
      // Sizes the editor to its text: a `TextEditor` takes all the height it is offered.
      Text(state.text + " ")
        .textStyle(font)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.horizontal, Space.xs)
        .hidden()
        .accessibilityHidden(true)
      if state.text.isEmpty {
        Text(state.isShell ? "Shell command" : "Ask cox…  @ files  / commands  ! shell")
          .textStyle(font)
          .foregroundStyle(Color(.textPlaceholder))
          .padding(.horizontal, Space.xs)
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
      TextEditor(text: $draft.text, selection: $draft.selection)
        .textStyle(font)
        .foregroundStyle(Color(.textPrimary))
        .scrollContentBackground(.hidden)
        .accessibilityLabel(state.isShell ? "Shell command" : "Message")
        .onKeyPress(action: key)
        .focused($isFocused)
        .background(
          WindowKeys(
            handle: ComposerPaste.keys(isActive: isFocused, pasteboard: pasteboard, send: send))
        )
        .onChange(of: draft) { _, new in new.intents(after: state).forEach(send) }
        // A typed `!` becomes shell mode with no text, so the text alone may not change.
        .onChange(of: state.text) { draft = ComposerDraft(state) }
        .onChange(of: state.isShell) { draft = ComposerDraft(state) }
        .onChange(of: state.selectedRange) { draft = ComposerDraft(state) }
    }
    .frame(maxHeight: Self.maxHeight)
  }

  /// The keys the composer answers before the text does; any other key types.
  private func key(_ press: KeyPress) -> KeyPress.Result {
    let rows = state.completion != nil
    switch press.key {
    case .return where press.modifiers.contains(.shift):
      return .ignored
    // AppKit spells ⇧⇥ as the back-tab character, and ⇥ with ⇧ held.
    case KeyEquivalent("\u{19}"),
      .tab where press.modifiers.contains(.shift):
      send(.cycleMode)
    case .return where press.modifiers.contains(.command):
      send(.submitNow)
    case .return:
      send(rows ? .pick(state.completion?.selection ?? 0) : .submit)
    case .tab where rows:
      send(.pick(state.completion?.selection ?? 0))
    case .upArrow, .downArrow:
      return arrow(press.key == .upArrow ? -1 : 1)
    case .escape where rows:
      send(.dismissCompletion)
    case .delete where state.isShell && state.text.isEmpty:
      send(.leaveShell)
    default:
      return .ignored
    }
    return .handled
  }

  /// ↑ (-1) or ↓ (+1): through the rows while they show, else through the earlier prompts — a
  /// walk ↑ starts in an empty line; otherwise the cursor moves.
  private func arrow(_ step: Int) -> KeyPress.Result {
    if state.completion != nil {
      send(.moveSelection(step))
    } else if state.isRecalling || (step < 0 && state.text.isEmpty && !state.isShell) {
      send(.recall(step))
    } else {
      return .ignored
    }
    return .handled
  }
}

extension EnvironmentValues {
  /// Where the composer's ⌘V reads files and images from; a test sets a private one.
  @Entry public var composerPasteboard: NSPasteboard = .general
}

/// ⌘V while the editor has focus: file URLs or an image on the pasteboard are attached (as a
/// drop is), anything else is left for the text view to paste. Through `WindowKeys`, because
/// the Edit menu's Paste claims ⌘V before a view's key handler sees it.
private enum ComposerPaste {
  /// The window's key handler while the editor has focus; `nil` while it has not.
  static func keys(
    isActive: Bool, pasteboard: NSPasteboard, send: @escaping (Composer.Intent) -> Void
  ) -> ((NSEvent) -> Bool)? {
    guard isActive else { return nil }
    return { event in isPaste(event) && attachable(on: pasteboard).map(send) != nil }
  }

  /// Files first — a file copied in Finder also carries its icon and name — then an image,
  /// unless the pasteboard has text too (a copied spreadsheet range carries a picture of itself).
  static func attachable(on pasteboard: NSPasteboard) -> Composer.Intent? {
    let options: [NSPasteboard.ReadingOptionKey: Any] = [.urlReadingFileURLsOnly: true]
    let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: options) as? [URL] ?? []
    if !urls.isEmpty { return .drop(urls) }
    guard pasteboard.string(forType: .string) == nil else { return nil }
    if let png = pasteboard.data(forType: .png) { return .pasteImage(png) }
    return pasteboard.data(forType: .tiff).flatMap(NSBitmapImageRep.init(data:))
      .flatMap { $0.representation(using: .png, properties: [:]) }.map(Composer.Intent.pasteImage)
  }

  /// ⌘V alone. `characters`, not `charactersIgnoringModifiers`: with ⌘ held a layout gives its
  /// command-key letters (Latin on a Cyrillic layout), which is what the menu matches too.
  private static func isPaste(_ event: NSEvent) -> Bool {
    WindowKeys.holds(event, only: .command) && event.characters == "v"
  }
}

/// The paperclip, the mode, the model and think, shell mode and its switch, mentions, the queue, Send.
private struct ComposerChipRow: View {
  let state: Composer.State
  let send: (Composer.Intent) -> Void

  var body: some View {
    HStack(spacing: Space.s) {
      Button {
        send(.attach)
      } label: {
        ComposerChip("", kind: .attachment)
      }
      .buttonStyle(.plain)
      .help("Attach files")
      .accessibilityLabel("Attach files")
      if let mode = state.mode {
        Button {
          send(.cycleMode)
        } label: {
          ComposerChip(mode.title, kind: .mode(mode), shortcut: "⇧⇥")
        }
        .buttonStyle(.plain)
        .help("Next permission mode (⇧⇥)")
      }
      if let model = state.model {
        ComposerChip(model, kind: .model)
        ThinkChip(isOn: state.think) { send(.toggleThink) }
      }
      if state.isShell {
        ComposerChip("Shell", kind: .shell) { send(.leaveShell) }
        Toggle(
          "Share output", isOn: Binding(get: { state.shareOutput }, set: { send(.shareOutput($0)) })
        )
        .toggleStyle(CoxToggleStyle())
      }
      ForEach(state.mentions) { mention in
        ComposerChip(mention.label, kind: .mention) { send(.removeMention(mention.id)) }
      }
      if state.queued > 0 {
        ComposerChip("Queued · \(state.queued)", kind: .queued)
      }
      Spacer(minLength: Space.m)
      if let meter = state.meter {
        // Its figures never wrap; the chips before it truncate instead.
        TokenMeter(state: meter, isOpen: state.tokens != nil) { send(.toggleTokens) }.fixedSize()
      }
      Button {
        send(.submit)
      } label: {
        Image(systemName: "arrow.up").symbolStyle(.body)
      }
      .buttonStyle(SendButtonStyle())
      .disabled(!state.canSend)
      .help(state.isRunning ? "Queue after this turn (⏎) · send now (⌘⏎)" : "Send (⏎)")
      .accessibilityLabel(state.isRunning ? "Queue" : "Send")
    }
  }
}

#Preview("empty") { PreviewMatrix { ComposerSample(state: PreviewState.composerEmpty) } }
#Preview("mention") { PreviewMatrix { ComposerSample(state: PreviewState.composerMention) } }
#Preview("commands") { PreviewMatrix { ComposerSample(state: PreviewState.composerCommands) } }
#Preview("attachments") {
  PreviewMatrix { ComposerSample(state: PreviewState.composerAttachments) }
}
#Preview("shell, queued") { PreviewMatrix { ComposerSample(state: PreviewState.composerShell) } }
#Preview("tokens") { PreviewMatrix { ComposerSample(state: PreviewState.composerTokens) } }
#Preview("failure") { PreviewMatrix { ComposerSample(state: PreviewState.composerFailure) } }
#Preview("status") { PreviewMatrix { ComposerSample(state: PreviewState.composerStatus) } }
#Preview("think") { PreviewMatrix { ComposerSample(state: PreviewState.composerThink) } }
