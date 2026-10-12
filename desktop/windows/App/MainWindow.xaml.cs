// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;

namespace Cox.App;

/// <summary>
/// Shell from the desktop mockup: sidebar, title, composer, inspector.
/// Stores and the live session wait on the C# bindings (T58.1) and T58.5.
/// </summary>
public sealed partial class MainWindow : Window
{
    private const int PreferredWidth = 1440;
    private const int PreferredHeight = 900;

    public MainWindow()
    {
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(TitleBar);
        ApplyBackdrop();
        ApplyFrame();
    }

    /// <summary>
    /// Mica on Windows 11 outside High Contrast. Anywhere else the window stays solid (R10.3.6).
    /// The accessibility and backdrop APIs throw on hosts that lack them; that is the fallback.
    /// </summary>
    private void ApplyBackdrop()
    {
        try
        {
            var contrast = new Windows.UI.ViewManagement.AccessibilitySettings().HighContrast;
            if (!contrast && Environment.OSVersion.Version.Build >= 22000)
            {
                SystemBackdrop = new MicaBackdrop();
            }
        }
        catch (Exception)
        {
            SystemBackdrop = null;
        }
    }

    /// <summary>1440×900 clamped to the work area (A126).</summary>
    private void ApplyFrame()
    {
        var work = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary).WorkArea;
        AppWindow.Resize(new SizeInt32(Math.Min(PreferredWidth, work.Width), Math.Min(PreferredHeight, work.Height)));
    }
}
