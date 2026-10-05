// `gradle pluginPackage` compiles the module and stages the installable
// package (plugin.toml plus the `.wasm` its `wasm` key names) in
// build/plugin, which `cox plugin install` and the e2e test take.
// Only the unoptimized compile task runs: the `…Optimize` variant would
// download Binaryen, and the host needs nothing it adds.
plugins {
    kotlin("multiplatform") version "2.4.20"
}

kotlin {
    @OptIn(org.jetbrains.kotlin.gradle.ExperimentalWasmDsl::class)
    wasmWasi {
        binaries.executable()
    }

    sourceSets {
        wasmWasiMain.dependencies {
            implementation("io.github.listepo.cox:sdk-kotlin:0.1.0")
        }
    }

    compilerOptions {
        optIn.add("kotlin.wasm.ExperimentalWasmInterop")
    }
}

tasks.register<Copy>("pluginPackage") {
    val compile = tasks.named("compileProductionExecutableKotlinWasmWasi")
    dependsOn(compile)
    from(layout.projectDirectory.file("plugin.toml"))
    from(layout.buildDirectory.dir("compileSync/wasmWasi/main/productionExecutable/kotlin")) {
        include("${project.name}.wasm")
        rename { "example_kotlin.wasm" }
    }
    into(layout.buildDirectory.dir("plugin"))
}
