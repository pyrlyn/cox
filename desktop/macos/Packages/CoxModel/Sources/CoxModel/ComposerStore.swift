// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The composer of one open session as observable state (DT§5.3, DT§4.6): the draft, shell
// mode, the files picked from `@` rows, the files attached, the rows the core offers for the
// token at the caret, and the earlier prompt ↑ brought back; beside it, the session's mode and
// model as the core reports them, ⇧⇥'s ask for the next mode, and the think toggle (A103).
// Separate from `SessionStore`, which holds what the core sent back; this holds what the
// person is about to send. It asks the core for rows (`cox_app::Completer`), for the token that
// asks for them, how a pick lands and what the draft becomes (T58.4.19), and sends one `Intent`;
// the command table, the ranking, the files and those rules all stay in Rust.

import CoxClient
import Foundation
import Observation
import UniformTypeIdentifiers

@Observable
@MainActor
public final class ComposerStore {
  public private(set) var text = ""
  /// A leading `!` turned the draft into a shell command.
  public private(set) var isShell = false
  /// The shell command's output goes to the agent too (`UserShell{share}`).
  public var shareOutput = true
  /// The `@path` inserts picked from the rows, in the order picked.
  public private(set) var mentions: [String] = []
  /// Files read from what was dropped, pasted or picked; the core decides what reaches the
  /// model (T37.6).
  public private(set) var attachments: [Attachment] = []
  /// Rows for the token being typed; empty when none is offered.
  public private(set) var completions: [Completion] = []
  public private(set) var selection = 0
  /// The editor's selection in `text`, in UTF-16 offsets; empty is the caret, `nil` is the caret
  /// at the end — so text typed at the end needs no report of the caret moving with it.
  public private(set) var selectedRange: Range<Int>?
  /// Why the last send failed; the draft stays so it can be sent again.
  public private(set) var failure: String?
  /// The next turn goes to the think tier, as `/think` sends it (A103); off again once the core
  /// took that turn, so the costly tier never stays on by accident.
  public private(set) var think = false
  @ObservationIgnored public let session: SessionStore
  /// The session's earlier prompts, newest first, and which one the draft shows, while ↑ ↓ walk
  /// them; `nil` once the draft is typed, sent or walked back to empty.
  private var recalled: (prompts: [String], index: Int)?

  /// Rows asked for at a time: more than the list shows scrolls nothing into view.
  static let rowLimit: UInt32 = 8
  /// Earlier prompts asked for when ↑ starts a walk.
  static let historyLimit: UInt32 = 100

  public init(session: SessionStore) {
    self.session = session
  }

  /// Something to send.
  public var canSend: Bool { draft(.queue).canSend }

  /// The draft as typed. A `!` typed into an empty draft enters shell mode instead.
  public func edit(_ new: String) {
    let asked = client.draftIntent(
      new, shell: isShell, attachments: 0, running: false, when: .queue)
    if text.isEmpty, asked.entersShell {
      isShell = true
      text = ""
      (completions, selectedRange) = ([], nil)
      return
    }
    if new != text { recalled = nil }
    text = new
    if let range = selectedRange, range.upperBound > text.utf16.count { selectedRange = nil }
    mentions = client.mentions(text, picked: mentions)
    complete()
  }

  public func moveSelection(by step: Int) {
    guard !completions.isEmpty else { return }
    selection = (selection + step + completions.count) % completions.count
  }

  /// The editor's selection moved, in UTF-16 offsets into `text`. Only a move onto another
  /// token asks for rows again, so a dismissed list stays away while the caret stays put.
  public func select(_ range: Range<Int>) {
    let before = typedToken
    let end = text.utf16.count
    let isEnd = range == end..<end || range.lowerBound < 0 || range.upperBound > end
    selectedRange = isEnd ? nil : range
    if typedToken != before { complete() }
  }

  /// Adds `insert`, a command palette's `/command` or `@file` (T37.44.13), at the end of the
  /// draft and then one space, as a picked row would stand.
  public func append(_ insert: String) {
    let end = text.utf16.count
    let token = TypedToken(start: end, end: end, text: "")
    guard let splice = client.pick(text, token: token, insert: insert) else { return }
    edit(splice.text)
    mentions = client.mentions(text, picked: mentions + [insert])
    completions = []
  }

  /// Puts row `index`'s insert in place of the token at the caret, then one space, and leaves
  /// the caret after it.
  public func pick(_ index: Int) {
    guard completions.indices.contains(index), let token = typedToken else { return }
    let insert = completions[index].insert
    guard let splice = client.pick(text, token: token, insert: insert) else { return }
    text = splice.text
    selectedRange = splice.caret == text.utf16.count ? nil : splice.caret..<splice.caret
    mentions = client.mentions(text, picked: mentions + [insert])
    completions = []
  }

  public func dismissCompletion() { completions = [] }

  /// The draft shows an earlier prompt, so ↑ and ↓ keep walking.
  public var isRecalling: Bool { recalled != nil }

