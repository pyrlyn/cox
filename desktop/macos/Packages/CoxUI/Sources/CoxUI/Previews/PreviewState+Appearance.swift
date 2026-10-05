// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` samples for the Appearance popover's material picker (T37.21.8), as mockup
// screens 28 and 29 show it. Separate from `PreviewState.swift` so molecules built in parallel
// add their samples without editing one file.

import SwiftUI

/// The material picker at the popover's width with `selection` chosen, at the cell's Depth or
/// at `depth` when one is given — Flat (0) drops every swatch's lift.
struct MaterialPickerSample: View {
  let selection: GlassMaterial
  var depth: Double?
  @Environment(\.coxAppearance) private var appearance

  var body: some View {
    MaterialPicker(selection: .constant(selection))
      .frame(width: Size.popoverWidth)
      .environment(\.coxAppearance, look)
  }

  private var look: Appearance {
    var look = appearance
    if let depth { look.depth = depth }
    return look
  }
}
