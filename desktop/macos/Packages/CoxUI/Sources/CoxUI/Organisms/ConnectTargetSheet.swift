// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ConnectTargetSheet` (DS§6.4, DT§5.10, T68.5): File › Connect to Target…, a kind picker (SSH,
// Docker, Lima, Colima), the targets of that kind the core lists, a free-text name, and Connect.
// Once the target answered `hello`, the sheet shows its OS and sandbox and waits for Done, so the
// person sees where the session will run before any opens. Separate so the app only presents it:
// the core checks the name, dials and reports why a connect failed, and the sheet shows that line.

import SwiftUI

public struct ConnectTargetSheet: View {
  /// One kind in the picker, as CoxModel names it.
  public struct Kind: Identifiable, Equatable, Sendable {
    public let id: String
    public let title: String
    /// What the empty name field suggests.
    public let example: String

    public init(id: String, title: String, example: String) {
      (self.id, self.title, self.example) = (id, title, example)
    }
  }

  /// One listed target; `id` is the text the core connects with.
  public struct Option: Identifiable, Equatable, Sendable {
    public let id: String
    public let kind: String
    public let name: String
    /// Named in config, as opposed to found running.
    public let isConfigured: Bool

    public init(id: String, kind: String, name: String, isConfigured: Bool) {
      (self.id, self.kind, self.name, self.isConfigured) = (id, kind, name, isConfigured)
    }
  }

  /// The `hello` answer: the machine in one line, and its sandbox if it has one.
  public struct Hello: Equatable, Sendable {
    public let summary: String
    public let sandbox: String?

    public init(summary: String, sandbox: String?) {
      (self.summary, self.sandbox) = (summary, sandbox)
    }
  }

  public struct State: Equatable, Sendable {
    public var kinds: [Kind]
    public var options: [Option]
    /// Why the last connect failed, as the core said it.
    public var failure: String?
    /// A connect runs; Connect waits.
    public var isConnecting: Bool
    /// Set once the target connected; the sheet then offers Done only.
    public var hello: Hello?

    public init(
      kinds: [Kind] = [], options: [Option] = [], failure: String? = nil,
      isConnecting: Bool = false, hello: Hello? = nil
    ) {
      (self.kinds, self.options, self.failure) = (kinds, options, failure)
      (self.isConnecting, self.hello) = (isConnecting, hello)
    }
  }

  public enum Intent: Equatable, Sendable {
    /// A listed target, by its text.
    case connect(target: String)
    /// A typed name under a kind.
    case connectTyped(kind: String, name: String)
    case close
  }

  let state: State
  let send: (Intent) -> Void
  @SwiftUI.State private var kind: String
  @SwiftUI.State private var picked: String?
  @SwiftUI.State private var draft: String

  public init(
    state: State, kind: String? = nil, picked: String? = nil, name: String = "",
    send: @escaping (Intent) -> Void
  ) {
    (self.state, self.send) = (state, send)
    _kind = SwiftUI.State(initialValue: kind ?? state.kinds.first?.id ?? "")
    _picked = SwiftUI.State(initialValue: picked)
    _draft = SwiftUI.State(initialValue: name)
  }

