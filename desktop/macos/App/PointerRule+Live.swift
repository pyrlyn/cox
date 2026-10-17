// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The shared pointer rule for CoxUI's `coxPointer` (A141, T37.51): the answer comes from
// `cox_render::pointer` through CoxCore, and this only joins the two packages' value types, which
// neither can see in the other (CoxUI links no Rust, CoxCore no view). It holds no rule of its own.

import CoxCore
import CoxUI

extension PointerRule {
  /// What the Rust core answers for each element, mapped to CoxUI's shapes.
  static let live = PointerRule { interaction in
    let answer = corePointer(
      clickable: interaction.clickable, selectable: interaction.selectable,
      pressed: interaction.pressed, blocked: interaction.blocked)
    return PointerShape(answer)
  }
}

extension PointerShape {
  init(_ answer: CorePointer) {
    switch answer {
    case .default: self = .default
    case .action: self = .action
    case .text: self = .text
    case .notAllowed: self = .notAllowed
    case .resize(.vertical): self = .resize(.vertical)
    case .resize(.horizontal): self = .resize(.horizontal)
    }
  }
}
