// A Kotlin/Wasm `wasmWasi` library: the target whose only WASI import is
// the stdlib's `random_get` unless the program does I/O (research.md
// R§4.3.5 P41, P45). `group` and the root project name are what
// `includeBuild` substitutes for `io.github.listepo.cox:sdk-kotlin`.
plugins {
    kotlin("multiplatform") version "2.4.20"
}

group = "io.github.listepo.cox"
version = "0.1.0"

kotlin {
    @OptIn(org.jetbrains.kotlin.gradle.ExperimentalWasmDsl::class)
    wasmWasi {}

    sourceSets {
        wasmWasiMain.dependencies {
            // `api`: payloads cross the PDK as `JsonElement`s.
            api("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
        }
    }

    compilerOptions {
        optIn.add("kotlin.wasm.ExperimentalWasmInterop")
    }
}
