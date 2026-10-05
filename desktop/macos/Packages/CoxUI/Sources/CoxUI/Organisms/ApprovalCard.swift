// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ApprovalCard` (DS§6.4 row `ApprovalCard`, the mockup's `.appr`, DT§5.2 Approval): a tool call
// that waits on the person — what it will run, why they are asked, which subagent asks, what
// Allow for session would grant — with Allow, Allow for session, Deny and Edit… (T37.27.6); once
// decided, the one line that says how. Separate so the
// transcript card and the pinned bar above the composer draw an approval alike. `DecisionFrame`
// here is shared with `QuestionCard`, the other card that waits on the person.

import SwiftUI

/// Pending: a header with a warning symbol and the risk, the command in a code well, the
/// reasons in `text.secondary`, then the buttons and the session grant on `fill.primary` under a
/// hairline — a readable face at e1 with an orange edge. Decided: a `NoticeRow`, "Allowed by you · for session".
/// Editing: the input as JSON in the well, Run edited and Cancel; the draft is the card's own
/// until it is sent.
public struct ApprovalCard: View {
  /// What the card shows; every string comes formatted from the transcript's mapping.
  public struct Content: Equatable, Sendable {
    /// `Run this command?`
    public var title: String
    /// The command or call, drawn monospaced in the well.
    public var command: String
    /// Why the person is asked, `matches ask rule Bash(git push:*)`.
    public var reason: String
    /// The subagent that asks; `nil` for the main agent.
    public var source: String?
    public var risk: ToolHeader.Risk?
    /// Set once decided: the card shrinks to this line.
    public var outcome: Outcome?
    /// What Allow for session would grant, `bash: git status · npm test`; `nil` hides the line.
    public var grant: String?
    /// The call's input as JSON text, what Edit… starts from; `nil` hides Edit….
    public var input: String?

    public init(
      title: String, command: String, reason: String, source: String? = nil,
      risk: ToolHeader.Risk? = nil, outcome: Outcome? = nil, grant: String? = nil,
      input: String? = nil
    ) {
      (self.title, self.command, self.reason) = (title, command, reason)
      (self.source, self.risk, self.outcome) = (source, risk, outcome)
      (self.grant, self.input) = (grant, input)
    }
  }

  /// How the call was decided, `Allowed by you · for session`.
  public struct Outcome: Equatable, Sendable {
    public var text: String
    public var isAllowed: Bool

    public init(text: String, isAllowed: Bool) {
      self.text = text
      self.isAllowed = isAllowed
    }
  }

  /// The fixed buttons; Edit… sends its JSON through `edit` instead.
  public enum Action: CaseIterable, Sendable {
    case allow, allowForSession, deny
  }

  let content: Content
  let act: (Action) -> Void
  let edit: ((String) -> Void)?
  /// The input being edited; `nil` when not editing.
  @State private var draft: String?

  public init(_ content: Content, act: @escaping (Action) -> Void) {
    self.content = content
    self.act = act
    self.edit = nil
  }

  /// With Edit…: the edited input, JSON text, goes to `edit`.
  public init(
    _ content: Content, act: @escaping (Action) -> Void, edit: @escaping (String) -> Void
  ) {
    self.content = content
    self.act = act
    self.edit = edit
  }

  public var body: some View {
    if let outcome = content.outcome {
      NoticeRow(outcome.text, symbol: outcome.isAllowed ? "checkmark.circle" : "xmark.circle")
    } else {
      DecisionFrame(edge: Color(.statusWarning)) {
        DecisionTitle(
          content.title, symbol: "exclamationmark.triangle", tint: Color(.statusWarning)
        ) {
          if let risk = content.risk { RiskChip(risk.text, level: risk.level) }
        }
        well
        VStack(alignment: .leading, spacing: Space.xs) {
          DecisionReason(label: "Why you are asked:", text: content.reason)
          if let source = content.source { DecisionReason(label: "Asked by:", text: source) }
        }
      } actions: {
        if draft != nil {
          Button("Run edited", action: runEdited)
            .buttonStyle(CoxButtonStyle(.primary))
            .disabled(EditedInput(draft).json == nil)
          Button("Cancel") { draft = nil }.buttonStyle(CoxButtonStyle(.secondary))
          Spacer(minLength: 0)
        } else {
          // The mockup's `.grant` stands at the row's trailing end; a grant too long for the
          // line goes under the buttons whole, since it says what Allow for session allows.
          ViewThatFits(in: .horizontal) {
            HStack(spacing: Space.m) {
              buttons
              Spacer(minLength: Space.m)
              grant
            }
            VStack(alignment: .leading, spacing: Space.m) {
              HStack(spacing: Space.m) { buttons }
              grant
            }
            .frame(maxWidth: .infinity, alignment: .leading)
          }
        }
      }
    }
  }

