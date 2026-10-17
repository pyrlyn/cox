// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Inspector` (DS§6.4 row `Inspector`, the mockup's `.insp`; DT§5.1): the pane at the trailing
// edge of the window — its title, the tab strip and the selected tab's content. Separate so the
// window shell owns the pane and its tabs while each tab's content is its own view (T37.29).
// A plugin's tab (PL§15.4) follows Info as its symbol, the title as tooltip, so a few plugins fit
// the fixed width; more than fit scroll sideways.

import SwiftUI

/// A floating `ShellPane(.inspector)` of `Size.inspectorWidth`: the title, the tabs with the
/// selected one marked, then `content` for the selected tab (`ChangesTab`, …), scrolling.
struct Inspector<Content: View>: View {
  let selection: InspectorTab
  let pluginTabs: [PluginTabItem]
  let select: (InspectorTab) -> Void
  let content: Content

  init(
    selection: InspectorTab, pluginTabs: [PluginTabItem] = [], content: Content,
    select: @escaping (InspectorTab) -> Void
  ) {
    self.selection = selection
    self.pluginTabs = pluginTabs
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
        ViewThatFits(in: .horizontal) {
          strip
          ScrollView(.horizontal, showsIndicators: false) { strip }
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

  private var strip: some View {
    HStack(spacing: Space.xxs) {
      ForEach(InspectorTab.builtIn, id: \.self) { tab in
        TabButton(title: tab.title, isSelected: tab == selection) { select(tab) }
      }
      ForEach(pluginTabs, id: \.plugin) { item in
        let tab = InspectorTab.plugin(item.plugin)
        TabButton(
          title: item.title, icon: PluginSymbol.name(item.symbol), isSelected: tab == selection
        ) {
          select(tab)
        }
      }
    }
  }
}

/// The DT§5.1 tabs, then one per plugin that declares one.
public enum InspectorTab: Hashable, Sendable {
  case changes, plan, context, tasks, info
  /// The tab of the plugin with this id.
  case plugin(String)

  /// The built-in tabs, in the order the strip shows them.
  public static let builtIn: [InspectorTab] = [.changes, .plan, .context, .tasks, .info]

  var title: String {
    switch self {
    case .changes: "Changes"
    case .plan: "Plan"
    case .context: "Context"
    case .tasks: "Tasks"
    case .info: "Info"
    case .plugin(let plugin): plugin
    }
  }
}

/// A plugin's inspector tab as the strip draws it.
public struct PluginTabItem: Equatable, Sendable {
  public var plugin: String
  public var title: String
  public var symbol: String?

  public init(plugin: String, title: String, symbol: String?) {
    (self.plugin, self.title, self.symbol) = (plugin, title, symbol)
  }
}

/// A plugin's tab content (PL§15.4): its tree in one `InspectorSection` titled as the tab, or a
/// quiet line until the plugin's first render arrives.
public struct PluginInspectorTab: View {
  let title: String
  let widget: PluginWidget?

  public init(title: String, widget: PluginWidget?) {
    (self.title, self.widget) = (title, widget)
  }

  public var body: some View {
    InspectorSection(title) {
      if let widget {
        PluginWidgetView(widget)
      } else {
        Text("Nothing to show yet").textStyle(.caption).foregroundStyle(Color(.textSecondary))
      }
    }
  }
}

/// One tab: a quiet label, or, while selected, lifted on the window surface on glass and a
/// `fill.secondary` well on Solid — the mockup's `.glass .tabs span.on` and `.tabs span.on`
/// (screens 28–30 and 01–02); a lifted white tab on a white Solid pane reads only by its shadow.
private struct TabButton: View {
  let title: String
  /// Set for a plugin's tab, which draws as this symbol.
  var icon: String?
  let isSelected: Bool
  let action: () -> Void
  @EffectiveAppearance private var appearance

  init(title: String, icon: String? = nil, isSelected: Bool, action: @escaping () -> Void) {
    (self.title, self.icon, self.isSelected, self.action) = (title, icon, isSelected, action)
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.tab, style: .continuous)
    let isLifted = isSelected && appearance.material != .solid
    Button(action: action) {
      label
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
    .buttonStyle(.coxPlain(isChosen: isSelected))
    .help(icon == nil ? "" : title)
    .accessibilityLabel(title)
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }

  @ViewBuilder private var label: some View {
    if let icon {
      Image(systemName: icon)
    } else {
      Text(title)
    }
  }
}

#Preview("empty tabs") {
  PreviewMatrix {
    Inspector(selection: .changes, content: EmptyView()) { _ in }
      .frame(height: Size.toolbarHeight * 2)
  }
}

#Preview("plugin tab") {
  PreviewMatrix { PreviewState.pluginInspector }
}
