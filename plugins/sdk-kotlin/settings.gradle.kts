// The cox Kotlin PDK (T33.36) is a Gradle build of its own, so a plugin
// consumes it with `includeBuild` instead of a published artifact: like
// cox-plugin-sdk, it is not published yet (PL§14 decision 1).
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

rootProject.name = "sdk-kotlin"
