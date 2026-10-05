// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the rewind timeline (T37.28.1): the Changes tab's checkpoints with
// the last one lifted, as a gutter mark would point at it. Separate so organisms built in
// parallel add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  /// Every checkpoint, the newest lifted.
  static let rewind = RewindTimeline.State(
    checkpoints: checkpoints, selection: checkpoints.last?.id)
}

/// The timeline on an inspector-wide pane, inset like an inspector tab's body, so its text sits
/// on the surface it is drawn on (DS§8).
struct RewindTimelineSample: View {
  let state: RewindTimeline.State

  var body: some View {
    ShellPane(.inspector) {
      RewindTimeline(state: state) { _ in }
        .padding(.horizontal, Space.xl)
        .padding(.vertical, Space.l)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
    .frame(width: Size.inspectorWidth)
  }
}