  /// ↑ (-1) to an older prompt or ↓ (+1) to a newer one. A walk starts with ↑ in an empty draft
  /// and ends with ↓ past the newest prompt, which empties the draft again.
  public func recall(_ step: Int) {
    if recalled == nil {
      guard step < 0, text.isEmpty, !isShell else { return }
      do {
        recalled = (try session.session.history(limit: Self.historyLimit), -1)
      } catch {
        return report(error)
      }
    }
    guard let (prompts, index) = recalled else { return }
    let next = index - step
    if next < 0 {
      (recalled, text, selectedRange) = (nil, "", nil)
    } else if prompts.indices.contains(next) {
      (recalled, text, completions, selectedRange) = ((prompts, next), prompts[next], [], nil)
    } else if index < 0 {
      recalled = nil
    }
  }

  /// Takes a picked file back out of the draft.
  public func removeMention(_ insert: String) {
    mentions.removeAll { $0 == insert }
    if let range = text.range(of: insert + " ") ?? text.range(of: insert) {
      text.removeSubrange(range)
    }
    selectedRange = nil
    complete()
  }

  public func leaveShell() { isShell = false }

  /// Reads each file off the main actor and attaches it, its media type from its extension. A
  /// file that cannot be read is named in `failure`; the others are still attached.
  public func attach(_ urls: [URL]) async {
    for url in urls {
      do {
        let data = try await Task.detached { try Self.read(url) }.value
        let type = UTType(filenameExtension: url.pathExtension)
        attach(data, name: url.lastPathComponent, type: type)
      } catch {
        report(error)
      }
    }
  }

  /// Bytes with no file behind them, such as a pasted screenshot.
  public func attach(_ data: Data, name: String, type: UTType?) {
    attachments.append(
      Attachment(
        name: name, mediaType: type?.preferredMIMEType ?? "application/octet-stream",
        dataB64: data.base64EncodedString()))
  }

  public func removeAttachment(at index: Int) {
    if attachments.indices.contains(index) { attachments.remove(at: index) }
  }

  /// Something the view could not do for the draft, such as open the file picker.
  public func report(_ error: any Error) {
    failure = String(describing: error)
  }

  /// A picked file may be security-scoped (a sandboxed file picker's); reading it asks for
  /// access for as long as the read takes.
  private nonisolated static func read(_ url: URL) throws -> Data {
    let scoped = url.startAccessingSecurityScopedResource()
    defer { if scoped { url.stopAccessingSecurityScopedResource() } }
    return try Data(contentsOf: url)
  }

  /// A turn runs, as the core's usage view says (`TurnStarted` until `TurnDone`).
  public var isRunning: Bool { session.isTurnRunning }

  /// Prompts queued behind the running turn that have not started, as the core counts them.
  public var queued: Int { Int(session.status.queued) }

  /// The permission mode in force, as the core reports it.
  public var mode: PermissionMode? { session.status.mode }

  /// The model the main turn runs on and its effort, `Sonnet 5 · high`: the core's short name,
  /// else the id. Swift does not shorten `modelName` (A129).
  public var model: String? {
    guard let id = session.status.model else { return nil }
    let model = session.status.shortName ?? id
    return session.status.effort.map { "\(model) · \($0.rawValue)" } ?? model
  }

  /// ⇧⇥: asks for the mode the core's cycle puts after this one. The chip moves when the core
  /// reports the change, never before.
  public func cycleMode() async {
    guard let next = session.status.nextMode else { return }
    do {
      _ = try await session.send(.setMode(mode: next))
    } catch {
      report(error)
    }
  }

  /// The think chip: on for the next turn, or off again before it is sent.
  public func toggleThink() { think.toggle() }

  /// Sends the draft: a shell line, a `/` command line for the core's command table, or a
  /// turn with the attachments — queued behind the running turn while one runs. The draft
  /// clears once the core took it; attachments stay for a shell or command line, which cannot
  /// carry them.
  public func submit() async {
    await send(draft(.queue))
  }

  /// ⌘⏎: interrupts the running turn, then sends the draft as a turn of its own.
  public func submitNow() async {
    guard canSend else { return }
    guard isRunning else { return await submit() }
    do {
      _ = try await session.send(.interrupt)
    } catch {
      return report(error)
    }
    await send(draft(.now))
  }

  /// What the core makes of the draft as it stands.
  private func draft(_ when: SendWhen) -> DraftIntent {
    client.draftIntent(
      text, shell: isShell, attachments: attachments.count, running: isRunning, when: when)
  }

  private func send(_ draft: DraftIntent) async {
    guard draft.canSend else { return }
    let intent: Intent =
      switch draft.kind {
      case .shell: .shell(command: text, share: shareOutput)
      case .command: .command(line: text)
      case .turn where draft.queued:
        .queue(text: text, attachments: attachments, confirmThink: think)
      case .turn: .send(text: text, attachments: attachments, confirmThink: think)
      }
    do {
      _ = try await session.send(intent)
      if !draft.keepsAttachments { (attachments, think) = ([], false) }
      (text, mentions, completions, isShell, failure) = ("", [], [], false, nil)
      (recalled, selectedRange) = (nil, nil)
    } catch {
      report(error)
    }
  }

  /// The word the caret ends, when the core says it asks for rows.
  private var typedToken: TypedToken? {
    let end = text.utf16.count
    let range = selectedRange ?? end..<end
    return client.typedToken(
      text, caret: range.upperBound, selection: !range.isEmpty, shell: isShell)
  }

  private var client: any SessionClient { session.session }

  private func complete() {
    completions = typedToken.map { client.complete($0.text, limit: Self.rowLimit) } ?? []
    selection = 0
  }
}
