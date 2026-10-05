// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Inspector` (DS§6.4 row `Inspector`, the mockup's `.insp`; DT§5.1): the pane at the trailing
// edge of the window — its title, the tab strip and the selected tab's content. Separate so the
// window shell owns the pane and its tabs while each tab's content is its own view (T37.29).

import SwiftUI

/// A floating `ShellPane(.inspector)` of `Size.inspectorWidth`: the title, the tabs with the
/// selected one marked, then `content` for the selected tab (`ChangesTab`, …), scrolling.
struct Inspector<Content: View>: View {
  let selection: InspectorTab
  let select: (InspectorTab) -> Void
  let content: Content

  init(selection: InspectorTab, content: Content, select: @escaping (InspectorTab) -> Void) {
    self.selection = selection
    self.content = content
    self.select = select
  }

  var body: some View {
    ShellPane(.inspector) {
      VStack(alignment: .leading, spacing: 0) {
        Text("Inspector")
          .textStyle(.titleWindow)
          .foregroundStyle(Color(.textPrimary))
          .accessibilityAddTraits(.isHeader)
          .padding(.horizontal, Space.xl)
          .frame(height: Size.toolbarHeight)
        HStack(spacing: Space.xxs) {
          ForEach(InspectorTab.allCases, id: \.self) { tab in
            TabButton(title: tab.title, isSelected: tab == selection) { select(tab) }
          }
        }
        .padding(.horizontal, Space.l)
        .padding(.bottom, Space.ml)
        .frame(maxWidth: .infinity, alignment: .leading)
        .hairline(.bottom)
        // The mockup's `.ib`: every tab scrolls in the same inset body. Its first `.ih` sits
        // 18 pt under the tabs (14 of padding and 4 of its own margin): `space.xl` is nearest.
        ScrollView {
          content
            .padding(.horizontal, Space.xl)
            .padding(.top, Space.xl)
            .padding(.bottom, Space.l)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(maxHeight: .infinity)
      }
    }
    .frame(width: Size.inspectorWidth)
    .coxTransition(.move(edge: .trailing).combined(with: .opacity))
  }
}

/// The DT§5.1 tabs, in the order the strip shows them.
public enum InspectorTab: CaseIterable, Sendable {
  case changes, plan, context, tasks, info

  var title: String {
    switch self {
    case .changes: "Changes"
    case .plan: "Plan"
    case .context: "Context"
    case .tasks: "Tasks"
    case .info: "Info"
    }
  }
}

/// One tab: a quiet label, or, while selected, lifted on the window surface on glass and a
/// `fill.secondary` well on Solid — the mockup's `.glass .tabs span.on` and `.tabs span.on`
/// (screens 28–30 and 01–02); a lifted white tab on a white Solid pane reads only by its shadow.
private struct TabButton: View {
  let title: String
  let isSelected: Bool
  let action: () -> Void
  @EffectiveAppearance private var appearance

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.tab, style: .continuous)
    let isLifted = isSelected && appearance.material != .solid
    Button(action: action) {
      Text(title)
        .textStyle(.segment)
        .foregroundStyle(Color(isSelected ? .textPrimary : .textSecondary))
        .padding(.horizontal, Space.tab)
        .padding(.vertical, Space.xs)
        .background {
          if isSelected { shape.fill(Color(isLifted ? .surfaceWindow : .fillSecondary)) }
        }
        .elevation(isLifted ? .e1 : .e0, cornerRadius: Radius.tab)
        .contentShape(shape)
    }
    .buttonStyle(.plain)
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }
}

#Preview("empty tabs") {
  PreviewMatrix {
    Inspector(selection: .changes, content: EmptyView()) { _ in }
      .frame(height: Size.toolbarHeight * 2)
  }
}
