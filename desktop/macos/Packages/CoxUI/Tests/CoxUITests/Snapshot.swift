// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The one snapshot harness every CoxUI suite shares (DS§9): a sample drawn over a colourful
// backdrop in one light/dark × Solid/Frosted cell, through a window-hosted `NSHostingView`
// (`ImageRenderer` drops glass content) into a bitmap of a fixed 2× scale, so the image does
// not depend on the machine's display. Separate so no suite carries its own renderer.

import AppKit
import SnapshotTesting
import SwiftUI
import Testing

@testable import CoxUI

/// One cell of the snapshot matrix.
struct Variant: CustomTestStringConvertible, Sendable {
  let scheme: ColorScheme
  let material: GlassMaterial

  static let all = [ColorScheme.light, .dark].flatMap { scheme in
    [GlassMaterial.solid, .frosted].map { Variant(scheme: scheme, material: $0) }
  }

  var name: String { "\(scheme == .dark ? "dark" : "light")-\(material.rawValue)" }
  var testDescription: String { name }
}

/// A sample hosted in a borderless window, re-rendered as its root view changes.
@MainActor
struct SnapshotHost<Sample: View> {
  private let settings: Settings
  private let host: NSHostingView<AnyView>
  private let window: NSWindow

  init(
    _ sample: Sample, _ variant: Variant, reduceMotion: Bool = false,
    reduceTransparency: Bool = false
  ) {
    settings = Settings(
      variant: variant, reduceMotion: reduceMotion, reduceTransparency: reduceTransparency)
    host = NSHostingView(rootView: Self.dressed(sample, settings))
    host.appearance = NSAppearance(named: variant.scheme == .dark ? .darkAqua : .aqua)
    host.frame = CGRect(origin: .zero, size: host.fittingSize)
    window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.contentView = host
    host.layoutSubtreeIfNeeded()
  }

  /// Swaps in `sample`, keeping the view's identity so its animations run.
  func update(_ sample: Sample) {
    host.rootView = Self.dressed(sample, settings)
    host.layoutSubtreeIfNeeded()
  }

  /// The current frame as a bitmap of a fixed 2× scale.
  func bitmap() throws -> NSBitmapImageRep {
    let bitmap = try #require(
      NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: Int(host.bounds.width) * 2,
        pixelsHigh: Int(host.bounds.height) * 2, bitsPerSample: 8, samplesPerPixel: 4,
        hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0,
        bitsPerPixel: 0))
    bitmap.size = host.bounds.size
    host.cacheDisplay(in: host.bounds, to: bitmap)
    return bitmap
  }

  /// The first frame for which `done` holds, rendered as the main run loop turns. The wait
  /// ends on what is drawn, not on a clock, so load can slow a test but not change the frame
  /// it reads. `limit` only bounds a wait that would never end: then the last frame comes back
  /// and the caller's expectation fails on it.
  func bitmap(
    until done: (NSBitmapImageRep) throws -> Bool, limit: TimeInterval = 10
  ) throws -> NSBitmapImageRep {
    let deadline = Date().addingTimeInterval(limit)
    var frame = try bitmap()
    while try !done(frame), Date() < deadline {
      RunLoop.main.run(until: Date().addingTimeInterval(Motion.durationFast))
      frame = try bitmap()
    }
    return frame
  }

  /// The current frame as an image, for `assertSnapshot`.
  func image() throws -> NSImage {
    let bitmap = try bitmap()
    let image = NSImage(size: bitmap.size)
    image.addRepresentation(bitmap)
    return image
  }

  private struct Settings {
    let variant: Variant
    let reduceMotion: Bool
    let reduceTransparency: Bool
  }

  private static func dressed(_ sample: Sample, _ settings: Settings) -> AnyView {
    AnyView(
      sample
        .padding(Space.xxl)
        .background(PreviewBackdrop())
        .environment(\.colorScheme, settings.variant.scheme)
        .environment(\.coxAppearance, Appearance(material: settings.variant.material))
        .environment(\._accessibilityReduceMotion, settings.reduceMotion)
        .environment(\._accessibilityReduceTransparency, settings.reduceTransparency))
  }
}

/// Asserts `sample` in `variant` against the image `name` of the calling test, stored beside
/// the calling test file.
@MainActor
func assertCoxSnapshot(
  _ sample: some View, _ variant: Variant, reduceTransparency: Bool = false,
  named name: String, testName: String = #function, filePath: StaticString = #filePath,
  fileID: StaticString = #fileID, line: UInt = #line
) throws {
  let image = try SnapshotHost(sample, variant, reduceTransparency: reduceTransparency).image()
  // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
  assertSnapshot(
    of: image, as: .image(precision: 0.995, perceptualPrecision: 0.98), named: name,
    fileID: fileID, file: filePath, testName: testName, line: line)
}

/// Asserts `screen` at `size` in `variant` as a whole window at 1× — its structure is the check,
/// and the organisms' own snapshots hold the detail at 2× — so the committed images stay a few
/// hundred kilobytes each.
@MainActor
func assertCoxWindowSnapshot(
  _ screen: some View, _ variant: Variant, size: CGSize = PreviewState.window,
  reduceTransparency: Bool = false, named name: String? = nil, testName: String = #function,
  filePath: StaticString = #filePath, fileID: StaticString = #fileID, line: UInt = #line
) throws {
  let sample = screen.frame(width: size.width, height: size.height)
  let full = try SnapshotHost(sample, variant, reduceTransparency: reduceTransparency).bitmap()
  let rep = try #require(
    NSBitmapImageRep(
      bitmapDataPlanes: nil, pixelsWide: Int(full.size.width), pixelsHigh: Int(full.size.height),
      bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
      colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
  rep.size = full.size
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
  full.draw(in: CGRect(origin: .zero, size: full.size))
  NSGraphicsContext.restoreGraphicsState()
  let image = NSImage(size: rep.size)
  image.addRepresentation(rep)
  assertSnapshot(
    of: image, as: .image(precision: 0.995, perceptualPrecision: 0.98),
    named: name ?? variant.name,
    fileID: fileID, file: filePath, testName: testName, line: line)
}