  private var name: String { draft.trimmingCharacters(in: .whitespacesAndNewlines) }
  private var listed: [Option] { state.options.filter { $0.kind == kind } }
  private var canConnect: Bool { (picked != nil || !name.isEmpty) && !state.isConnecting }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.l) {
      Text("Connect to Target").textStyle(.body).fontWeight(.semibold)
        .foregroundStyle(Color(.textPrimary))
        .accessibilityAddTraits(.isHeader)
      if let hello = state.hello {
        HelloBlock(hello: hello)
        HStack {
          Spacer(minLength: 0)
          Button("Done") { send(.close) }
            .buttonStyle(CoxButtonStyle(.primary, size: .small))
            .keyboardShortcut(.defaultAction)
        }
      } else {
        picker
        buttons
      }
    }
    .padding(Space.xl)
    .frame(width: Size.popoverWidth)
  }

  @ViewBuilder private var picker: some View {
    Text(
      "cox runs on the target, over your own ssh, Docker, Lima or Colima. No key or ssh agent leaves this Mac."
    )
    .textStyle(.caption)
    .foregroundStyle(Color(.textSecondary))
    .fixedSize(horizontal: false, vertical: true)
    CoxSegmented(
      "Kind", selection: Binding(get: { kind }, set: { (kind, picked) = ($0, nil) }),
      options: state.kinds.map(\.id),
      title: { id in Text(state.kinds.first { $0.id == id }?.title ?? id) })
    VStack(spacing: Space.xxs) {
      if listed.isEmpty {
        Text("Nothing listed. Type a name below.").textStyle(.caption)
          .foregroundStyle(Color(.textTertiary))
          .frame(maxWidth: .infinity, alignment: .leading)
      }
      ForEach(listed) { option in
        TargetRow(option: option, isPicked: option.id == picked) {
          (picked, draft) = (option.id, "")
        }
      }
    }
    HStack(spacing: Space.s) {
      Image(systemName: "server.rack").symbolStyle(.caption).foregroundStyle(Color(.textSecondary))
      TextField(
        "Name", text: $draft,
        prompt: Text(state.kinds.first { $0.id == kind }?.example ?? "")
          .foregroundStyle(Color(.textPlaceholder))
      )
      .textFieldStyle(.plain)
      .textStyle(.monoInline)
      .foregroundStyle(Color(.textPrimary))
      .onChange(of: draft) { if !draft.isEmpty { picked = nil } }
      .onSubmit(connect)
    }
    .padding(.horizontal, Space.ml)
    .frame(height: Size.buttonHeight)
    .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
    if let failure = state.failure {
      Text(failure).textStyle(.caption).foregroundStyle(Color(.statusDanger))
        .fixedSize(horizontal: false, vertical: true)
    }
  }

  private var buttons: some View {
    HStack(spacing: Space.s) {
      Spacer(minLength: 0)
      Button("Cancel") { send(.close) }
        .buttonStyle(CoxButtonStyle(.secondary, size: .small))
        .keyboardShortcut(.cancelAction)
      Button(state.isConnecting ? "Connecting…" : "Connect", action: connect)
        .buttonStyle(CoxButtonStyle(.primary, size: .small))
        .keyboardShortcut(.defaultAction)
        .disabled(!canConnect)
    }
  }

  private func connect() {
    guard canConnect else { return }
    send(picked.map { .connect(target: $0) } ?? .connectTyped(kind: kind, name: name))
  }
}

/// A listed target: its name, and whether config names it or it was found running.
private struct TargetRow: View {
  let option: ConnectTargetSheet.Option
  let isPicked: Bool
  let pick: () -> Void

  var body: some View {
    Button(action: pick) {
      HStack(spacing: Space.s) {
        Text(option.name).textStyle(.monoInline).foregroundStyle(Color(.textPrimary))
        Spacer(minLength: 0)
        Text(option.isConfigured ? "configured" : "running").textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
      }
      .padding(.horizontal, Space.ml)
      .frame(height: Size.buttonHeightSmall)
      .background(isPicked ? Color(.accentSoft) : .clear, in: .rect(cornerRadius: Radius.m))
      .contentShape(.rect)
    }
    .buttonStyle(.plain)
    .accessibilityAddTraits(isPicked ? .isSelected : [])
  }
}

/// What `hello` answered: the machine, and a warning line when it has no sandbox (the session then
/// asks before every command).
private struct HelloBlock: View {
  let hello: ConnectTargetSheet.Hello

  var body: some View {
    VStack(alignment: .leading, spacing: Space.s) {
      Label {
        Text(hello.summary).textStyle(.monoInline).foregroundStyle(Color(.textPrimary))
      } icon: {
        Image(systemName: "checkmark.circle.fill").symbolStyle(.caption)
          .foregroundStyle(Color(.statusSuccess))
      }
      if let sandbox = hello.sandbox {
        Text("Sandbox: \(sandbox)").textStyle(.caption).foregroundStyle(Color(.textSecondary))
      } else {
        Text("No sandbox there: cox asks before every command.").textStyle(.caption)
          .foregroundStyle(Color(.statusWarning))
      }
    }
  }
}

#Preview("empty") {
  PreviewMatrix { ConnectTargetSheet(state: PreviewState.connectTargetListed) { _ in } }
}
#Preview("connected") {
  PreviewMatrix { ConnectTargetSheet(state: PreviewState.connectTargetHello) { _ in } }
}