  /// Allow, Allow for session, Edit… and Deny, in the mockup's order.
  @ViewBuilder private var buttons: some View {
    Button("Allow") { act(.allow) }.buttonStyle(CoxButtonStyle(.primary))
    Button("Allow for session") { act(.allowForSession) }
      .buttonStyle(CoxButtonStyle(.secondary))
    if let input = content.input, edit != nil {
      Button("Edit…") { draft = input }.buttonStyle(CoxButtonStyle(.secondary))
    }
    Button("Deny") { act(.deny) }.buttonStyle(CoxButtonStyle(.danger))
  }

  /// What Allow for session would grant, `Session grant: git push *`.
  @ViewBuilder private var grant: some View {
    if let grant = content.grant {
      Text("Session grant: \(grant)")
        .textStyle(.footnote)
        .foregroundStyle(Color(.textSecondary))
        .fixedSize(horizontal: false, vertical: true)
    }
  }

  /// The command in the code well, or the input being edited in its place.
  @ViewBuilder private var well: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.m, style: .continuous)
    Group {
      if let text = draft {
        TextField(
          "Input", text: Binding(get: { text }, set: { draft = $0 }), axis: .vertical
        )
        .textFieldStyle(.plain)
        .lineLimit(3...12)
        .onSubmit(runEdited)
        .accessibilityLabel("Edited input")
      } else {
        Text(content.command).textSelection(.enabled)
      }
    }
    .textStyle(.monoCommand)
    .foregroundStyle(Color(.textPrimary))
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.horizontal, Space.l)
    .padding(.vertical, Space.m)
    .background { shape.fill(Color(.surfaceCode)) }
    .hairline(in: shape)
  }

  private func runEdited() {
    guard let json = EditedInput(draft).json, let edit else { return }
    edit(json)
    draft = nil
  }
}

/// A draft of an edited input: sent only as JSON the core can parse, trimmed. Whether the edit
/// is allowed stays the core's; this only keeps a typo from failing on the way there.
struct EditedInput {
  let json: String?

  init(_ draft: String?) {
    let text = draft?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    let parses = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) != nil
    json = parses ? text : nil
  }
}

/// The mockup's `.appr` shell both waiting cards share: the body, then the action row on
/// `fill.primary` under a hairline, on a readable face at e1 with a coloured leading edge.
struct DecisionFrame<Body: View, Actions: View>: View {
  let edge: Color
  @ViewBuilder let content: Body
  @ViewBuilder let actions: Actions

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.xxl, style: .continuous)
    VStack(alignment: .leading, spacing: 0) {
      VStack(alignment: .leading, spacing: Space.m) { content }
        .padding(.top, Space.l)
        .padding(.bottom, Space.ml)
        .padding(.leading, Space.xxl)
        .padding(.trailing, Space.xl)
      HStack(spacing: Space.m) { actions }
        .padding(.top, Space.ml)
        .padding(.bottom, Space.l)
        .padding(.leading, Space.xxl)
        .padding(.trailing, Space.xl)
        .background(Color(.fillPrimary))
        .hairline(.top)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .background { Color.clear.glassPane(shape, role: .readable) }
    // The mockup's 4 px edge: the nearest step, `space.xs`.
    .overlay(alignment: .leading) { edge.frame(width: Space.xs) }
    .clipShape(shape)
    .hairline(in: shape)
    .elevation(.e1, cornerRadius: Radius.xxl)
  }
}

/// A waiting card's header: its symbol in `tint`, the title, then any trailing chips.
struct DecisionTitle<Trailing: View>: View {
  let title: String
  let symbol: String
  let tint: Color
  @ViewBuilder let trailing: Trailing

  init(
    _ title: String, symbol: String, tint: Color,
    @ViewBuilder trailing: () -> Trailing = { EmptyView() }
  ) {
    (self.title, self.symbol, self.tint) = (title, symbol, tint)
    self.trailing = trailing()
  }

  var body: some View {
    HStack(spacing: Space.m) {
      Image(systemName: symbol).symbolStyle(.titleWindow).foregroundStyle(tint)
        .accessibilityHidden(true)
      Text(title).textStyle(.titleWindow).foregroundStyle(Color(.textPrimary))
      trailing
      Spacer(minLength: 0)
    }
  }
}

/// One reason line, the mockup's `.why`: the label in `text.primary`, the rest secondary.
private struct DecisionReason: View {
  let label: String
  let text: String

  var body: some View {
    let rest = Text(text).foregroundStyle(Color(.textSecondary))
    Text("\(Text(label).fontWeight(.medium).foregroundStyle(Color(.textPrimary))) \(rest)")
      .textStyle(.caption)
      .fixedSize(horizontal: false, vertical: true)
  }
}

#Preview("pending") { PreviewMatrix { ApprovalCardSample(PreviewState.approvalPending) } }
#Preview("subagent, risky") {
  PreviewMatrix { ApprovalCardSample(PreviewState.approvalRisky) }
}
#Preview("grant, editable") {
  PreviewMatrix { ApprovalCardSample(PreviewState.approvalGrant) }
}
#Preview("allowed") { PreviewMatrix { ApprovalCardSample(PreviewState.approvalAllowed) } }
#Preview("denied") { PreviewMatrix { ApprovalCardSample(PreviewState.approvalDenied) } }
