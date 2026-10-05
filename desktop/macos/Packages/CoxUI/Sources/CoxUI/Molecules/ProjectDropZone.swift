// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ProjectDropZone` (DS§6.3, mockup screen 21's dashed onboarding box; DT§5.8, T37.45.5): the
// first-run window's Open a project step, which also takes a project folder dropped on it.
// Separate so the rule for what a drop may open — one local directory, nothing else — lives in
// one place, beside the look that says a drop is welcome.

import SwiftUI

/// The Open a project `ChecklistRow` inside a dashed `separator` border that turns `accent` while
/// a drag hovers over it. A drop of one directory reports `.openFolder`, the intent the app's
/// folder picker reports for the folder it chose; anything else is refused and reports nothing.
struct ProjectDropZone: View {
  let send: (OnboardingScreenIntent) -> Void
  /// A drag is over the zone. Local UI state (DS§9); a snapshot starts it hovered.
  @State private var isTargeted: Bool

  init(isTargeted: Bool = false, send: @escaping (OnboardingScreenIntent) -> Void) {
    _isTargeted = State(initialValue: isTargeted)
    self.send = send
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.panel, style: .continuous)
    ChecklistRow(
      "Open a project", detail: "Choose a folder or drop it here. A git repository is recommended.",
      status: .step, symbol: "folder", action: "Choose Folder…"
    ) { send(.chooseFolder) }
    .padding(Space.m)
    .contentShape(shape)
    .dashedBorder(in: shape, color: isTargeted ? Color(.accent) : Color(.separator))
    .animation(.cox(Motion.durationFast), value: isTargeted)
    .dropDestination(for: URL.self) { urls, _ in
      guard let intent = Self.intent(for: urls) else { return false }
      send(intent)
      return true
    } isTargeted: {
      isTargeted = $0
    }
  }

  /// What a drop of `urls` opens: exactly one local directory (a link to one counts), reported as
  /// dropped. A file, a web link or several items open nothing, so the drop is refused.
  /// Nonisolated: it reads only the URLs and the file system, never the view.
  nonisolated static func intent(for urls: [URL]) -> OnboardingScreenIntent? {
    guard urls.count == 1, let url = urls.first, url.isFileURL else { return nil }
    let values = try? url.resolvingSymlinksInPath().resourceValues(forKeys: [.isDirectoryKey])
    return values?.isDirectory == true ? .openFolder(url) : nil
  }
}

#Preview("idle and hovered") {
  PreviewMatrix {
    VStack(spacing: Space.xl) {
      ProjectDropZone { _ in }
      ProjectDropZone(isTargeted: true) { _ in }
    }
    .frame(width: Size.readingWidth)
  }
}
