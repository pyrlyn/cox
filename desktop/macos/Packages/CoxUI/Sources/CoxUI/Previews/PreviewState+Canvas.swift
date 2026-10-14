// `PreviewState` fixtures for `PluginCanvasPane` (T33.47.8.4): Weft's design canvas with a
// phone and a tablet frame, and the same canvas beside a second plugin's, stopped and zoomed
// out, with each page drawn as SwiftUI where the app shows WebKit's view. Separate so the
// organism's fixtures do not edit the shared file.

import SwiftUI

extension PreviewState {
  static let canvasDesign = PluginCanvasPaneState.Canvas(
    id: "weft", title: "Design", symbol: "rectangle.on.rectangle")

  /// A phone's and a tablet's frame, scaled down to fit the sample beside each other.
  static let canvasPhoneWidth: CGFloat = 220
  static let canvasPhoneHeight: CGFloat = 300
  static let canvasTabletWidth: CGFloat = 300
  static let canvasTabletHeight: CGFloat = 240

  static let canvasFrames = [
    PluginCanvasPaneState.Frame(
      id: "home", title: "Home · iPhone", width: canvasPhoneWidth, height: canvasPhoneHeight),
    PluginCanvasPaneState.Frame(
      id: "cart", title: "Cart · iPad", width: canvasTabletWidth, height: canvasTabletHeight),
  ]

  static let canvasTwoFrames = PluginCanvasPaneState(
    canvases: [canvasDesign], shown: "weft", frames: canvasFrames)

  static let canvasPicker = PluginCanvasPaneState(
    canvases: [canvasDesign, .init(id: "notes", title: "Notes", symbol: "note.text")],
    shown: "weft", frames: canvasFrames, stopped: true, zoom: 0.75)
}

/// The pane at a width that holds both frames, a page of SwiftUI in WebKit's place.
struct PluginCanvasSample: View {
  let state: PluginCanvasPaneState

  static let width: CGFloat = 640
  static let height: CGFloat = 420

  var body: some View {
    PluginCanvasPane(state: state) { _ in
    } page: { frame in
      VStack(alignment: .leading, spacing: Space.m) {
        Text(frame.id.capitalized).textStyle(.titleCard)
        RoundedRectangle(cornerRadius: Radius.m).fill(Color(.fillPrimary))
          .frame(height: Size.toolbarHeight)
        Text("Continue")
      }
      .textStyle(.body)
      .foregroundStyle(Color(.textPrimary))
      .padding(Space.l)
      .frame(width: frame.width, height: frame.height, alignment: .topLeading)
      .scaleEffect(state.zoom, anchor: .topLeading)
      .background(Color(.surfaceWindow))
    }
    .frame(width: Self.width, height: Self.height)
  }
}
