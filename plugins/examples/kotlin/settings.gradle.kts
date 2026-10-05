// The PL§13 example in Kotlin (T33.36). The PDK comes from this repository
// through a composite build; a plugin outside it points `includeBuild` at a
// cox checkout's `plugins/sdk-kotlin` the same way.
pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenCentral()
    }
}

rootProject.name = "example-kotlin"
includeBuild("../../sdk-kotlin")
