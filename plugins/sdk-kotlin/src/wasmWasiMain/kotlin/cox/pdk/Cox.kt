// cox's Kotlin PDK (T33.36): the wire from docs/plugins.md "The wire, for
// other languages" over extism's raw `extism:host/env` kernel imports and
// cox's `cox:host/v1` host functions. cox owns it because no maintained
// Kotlin extism PDK exists (research.md R§4.3.5 P30). Payloads stay
// `JsonElement`s: their shapes are docs/plugin-abi.schema.json, and typed
// classes would need the serialization compiler plugin in every plugin build.
//
// Nothing here may reach WASI (println, clocks, random): cox runs plugins
// with WASI off (plan.md A55), so a WASI import fails the module's load.
package cox.pdk

import kotlin.wasm.WasmImport
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.booleanOrNull

// extism's kernel memory: blocks live outside the guest's own memory, so
// every byte crosses through these calls. A Rust `u8` is a wasm `i32`.
@WasmImport("extism:host/env", "input_length") private external fun inputLength(): Long
@WasmImport("extism:host/env", "input_load_u8") private external fun inputLoadU8(offs: Long): Int
@WasmImport("extism:host/env", "input_load_u64") private external fun inputLoadU64(offs: Long): Long
@WasmImport("extism:host/env", "length") private external fun length(offs: Long): Long
@WasmImport("extism:host/env", "alloc") private external fun alloc(n: Long): Long
@WasmImport("extism:host/env", "free") private external fun free(offs: Long)
@WasmImport("extism:host/env", "output_set") private external fun outputSet(offs: Long, n: Long)
@WasmImport("extism:host/env", "error_set") private external fun errorSet(offs: Long)
@WasmImport("extism:host/env", "load_u8") private external fun loadU8(offs: Long): Int
@WasmImport("extism:host/env", "load_u64") private external fun loadU64(offs: Long): Long
@WasmImport("extism:host/env", "store_u8") private external fun storeU8(offs: Long, v: Int)
@WasmImport("extism:host/env", "store_u64") private external fun storeU64(offs: Long, v: Long)

// cox:host/v1: each takes a block holding one JSON value and returns a block
// holding `{"Ok": value}` or `{"Err": AbiError}`.
@WasmImport("cox:host/v1", "cox_log") private external fun coxLog(args: Long): Long
@WasmImport("cox:host/v1", "cox_notify") private external fun coxNotify(args: Long): Long
@WasmImport("cox:host/v1", "cox_kv_get") private external fun coxKvGet(args: Long): Long
@WasmImport("cox:host/v1", "cox_kv_put") private external fun coxKvPut(args: Long): Long
@WasmImport("cox:host/v1", "cox_kv_delete") private external fun coxKvDelete(args: Long): Long
@WasmImport("cox:host/v1", "cox_context") private external fun coxContext(args: Long): Long
@WasmImport("cox:host/v1", "cox_invoke_tool") private external fun coxInvokeTool(args: Long): Long
@WasmImport("cox:host/v1", "cox_model_call") private external fun coxModelCall(args: Long): Long
@WasmImport("cox:host/v1", "cox_http") private external fun coxHttp(args: Long): Long
@WasmImport("cox:host/v1", "cox_output") private external fun coxOutput(args: Long): Long
@WasmImport("cox:host/v1", "cox_cancelled") private external fun coxCancelled(args: Long): Long
@WasmImport("cox:host/v1", "cox_redraw") private external fun coxRedraw(args: Long): Long

/** cox refused or failed a host call (PL§4); [error] is the `AbiError`. */
class HostException(val error: JsonElement) : Exception("cox host call failed: $error")

/** `NoticeLevel`: a plugin may not raise anything above `warn`. */
enum class Level(val wire: String) { INFO("info"), WARN("warn") }

/** The PDK: [export] for the body of each `@WasmExport`, the rest wrap `cox:host/v1`. */
object Cox {
    /**
     * Runs one export: reads its JSON input (`null` when empty), passes it
     * to [handler] and sets the result as the output. A throw becomes the
     * call's error text and return code 1, which the host shows as a
     * warning; the plugin keeps running.
     */
    fun export(handler: (JsonElement) -> JsonElement): Int = try {
        val n = inputLength()
        val input = if (n == 0L) JsonNull else parse(read(n, ::inputLoadU64, ::inputLoadU8))
        val out = block(handler(input).toString().encodeToByteArray())
        outputSet(out, length(out))
        0
    } catch (e: Throwable) {
        errorSet(block((e.message ?: e.toString()).encodeToByteArray()))
        1
    }

