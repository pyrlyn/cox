// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `KeyValueGrid` (DS§6.3 row `KeyValueGrid`, the mockup's `.tokpop .grid` and inspector `.kv`):
// labelled figures in columns — the token popover's turn and session totals, the inspector's
// facts. Separate so every table of figures aligns its numbers the same way.

import SwiftUI

/// Labels on the left, one right-aligned tabular column per value, under optional uppercase
/// column headers; a detail row is indented and quieter. Without headers and with at most one
/// value a row, the grid reads as key–value pairs instead, each value after its key.
public struct KeyValueGrid: View {
  /// One line of the grid: the label and a value per column, formatted by the core.
  public struct Row: Equatable, Sendable {
    public var label: String
    public var values: [String]
    /// A breakdown of the row above it (`cache read` under `Sent`).
    public var isDetail = false

    public init(label: String, values: [String], isDetail: Bool = false) {
      (self.label, self.values, self.isDetail) = (label, values, isDetail)
    }
  }

  /// The value columns' headers, `Turn` and `Session`; empty for no header row.
  let columns: [String]
  let rows: [Row]

  init(columns: [String] = [], rows: [Row]) {
    self.columns = columns
    self.rows = rows
  }

  public var body: some View {
    if columns.isEmpty && rows.allSatisfy({ $0.values.count <= 1 }) { pairs } else { table }
  }

  /// The mockup's inspector `.kv`: quiet keys in a column as wide as the longest, each value
  /// starting right after it and wrapping; a detail row without a value spans both columns so a
  /// long path under a key does not widen the key column.
  private var pairs: some View {
    Grid(
      alignment: .leadingFirstTextBaseline, horizontalSpacing: Space.l, verticalSpacing: Space.s
    ) {
      ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
        let label = Text(row.label)
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
          .padding(.leading, row.isDetail ? Space.ml : 0)
        GridRow {
          if let value = row.values.first {
            label
            Text(value)
              .textStyle(.caption)
              .foregroundStyle(Color(row.isDetail ? .textSecondary : .textPrimary))
              .frame(maxWidth: .infinity, alignment: .leading)
              .fixedSize(horizontal: false, vertical: true)
          } else {
            label.gridCellColumns(2)
          }
        }
      }
    }
  }

  private var table: some View {
    // Styles go on the cells: a modifier on a `GridRow` would turn it into one spanning view.
    Grid(alignment: .trailing, horizontalSpacing: Space.l, verticalSpacing: Space.xs) {
      if !columns.isEmpty {
        GridRow {
          Color.clear.gridCellUnsizedAxes([.horizontal, .vertical])
          ForEach(Array(columns.enumerated()), id: \.offset) { _, header in
            Text(header)
              .textStyle(.micro)
              .textCase(.uppercase)
              .foregroundStyle(Color(.textTertiary))
          }
        }
      }
      ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
        let colour = Color(row.isDetail ? .textSecondary : .textPrimary)
        GridRow {
          Text(row.label)
            .textStyle(.caption)
            .foregroundStyle(colour)
            .padding(.leading, row.isDetail ? Space.ml : 0)
            .frame(maxWidth: .infinity, alignment: .leading)
          ForEach(Array(row.values.enumerated()), id: \.offset) { _, value in
            Text(value).textStyle(.caption, tabularDigits: true).foregroundStyle(colour)
          }
        }
      }
    }
  }
}

#Preview("columns") {
  PreviewMatrix {
    KeyValueGrid(columns: PreviewState.tokenColumns, rows: PreviewState.tokenRows)
      .frame(width: Size.tokenPopoverWidth)
  }
}
#Preview("pairs") {
  PreviewMatrix {
    KeyValueGrid(rows: PreviewState.factRows).frame(width: Size.inspectorWidth)
  }
}
