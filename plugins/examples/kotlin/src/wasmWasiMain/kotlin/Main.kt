// The reference plugin PL§13 asks every guest language to implement, in
// Kotlin over cox's PDK (plugins/sdk-kotlin), matching plugins/examples/rust:
// it counts turns through a `turn_started` subscription, counts failed tool
// calls in a `PostToolUse`/`PostToolUseFailure` hook, shows both in a status
// segment and clears them with `/example-kotlin:reset`. Both counters persist
// in the plugin's kv store, mirrored in memory for `cox_render`, which may
// not touch kv (PL§4).
import cox.pdk.Cox
import cox.pdk.Level
import kotlin.wasm.WasmExport
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray

private const val TURNS = "turns"
private const val FAILURES = "failures"

private var turns = 0L
private var failures = 0L

private fun stored(key: String): Long = Cox.kvGet(key)?.jsonPrimitive?.longOrNull ?: 0L

private fun field(json: JsonElement, name: String): JsonElement? = (json as? JsonObject)?.get(name)

@WasmExport
fun cox_init(): Int = Cox.export {
    turns = stored(TURNS)
    failures = stored(FAILURES)
    buildJsonObject {
        putJsonArray("commands") {
            addJsonObject {
                put("name", "reset")
                put("description", "Reset the turn and failure counters")
            }
        }
        putJsonArray("status") { add(JsonPrimitive("status.right")) }
        putJsonArray("subscribe") { add(JsonPrimitive("turn_started")) }
    }
}

// The host sends only subscribed kinds, but a newer host may widen a batch,
// so the tag is checked here too.
@WasmExport
fun cox_on_event(): Int = Cox.export { batch ->
    val started = field(batch, "events")?.jsonArray
        ?.count { field(it, "type")?.jsonPrimitive?.contentOrNull == "turn_started" } ?: 0
    if (started > 0) {
        Cox.kvPut(TURNS, JsonPrimitive(turns + started))
        turns += started
    }
    buildJsonObject { put("redraw", started > 0) }
}

@WasmExport
fun cox_hook(): Int = Cox.export { call ->
    val response = field(call, "payload")?.let { field(it, "tool_response") }
    if (response?.let { field(it, "is_error") }?.jsonPrimitive?.booleanOrNull == true) {
        Cox.kvPut(FAILURES, JsonPrimitive(failures + 1))
        failures += 1
        // A refusal (the notice queue is full) only loses the notice; the
        // count is already stored, so the hook still succeeds.
        runCatching { Cox.notify(Level.INFO, "failed tool calls: $failures") }
    }
    // Informational: a post-tool hook never changes the call.
    buildJsonObject { put("type", "continue") }
}

@WasmExport
fun cox_command(): Int = Cox.export { command ->
    if (field(command, "name")?.jsonPrimitive?.contentOrNull != "reset") {
        buildJsonObject { put("kind", "nothing") }
    } else {
        Cox.kvDelete(TURNS)
        Cox.kvDelete(FAILURES)
        turns = 0
        failures = 0
        buildJsonObject {
            put("kind", "notice")
            put("level", Level.INFO.wire)
            put("text", "counters reset")
        }
    }
}

// Dim when nothing failed, a warning colour otherwise.
@WasmExport
fun cox_render(): Int = Cox.export {
    val span = buildJsonObject {
        put("text", "turns $turns · failed tools $failures")
        put("style", if (failures == 0L) "dim" else "warn")
    }
    buildJsonObject { put("text", buildJsonArray { add(buildJsonArray { add(span) }) }) }
}