    fun log(level: Level, text: String) { call(::coxLog, notice(level, text)) }
    fun notify(level: Level, text: String) { call(::coxNotify, notice(level, text)) }
    /** Capability `kv`; not from `cox_render`. */
    fun kvGet(key: String): JsonElement? = call(::coxKvGet, JsonPrimitive(key)).takeUnless { it is JsonNull }
    fun kvPut(key: String, value: JsonElement) {
        call(::coxKvPut, buildJsonObject { put("key", JsonPrimitive(key)); put("value", value) })
    }
    fun kvDelete(key: String) { call(::coxKvDelete, JsonPrimitive(key)) }
    /** Capability `context`: the host's event-folded session snapshot. */
    fun context(): JsonElement = call(::coxContext, JsonNull)
    /** Capability `invoke`: a `ToolOutput`, after hooks, permissions and the sandbox. */
    fun invokeTool(name: String, input: JsonElement): JsonElement =
        call(::coxInvokeTool, buildJsonObject { put("name", JsonPrimitive(name)); put("input", input) })
    /** Capability `model`: a `ModelCall` in, the `ProviderEvent` array out. */
    fun modelCall(call: JsonElement): JsonElement = call(::coxModelCall, call)
    /** Capability `net`: an `HttpReq` in, an `HttpResp` out. */
    fun http(request: JsonElement): JsonElement = call(::coxHttp, request)
    /** Inside `cox_tool_call` only. */
    fun output(line: String) { call(::coxOutput, JsonPrimitive(line)) }
    fun cancelled(): Boolean = call(::coxCancelled, JsonNull).jsonPrimitive.booleanOrNull ?: false
    fun redraw() { call(::coxRedraw, JsonNull) }

    private fun notice(level: Level, text: String) =
        buildJsonObject { put("level", JsonPrimitive(level.wire)); put("text", JsonPrimitive(text)) }

    private fun call(import: (Long) -> Long, arg: JsonElement): JsonElement {
        val argBlock = block(arg.toString().encodeToByteArray())
        val reply = import(argBlock)
        free(argBlock)
        if (reply == 0L) throw IllegalStateException("cox host call returned no block")
        val bytes = read(length(reply), { loadU64(reply + it) }, { loadU8(reply + it) })
        free(reply)
        val envelope = parse(bytes) as? JsonObject
            ?: throw IllegalStateException("cox host reply is not an Ok/Err object")
        envelope["Err"]?.let { throw HostException(it) }
        return envelope["Ok"] ?: throw IllegalStateException("cox host reply has neither Ok nor Err")
    }
}

private fun parse(bytes: ByteArray): JsonElement = Json.parseToJsonElement(bytes.decodeToString())

/** Reads `n` bytes eight at a time, then the tail byte by byte (little endian). */
private fun read(n: Long, u64: (Long) -> Long, u8: (Long) -> Int): ByteArray {
    val out = ByteArray(n.toInt())
    var i = 0
    while (i + 8 <= out.size) {
        val word = u64(i.toLong())
        for (k in 0 until 8) out[i + k] = (word ushr (8 * k)).toByte()
        i += 8
    }
    while (i < out.size) {
        out[i] = u8(i.toLong()).toByte()
        i++
    }
    return out
}

/** Allocates an extism block holding [bytes]; the caller or the host frees it. */
private fun block(bytes: ByteArray): Long {
    val offs = alloc(bytes.size.toLong())
    var i = 0
    while (i + 8 <= bytes.size) {
        var word = 0L
        for (k in 0 until 8) word = word or ((bytes[i + k].toLong() and 0xFF) shl (8 * k))
        storeU64(offs + i, word)
        i += 8
    }
    while (i < bytes.size) {
        storeU8(offs + i, bytes[i].toInt() and 0xFF)
        i++
    }
    return offs
}
