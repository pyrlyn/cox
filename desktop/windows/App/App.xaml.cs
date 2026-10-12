// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.UI.Xaml;

namespace Cox.App;

/// <summary>
/// WinUI 3 entry. The window shows the mockup shell; live sessions wait on T58.1.
/// </summary>
public partial class App : Application
{
    private Window? window;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        window = new MainWindow();
        window.Activate();
    }
}
