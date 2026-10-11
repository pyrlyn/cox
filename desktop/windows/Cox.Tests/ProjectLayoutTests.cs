// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Xml.Linq;

namespace Cox.Tests;

[TestClass]
public sealed class ProjectLayoutTests
{
    [TestMethod]
    public void CoxModel_has_no_reference_to_WindowsAppSDK()
    {
        var project = XDocument.Load(ProjectFile("Cox.Model", "Cox.Model.csproj"));
        var includes = project
            .Descendants()
            .Where(element => element.Name.LocalName is "PackageReference" or "Reference" or "ProjectReference")
            .Select(element => (string?)element.Attribute("Include") ?? "");

        Assert.IsFalse(
            includes.Any(include =>
                include.Contains("Microsoft.WindowsAppSDK", StringComparison.OrdinalIgnoreCase)
                || include.Contains("Microsoft.Windows.SDK", StringComparison.OrdinalIgnoreCase)));

        var framework = project.Descendants().Single(element => element.Name.LocalName == "TargetFramework");
        Assert.AreEqual("net10.0", framework.Value);

        var useWinUi = project.Descendants().SingleOrDefault(element => element.Name.LocalName == "UseWinUI");
        Assert.IsNull(useWinUi);
    }

    private static string ProjectFile(string project, string file)
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null)
        {
            var candidate = Path.Combine(dir.FullName, project, file);
            if (File.Exists(candidate))
            {
                return candidate;
            }

            dir = dir.Parent;
        }

        Assert.Fail($"could not find {project}/{file} above {AppContext.BaseDirectory}");
        return "";
    }
}
