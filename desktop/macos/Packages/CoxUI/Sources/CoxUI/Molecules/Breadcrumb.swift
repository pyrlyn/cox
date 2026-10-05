// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Breadcrumb` (DS§6.3 row `Breadcrumb`, the mockup's `.crumb`): where the open session lives —
// its title, then its project and branch — at the leading end of the toolbar. Separate so the
// toolbar and any detached session window name a session the same way.

import SwiftUI

/// The title in `font.title.window`, a chevron, then the project and branch in `text.secondary`.
/// With `rename`, a double-click on the title edits it in place (A113): Return or a click away
/// commits, Escape cancels.
struct Breadcrumb: View {
  let title: String
  let project: String
  /// The worktree's branch, or `nil` outside git.
  let branch: String?
  /// Receives the edited title when it has text and differs; `nil` keeps the title read-only.
  let rename: ((String) -> Void)?
  /// The title being edited; `nil` while it is not.
  @State private var draft: String?
  @FocusState private var isFocused: Bool

  init(
    _ title: String, project: String, branch: String? = nil,
    rename: ((String) -> Void)? = nil
  ) {
    self.title = title
    self.project = project
    self.branch = branch
    self.rename = rename
  }

  var body: some View {
    HStack(spacing: Space.s) {
      if draft != nil {
        TextField(
          "Session title", text: Binding(get: { draft ?? "" }, set: { draft = $0 })
        )
        .textFieldStyle(.plain)
        .textStyle(.titleWindow)
        .foregroundStyle(Color(.textPrimary))
        .focused($isFocused)
        .onSubmit(commit)
        .onExitCommand { draft = nil }
        .onAppear { isFocused = true }
        .onChange(of: isFocused) { if !isFocused { commit() } }
        .frame(minWidth: Size.sidebarWidth / 2)
        .fixedSize()
      } else {
        Text(title)
          .textStyle(.titleWindow)
          .foregroundStyle(Color(.textPrimary))
          .lineLimit(1)
          .layoutPriority(1)
          .onTapGesture(count: 2) { if rename != nil { draft = title } }
          .help(rename == nil ? "" : "Double-click to rename")
          .accessibilityLabel([title, project, branch].compactMap(\.self).joined(separator: ", "))
          .accessibilityAction(named: "Rename") { if rename != nil { draft = title } }
      }
      Group {
        Image(systemName: "chevron.right").symbolStyle(.micro)
        Text(project).textStyle(.control)
        if let branch {
          Image(systemName: "arrow.triangle.branch").symbolStyle(.body)
          Text(branch).textStyle(.control).lineLimit(1).truncationMode(.middle)
        }
      }
      .foregroundStyle(Color(.textSecondary))
      .accessibilityHidden(true)
    }
  }

  /// Ends the edit, handing a changed title with text to `rename`.
  private func commit() {
    guard let text = draft?.trimmingCharacters(in: .whitespacesAndNewlines) else { return }
    draft = nil
    if !text.isEmpty && text != title { rename?(text) }
  }
}

#Preview("branch") { PreviewMatrix { BreadcrumbSample(branch: PreviewState.branch) } }
#Preview("no branch") { PreviewMatrix { BreadcrumbSample(branch: nil) } }
