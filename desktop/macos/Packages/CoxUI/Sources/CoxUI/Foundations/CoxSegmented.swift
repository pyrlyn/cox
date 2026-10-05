// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CoxSegmented` (DS§6.1, the mockup's `.seg`): the segmented control, a glass capsule whose
// selection is a pill lifted to e1 that slides between segments, or cross-fades under Reduce
// Motion. A view rather than a `PickerStyle`: SwiftUI has no public hook to restyle the
// segments of a picker on macOS, so this takes a picker's inputs and VoiceOver sees a picker.

import SwiftUI

/// How the selected segment is marked: the lifted window-surface pill with a primary label
/// (`plain`), that pill with a coloured label (`tinted`), or a pill filled with the colour
/// under a `text.onAccent` label (`filled`) — the DS§3.1 mode colours.
enum SegmentLook: Equatable, Sendable {
  case plain
  case tinted(Color)
  case filled(Color)
}

/// Mutually exclusive `options`, each named by `title` and marked by `look` when selected, with
/// `selection` lifted.
struct CoxSegmented<Option: Hashable>: View {
  let label: LocalizedStringKey
  @Binding var selection: Option
  let options: [Option]
  let look: (Option) -> SegmentLook
  let title: (Option) -> Text
  @Namespace private var pill

  init(
    _ label: LocalizedStringKey, selection: Binding<Option>, options: [Option],
    look: @escaping (Option) -> SegmentLook = { _ in .plain },
    title: @escaping (Option) -> Text
  ) {
    self.label = label
    self._selection = selection
    self.options = options
    self.look = look
    self.title = title
  }

  var body: some View {
    HStack(spacing: Space.xxs) {
      ForEach(options, id: \.self) { option in
        Segment(
          title: title(option), look: option == selection ? look(option) : nil, pill: pill
        ) { selection = option }
      }
    }
    .padding(Space.xxs)
    .frame(height: Size.capsuleHeight)
    .glassPane(Capsule(), surface: Color(.surfaceCapsule), role: .readable)
    .hairline(in: Capsule(), color: Color(.surfaceCapsuleBorder))
    .elevation(.e1, cornerRadius: Radius.capsule)
    .animation(.cox(Motion.durationBase), value: selection)
    .accessibilityRepresentation {
      Picker(label, selection: $selection) {
        ForEach(options, id: \.self) { title($0).tag($0) }
      }
      .pickerStyle(.segmented)
    }
  }
}

private struct Segment: View {
  let title: Text
  /// The selected segment's look; `nil` for every other segment.
  let look: SegmentLook?
  let pill: Namespace.ID
  let select: () -> Void

  var body: some View {
    Button(action: select) {
      title
        .textStyle(.segment)
        .foregroundStyle(look?.label ?? Color(.textSecondary))
        .padding(.horizontal, Space.ml)
        .frame(maxHeight: .infinity)
        .background {
          if let look { SelectionPill(fill: look.fill, pill: pill) }
        }
        .contentShape(Capsule())
    }
    .buttonStyle(.plain)
  }
}

/// The selected segment's lifted surface; one per control, so it moves rather than blinks.
private struct SelectionPill: View {
  let fill: Color
  let pill: Namespace.ID

  var body: some View {
    Capsule()
      .fill(fill)
      .elevation(.e1, cornerRadius: Radius.capsule)
      .coxMatchedGeometry(id: 0, in: pill)
  }
}

extension SegmentLook {
  var label: Color {
    switch self {
    case .plain: Color(.textPrimary)
    case .tinted(let color): color
    case .filled: Color(.textOnAccent)
    }
  }

  var fill: Color {
    switch self {
    case .plain, .tinted: Color(.surfaceWindow)
    case .filled(let color): color
    }
  }
}
