# Написання плагіна cox на Rust

Це переклад [docs/plugins.md](../plugins.md). Авторитетною вважається англійська сторінка: якщо тексти розходяться, правильна саме вона.

Плагін cox — це модуль WebAssembly плюс маніфест `plugin.toml`. cox завантажує його через extism, видає лише ті можливості (capabilities), які схвалив користувач, і викликає знайдені експорти. Цей посібник описує сторону Rust: крейт `cox-plugin-sdk` у `plugins/sdk`. Дизайн, зокрема причину кожного правила, описано в `docs/design/plugins.md` (PL); ABI — у його §4.

Ця сторінка призначена для написання плагіна. Щоб установити й використовувати плагін, який уже постачається в цьому репозиторії, дивіться його власний посібник, наприклад `docs/plugins/cursor.md` для зовнішнього агента Cursor.

API складається із [спільної](#спільна-частина), [термінальної](#лише-для-термінала) та [десктопної](#лише-для-робочого-стола) частин. [Покажчик полів](#покажчик-полів-api) перелічує кожне поле маніфесту й ABI у своїй частині.

## Налаштування

Набір інструментів Rust і його ціль `wasm32-unknown-unknown` беруться з `mise.toml` репозиторію. `mise install` додасть ціль, якщо її немає.

```toml
# Cargo.toml
[package]
name = "turn-counter"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
# Not on crates.io yet: a path inside this repository, or
# { git = "https://github.com/pyrlyn/cox" } from outside it.
cox-plugin-sdk = { path = "../../sdk" }
```

```bash
mise exec -- cargo build --release --target wasm32-unknown-unknown
# → target/wasm32-unknown-unknown/release/turn_counter.wasm
```

Скопіюйте `.wasm` поруч із `plugin.toml` під іменем, яке задає його ключ `wasm`. Схема маніфесту — `docs/plugin.schema.json`; PL§2 показує кожен ключ.

## Експорти

Плагін — це набір функцій-обробників і один виклик `register!`. `init` обов'язковий і йде першим. Решта експортів необов'язкові й існують, лише якщо ви їх перелічили: cox шукає експорти за іменем і вважає відсутній «ненаданим».

```rust
use cox_plugin_sdk::*;

fn init(_: InitIn) -> Result<InitOut, SdkError> {
    Ok(InitOut {
        commands: vec![CommandDecl { name: "reset".into(), description: "Reset the counter".into() }],
        subscribe: vec!["turn_started".into()],
        ..InitOut::default()
    })
}

fn on_event(batch: EventBatch) -> Result<Effects, SdkError> {
    let turns = kv_get("turns")?.and_then(|v| v.as_u64()).unwrap_or(0);
    kv_put("turns", &Value::from(turns + batch.events.len() as u64))?;
    Ok(Effects { redraw: true, ..Effects::default() })
}

fn command(_: CommandIn) -> Result<CommandOut, SdkError> {
    kv_delete("turns")?;
    Ok(CommandOut::Nothing)
}

cox_plugin_sdk::register!(init => init, on_event => on_event, command => command);
```

Повна версія — `plugins/examples/rust`, еталонний плагін, який PL§13 вимагає реалізувати кожною мовою: він додає хук, що рахує збої, і сегмент статусу, а лічильники тримає і в пам'яті, і в kv, бо `cox_render` не може викликати `kv_get`.

Обробник повертає `Result<output, E>` для будь-якого типу помилки, що реалізує `Display`. `Err` завершує з помилкою лише цей виклик: cox показує повідомлення як попередження й продовжує роботу (розширення відмовляють м'яко, fail open).

| Ключ `register!` | Експорт | Вхід → вихід | Коли викликається |
| --- | --- | --- | --- |
| `init` | `cox_init` | `InitIn` → `InitOut` | один раз за сесію; `InitIn.config` — ваша таблиця `[plugins.<id>]` з конфігурації cox (`{}`, якщо її немає), яку ви перевіряєте самі; внески, які вам не видано, відкидаються зі сповіщенням |
| `on_event` | `cox_on_event` | `EventBatch` → `Effects` | для подій із `InitOut.subscribe` (PL§5) |
| `hook` | `cox_hook` | `HookCall` → `HookOutcome` як `Value` | для хуків із `capabilities.hooks` (PL§6) |
| `decide` | `cox_decide` | `Question` → `Advice` | у точках рішення з `capabilities.decide` (PL§6b) |
| `decide_call` | `cox_decide` | `Question` → `DecideOut` | замість `decide`, коли для відповіді потрібен виклик моделі через вашу секцію `[[provider]]` (T33.40.1) |
| `decide_resume` | `cox_decide_resume` | `DecideResume` → `Option<Advice>` | після цього виклику, з його подіями |
| `tool_subject`, `tool_risk`, `tool_call` | `cox_tool_*` | `ToolCallIn` → `String`, `Risk` / `ToolOutput` як `Value` | для інструментів із `InitOut.tools` (PL§7) |
| `provider_stream` | `cox_provider_stream` | `ProviderCall` → `Vec<ProviderEvent>` як `Vec<Value>` | для провайдера з `api = "plugin"` (PL§7a) |
| `command` | `cox_command` | `CommandIn` → `CommandOut` | `/<id>:<name>` на будь-якій поверхні (PL§8) |
| `key` | `cox_key` | `CommandIn` → `CommandOut` | лише лідерні клавіші термінала (PL§15.3) |
| `render` | `cox_render` | `RenderIn` → `Widget` | для сегмента статусу, панелі або оверлея (PL§8) |
| `render_item` | `cox_render_item` | `RenderItemIn` → `Option<Widget>` | для цілей із `ui.render` (PL§8) |
| `canvas` | `cox_canvas` | `CanvasIn` → `CanvasOut` | лише на робочому столі, поки показано панель полотна: під час її відкриття, за `Effects.redraw`, після завершення кожного з ваших інструментів і коли розмір панелі змінюється достатньо сильно; у межах `min(limits.call_ms, 2000 ms)` |
| `canvas_select` | `cox_canvas_select` | `CanvasSelect` → `Option<CanvasAttach>` | під час кліку в одному з ваших фреймів; у межах `min(limits.call_ms, 500 ms)` |
| `shutdown` | `cox_shutdown` | `()` → `()` | наприкінці сесії, за можливості |

Корисні навантаження, що несуть тип протоколу cox (`Event`, `HookOutcome`, `ToolOutput`, `ProviderEvent`, `Request`), — це `serde_json::Value`, бо SDK не залежить від хост-крейтів. Їхню JSON-форму описано в `docs/protocol.jsonschema`; усе інше — у `docs/plugin-abi.schema.json`.

## Хост-функції

Кожна обгортка повертає `SdkResult<T>`. `SdkError::Host(AbiError)` означає, що cox відмовив або виклик не вдався: можливість не видано, виклик не дозволено з поточного експорту, рушій дозволів чи бюджет сказав «ні», досягнуто ліміт або минув строк. Відмови — це значення, а не пастки (traps), тому плагін може деградувати замість того, щоб провалити весь виклик.

| Функція | Можливість | Не дозволена з | Повертає |
| --- | --- | --- | --- |
| `log(level, text)` | — | — | `()`; потрапляє в журнал cox, з обмеженням частоти |
| `notify(level, text)` | — | `render` | `()`; сповіщення в транскрипті, рівень не вищий за `Warn`, очищується (sanitized) |
| `kv_get(key)`, `kv_put(key, value)`, `kv_delete(key)` | `kv` | `render` | `Option<Value>` / `()`; у межах квоти сховища (64 КіБ на значення, 1 МіБ на плагін, ключі до 256 байтів) |
| `context()` | `context` | — | знімок сесії, згорнутий із подій: id сесії, cwd, рівень і модель, суми використання, останні 50 елементів з текстом, список завдань, ущільнення (compactions); текст, схожий на секрет, приховано |
| `invoke_tool(name, input)` | `invoke` містить `name` | усе, крім `on_event`, `command`, `key`, `tool_call` | `ToolOutput`; виклик проходить хуки, рушій дозволів і пісочницю так само, як виклик моделі |
| `model_call(&ModelCall)` | `model` | `render` | `Vec<ProviderEvent>`; на виданому рівні або нижче, з перевіркою бюджету, записується в облік витрат |
| `http(&HttpReq)` | `net` містить хост | `render` | `HttpResp`; хост із `[[provider]]` — лише з `provider_stream`, заголовок авторизації додає cox |
| `output(line)`, `cancelled()` | — | усе, крім `tool_call` | `()` / `bool`: прогрес інструмента й скасування |
| `redraw()` | будь-яка `ui` | — | `()`; позначає ваші слоти як такі, що потребують перемальовування |

`invoke_tool` відхиляється всередині `hook`, `decide` і `provider_stream`, бо цикл агента чекає на ці виклики, і запуск інструмента зсередини призвів би до взаємного блокування.

## Можливості рушія

cox запускає кожен плагін на одній конфігурації рушія; модуль, якому потрібне розширення WebAssembly поза нею, не завантажується з помилкою розбору.

| Можливість | Стан | Чому |
| --- | --- | --- |
| tail calls, function references, GC | увімкнено | extism 1.30.0 завжди вмикає їх |
| обробка винятків (`try_table`, `throw`, теги) | увімкнено | функція extism `wasmtime-exceptions`, увімкнена для всього workspace (plan.md A61): вивід Kotlin/Wasm використовує її безумовно й без неї не розбирається (research.md R§4.3.5 P42–P43) |
| WASI preview 1 | на плагін | підключається лише для маніфесту з `wasi = true` (гість `wasip1`, наприклад TinyGo) або за видачі `[capabilities.fs]`; без видачі немає preopens (T33.43, T33.14.2) |

Перемикач винятків діє на весь workspace: це один прапорець рушія, а не можливість окремого плагіна, тож будь-який плагін може використовувати винятки. Він додає лише інструкції, видимі гостю; хост, як і раніше, дає кожному плагіну власний рушій і сховище, той самий ліміт пам'яті та той самий строк на виклик. RUSTSEC-2026-0222 називає API вбудовування, пов'язані з винятками (`ExnRefPre::new`, `Tag::new`), але для нього хост має переносити об'єкти між рушіями, чого не роблять ні extism, ні cox, тож увімкнення розширення не робить вразливість досяжною з гостя (`deny.toml`). Підтримку обробки винятків у самому wasmtime 43 не перевіряли глибше за це попередження; перегляньте її під час оновлення T33.43.

## Протокол обміну для інших мов

SDK — тонкий шар над одним правилом, якому може наслідувати гість будь-якою мовою зі своїм extism PDK:

- Експорт читає вхід як одне значення JSON (порожній вхід означає `null`) і записує як вихід одне значення JSON. Збій установлює текст помилки extism і повертає 1.
- Хост-функція живе в модулі імпорту `cox:host/v1` і має сигнатуру `(u64) -> u64`: вона приймає зміщення блока пам'яті extism з одним значенням JSON і повертає зміщення виділеного нею блока. Аргумент — єдиний аргумент функції: об'єкт із ключами за іменами аргументів, якщо їх кілька (`{"level": "warn", "text": "…"}`, `{"key": "turns", "value": 3}`, `{"name": "read", "input": {…}}`), або `null`, якщо їх немає. Відповідь — `{"Ok": value}` або `{"Err": AbiError}`.

## Go

`plugins/sdk-go` (модуль `github.com/pyrlyn/cox/plugins/sdk-go`) — той самий API поверх `github.com/extism/go-pdk` v1.1.3. Go і TinyGo беруться з `plugins/mise.toml`. Плагін на Go — це модуль TinyGo `wasip1`, тому в його `plugin.toml` задано `wasi = true`:

```bash
tinygo build -o plugin.wasm -target wasip1 -buildmode=c-shared .
```

Кожен експорт — це функція `//go:wasmexport`, усе тіло якої — `cox.Export(handler)`, де обробник відображає вхідний тип експорту у вихідний і `error`. `cox_init` обов'язковий; решту cox шукає за іменем. Хост-функції: `cox.Log`, `Notify`, `KVGet`, `KVPut`, `KVDelete`, `Context`, `InvokeTool`, `ModelCall`, `HTTP`, `Output`, `Cancelled`, `Redraw` і `DesktopNotify`, з можливостями й контекстами з таблиці вище. Відмова — це `*cox.HostError`, чий `Kind` дорівнює тегу `AbiError`. У `types.go` лежать поширені навантаження; будь-яке інше несе `json.RawMessage`, а його форму описано в `docs/plugin-abi.schema.json`.

```go
//go:wasmexport cox_init
func coxInit() int32 {
	return cox.Export(func(cox.InitIn) (cox.InitOut, error) {
		return cox.InitOut{Subscribe: []string{"turn_started"}}, nil
	})
}
```

TinyGo — не gc: він обчислює `return n, json.Unmarshal(raw, &n)`, читаючи `n` до виклику, тому спершу візьміть помилку в змінну. Запускайте тести пакета і під `tinygo test`, і під `go test`.

Повна версія — `plugins/examples/go`; `cox plugin new <name> --lang go` створює заготовку пакета з `justfile` і димовим тестом. `just plugin-examples go` збирає приклад і запускає його наскрізні тести.

## Kotlin

`plugins/sdk-kotlin` — власний PDK cox поверх сирих імпортів extism `extism:host/env` і `cox:host/v1`, бо підтримуваного extism PDK для Kotlin немає. Його не опубліковано: плагін бере його через складену збірку Gradle (`includeBuild`) з каталогу `plugins/sdk-kotlin` у копії cox. Java, Gradle і Kotlin беруться з `plugins/mise.toml`. Плагін на Kotlin — це модуль Kotlin/Wasm `wasmWasi`, а стандартна бібліотека Kotlin імпортує WASI `random_get`, тому в його `plugin.toml` задано `wasi = true`.

Кожен експорт — це функція `@WasmExport`, усе тіло якої — `Cox.export { input -> … }`: обробник приймає вхід експорту як `JsonElement` і повертає вихід як один, з формами з `docs/plugin-abi.schema.json`. `cox_init` обов'язковий; решту cox шукає за іменем. Хост-функції: `Cox.log`, `notify`, `kvGet`, `kvPut`, `kvDelete`, `context`, `invokeTool`, `modelCall`, `http`, `output`, `cancelled` і `redraw`. Відмова кидає `HostException`, чий `error` дорівнює `AbiError`.

```kotlin
@WasmExport
fun cox_init(): Int = Cox.export {
    buildJsonObject { putJsonArray("subscribe") { add("turn_started") } }
}
```

Повна версія — `plugins/examples/kotlin`; `gradle pluginPackage` складає `plugin.toml` і модуль у `build/plugin` — це те, що приймає `cox plugin install`, бо він копіює цілу теку. `cox plugin new <name> --lang kotlin` створює заготовку пакета з `justfile`, чий рецепт `smoke` встановлює його в окремий `COX_HOME`. `just plugin-examples kotlin` збирає приклад і запускає його наскрізні тести.

## Спільна частина

Ідентичність пакета в маніфесті, ліміти ресурсів, провайдери, моделі, агенти, сервери MCP і спільні можливості діють скрізь, де завантажується плагін. Необов'язковий `surfaces` звужує завантаження до `terminal`, `desktop`, `headless` або `acp`; якщо його немає, дозволено всі чотири. `InitIn.surface` повідомляє гостю, яка з них відкрила сесію. Цю частину використовують `cox:host/v1`, експорти вище, крім `cox_key`, команди, рендерери та слоти `status.left`, `status.right`, `panel` і `overlay`. Сесії headless і ACP не мають інтерфейсу, тому вони пропускають видачі для UI. Записи `[[skills]]` (`name`, `dir`) постачають Agent Skills: `dir` — тека пакета, названа за іменем навички, з `SKILL.md` у форматі `.cox/skills`. Навички виданого плагіна входять в індекс навичок сесії, а навичка користувача чи проєкту з тим самим іменем має пріоритет. Пакету лише з `[[mcp]]`, `[[agents]]` і `[[skills]]` `wasm` не потрібен. `Span.link` приймає URL HTTPS або шлях відносно робочої області й відкривається лише за кліком людини; непідтримувані посилання лишаються текстом.

## Лише для термінала

`[capabilities.terminal] keys = true` видає прив'язки лідерних клавіш, оголошені в `InitOut.keys` (`KeyDecl`) і оброблювані `cox_key`. Старий запис `capabilities.ui.keys` — псевдонім в `api = 1`. `cox:tui/v1` зарезервовано, і наразі в ньому немає функцій. Сесія на робочому столі відкидає повернуті клавіші з одним інформаційним сповіщенням. `cox --plain` відповідає термінальній поверхні, але не малює слотів.

## Лише для робочого стола

`[capabilities.desktop]` приймає `inspector`, `toolbar`, `palette`, `notify` і `canvas`. `InitOut.desktop.inspector` оголошує один `TabDecl`; `cox_render` заповнює його слот `desktop.inspector`. `toolbar` і `palette` містять значення `ActionDecl`, чия `command` має називати оголошену команду й запускається через `cox_command`. `CommandOut.open_inspector` вибирає вкладку. `cox:desktop/v1` надає `cox_desktop_notify` з `DesktopNotice` і щонайбільше двома `NoticeAction`. `Widget.image` називає PNG відносно пакета (не більше 256 КіБ); термінал показує його текст `alt`. Десктопний виклик хоста на іншій поверхні повертає `AbiError.not_on_this_surface`. Внески, повернуті на недоступній поверхні, відкидаються з одним інформаційним сповіщенням, а плагін, виключений через `surfaces`, пропускається зі сповіщенням. Одна видача за дайджестом покриває всі поверхні.

### Полотно (canvas)

`canvas = true` додає поруч із транскриптом панель сторінок, які малюєте ви. `InitOut.desktop.canvas` оголошує один `CanvasDecl`: `title`, символ SF і `select_attr` — атрибут `data-` завдовжки 1–48 символів із `[a-z0-9-]`. `cox_canvas` повертає фрейми, кожен — статична сторінка HTML з `id`, `title` і `width_pt` (обмежується діапазоном 200–2560); панель показує їх поруч, кожен заввишки за вмістом, і людина може масштабувати. Видача запитує `desktop.canvas` окремим рядком, навіть якщо інші десктопні частини плагіна було видано раніше, бо панель показує власний HTML плагіна. Панель відкриває лише людина, через ⌘⇧D або пункт палітри «Show <title>»; ніщо з повернутого плагіном не може її відкрити.

Ліміти: не більше 12 фреймів, 1 МіБ HTML у фреймі та 4 МіБ за один виклик; `id` фрейма відповідає `^[a-z0-9_.-]{1,64}$` і є унікальним. Фрейм понад ліміт відкидається з попередженням. Три пропущені строки поспіль зупиняють полотно на сесію; останні фрейми лишаються. Обидва експорти виконуються в контексті рендеру: без запису в kv, без сповіщень, без інструментів. cox порівнює HTML кожного фрейма з попереднім і перезавантажує лише змінені фрейми.

Що сторінка може: компонувати себе за допомогою HTML, вбудованих `<style>` і атрибутів `style` та вбудованого SVG; слідувати світлому чи темному оформленню застосунку через `prefers-color-scheme`; позначати елементи, які можна вибрати кліком, через `select_attr`. Чого вона не може:

- виконувати скрипти: JavaScript сторінки вимкнено, тому `<script>` і атрибути `on…` нічого не роблять;
- завантажувати що-небудь: зображення, шрифти, таблиці стилів і фрейми заблоковано — з мережі, з URL `data:` і з власної схеми сторінки (список правил вмісту та Content-Security-Policy `default-src 'none'`);
- переходити за посиланнями: посилання й форми нічого не роблять;
- зберігати стан: кожен фрейм має непостійне сховище.

Клік запускає власний скрипт застосунку, що живе у світі вмісту, недоступному сторінці: він обводить найближчий елемент із `select_attr` і повідомляє його значення. cox відкидає значення довше за 256 символів або з керувальними символами, а тоді викликає `cox_canvas_select` з `id` фрейма та значенням. Повернутий `CanvasAttach` (`name` до 80 символів, `text` до 8 КіБ, обидва очищуються) стає чипом `text/plain` у полі вводу, який людина надсилає з наступним повідомленням або видаляє; наступний клік по тому самому полотну замінює його. `None` нічого не додає. Ніщо з полотна не потрапляє до моделі, доки людина не надішле це. Дизайн описано в `docs/design/plugin-canvas.md`.

## Хмарні агенти

Запис `[[cloud_agents]]` запускає фонове завдання на розміщеному хмарному агенті. Його веде хост: жоден експорт пакета для нього не викликається. Пакет усе одно називає файл `wasm`, як і `[[external_agents]]`. Єдиний бекенд — `cursor`, і хмарний плагін у цьому репозиторії не постачається; офлайн-тести використовують тестовий пакет.

```toml
# plugin.toml
api = 1
id = "cloud-bg"
version = "0.1.0"
name = "Cloud background"
wasm = "plugin.wasm"

[[cloud_agents]]
name = "cloud"                  # agent(preset: "cloud")
backend = "cursor"              # closed set; its host is fixed
key_env = "CURSOR_API_KEY"      # the name of an env var, never the key
model = "composer-2"            # optional; the backend's default when absent
description = "Long background runs on Cursor's servers"
```

Поля: `name` (ім'я диспетчеризації `agent(preset: "<name>")`), `backend`, `key_env` (типово `CURSOR_API_KEY`; значення відхиляється), а також необов'язкові `model` і `description` (показується під час схвалення). Поля URL немає, а невідомий ключ на кшталт `url` відхиляється, тож ключ не можна надіслати на інший сервер.

`cox plugin install` запитує одну видачу на запис. Рядок називає бекенд, хост, змінну ключа й модель:

```text
cloud-agent:cloud cursor host=api.cursor.com key=CURSOR_API_KEY model=composer-2; sends the prompt and the repository's GitHub URL; the service clones the repository and pushes a branch on its servers, billed to your plan
```

Змінений запис знову запитує видачу. Ключ читається за іменем із середовища, потім із системного в'язка ключів, як для будь-якого провайдера. Працює лише ключ, випущений у панелі керування.

### Що залишає машину

Кожен запуск надсилає лише таке:

- промпт, який модель передала в `agent(preset)`;
- URL репозиторію на GitHub, перебудований із власника та імені в `origin` (облікові дані, порт, шлях і запит у remote відкидаються); remote на будь-якому іншому хості відхиляється до будь-якого запиту;
- початковий ref: поточну гілку або коміт `HEAD` у разі від'єднаного HEAD;
- id моделі, якщо її задано;
- ключ, як bearer-токен, і лише на `api.cursor.com`.

cox не завантажує жодних інших файлів. Cursor клонує репозиторій із GitHub, тому коміти, яких немає в `origin`, не видно. Якщо локального `HEAD` немає в `origin` (за останнім fetch), відповідь починається з попередження. Запити не несуть git-імені й адреси електронної пошти. Код, який запуск читає й править, виконується на серверах Cursor, поза пісочницею cox та перевірками шляхів.

### Дозвіл

Кожен запуск потребує рішення allow для `CloudAgent(<owner>/<repo>)`. Додайте точний запис до власної конфігурації, наприклад `allow = ["CloudAgent(acme/widgets)"]` у `[permissions]`; імена порівнюються без урахування регістру. Голий allow `CloudAgent`, дозвіл на сесію чи власна конфігурація проєкту репозиторію не зараховуються. Без запису, за збігу з `deny` або в режимі плану запуск відхиляється до будь-якого запиту, а відмова називає правило, яке потрібно додати. Фонове завдання не має запиту на схвалення, тому питання там ніколи не ставиться.

### Запуск і завершення

`agent(preset: "<name>")` завжди працює у фоні, навіть без `background`. Запуск з'являється як фонове завдання, результат якого — відповідь запуску, без тексту `thinking`. Скасування завдання скасовує запуск на боці Cursor. Якщо cox закривається до закінчення запуску, запуск триває; під час відновлення сесії cox знову стежить за запуском або завершує його й додає результат до завдання.

Токени кожного запуску, як їх повідомляє Cursor, записуються в журнал використання за ціною $0 із встановленим `billed_externally`. cox їх не оцінює: витрата йде на ваш план Cursor.

### Обмеження

- Наступне повідомлення в запущений прогін не надсилається. Клієнт уміє його зібрати, але жоден драйвер ним не користується.
- Фонове завдання не може показати запит на схвалення. Натомість воно відхиляється із зазначенням правила, яке потрібно додати.
- Запуск, повторно підключений після відновлення, не можна скасувати.
- Сповіщення про завершення відхиленого запуску все ще може повідомляти, що субагент не дав відповіді. Помилка завдання несе причину відмови (T56.12 відкрита).
- Лише Cursor і лише репозиторії на GitHub.
- Клієнт протестовано офлайн на написаних вручну фікстурах. З живим обліковим записом Cursor його ще не перевіряли (T56.10).

<!-- plugin-api:start -->
## Покажчик полів API

Згенеровано зі схем [маніфесту](../plugin.schema.json) та [ABI](../plugin-abi.schema.json) скриптом `scripts/plugin_api_reference.py`. Кожен запис перелічує поля типу та варіанти перелічень; типи, на які є посилання, мають власні записи. Після навмисної зміни схеми виконайте `python3 scripts/plugin_api_reference.py --write` і перенесіть списки нижче в переклади `docs/ru` та `docs/uk` без змін. Ідентифікатори лишаються як на англійській сторінці.

### Спільна частина

- manifest `PluginManifest`: `api`, `surfaces`, `id`, `version`, `name`, `description`, `wasm`, `wasi`, `limits`, `capabilities`, `provider`, `models`, `mcp`, `language_servers`, `debug_adapters`, `external_agents`, `cloud_agents`, `agents`, `skills`
- manifest `Surface`: `terminal`, `desktop`, `headless`, `acp`
- manifest `Limits`: `memory_mib`, `call_ms`
- manifest `Capabilities`: `events`, `hooks`, `tools`, `invoke`, `context`, `kv`, `model`, `net`, `fs`, `decide`, `ui`, `ide`
- manifest `ModelTier`: `cheap`, `code`
- manifest `FsCaps`: `read`, `write`
- manifest `UiCaps`: `status`, `panel`, `overlay`, `commands`, `render`
- manifest `IdeCaps`: `buffer`, `lsp`, `dap`, `index`, `terminal`, `git`
- manifest `BufferAccess`: `read`, `edit`
- manifest `GitAccess`: `read`, `write`
- manifest `ProviderDecl`: `name`, `api`, `base_url`, `api_key_env`, `auth`
- manifest `ProviderApi`: `chat`, `responses`, `plugin`
- manifest `ProviderAuth`: `bearer`, `x-api-key`, `none`
- manifest `ModelDecl`: `id`, `provider`, `context_window`, `max_output`, `price`
- manifest `PriceDecl`: `input`, `output`, `cache_read`, `cache_write`
- manifest `McpDecl`: `name`, `command`, `args`, `url`
- manifest `LanguageServerDecl`: `name`, `languages`, `globs`, `command`, `args`, `env`, `config`
- manifest `DebugAdapterDecl`: `name`, `languages`, `command`, `args`, `env`, `config`
- manifest `ExternalAgentDecl`: `name`, `command`, `args`, `mode`, `key_env`
- manifest `AgentMode`: `acp`, `stream-json`
- manifest `CloudAgentDecl`: `name`, `backend`, `key_env`, `model`, `description`
- manifest `CloudBackend`: `cursor`
- manifest `AgentDecl`: `name`, `file`
- manifest `SkillDecl`: `name`, `dir`
- ABI `InitIn`: `api`, `plugin_id`, `config`, `session`, `granted`, `surface`
- ABI `SessionInfo`: `id`, `cwd`
- ABI `Surface`: `terminal`, `desktop`, `headless`, `acp`
- ABI `InitOut`: `tools`, `commands`, `status`, `panels`, `renderers`, `subscribe`
- ABI `CommandDecl`: `name`, `description`
- ABI `Slot`: `status.left`, `status.right`, `panel`, `overlay`
- ABI `CanvasDecl`: `title`, `symbol`, `select_attr`
- ABI `EventBatch`: `first_seq`, `dropped`, `events`
- ABI `Effects`: `redraw`, `notices`
- ABI `PluginNotice`: `level`, `text`
- ABI `NoticeLevel`: `info`, `warn`
- ABI `HookCall`: `event`, `payload`
- ABI `ToolCallIn`: `name`, `input`
- ABI `CommandIn`: `name`, `args`
- ABI `CommandOut`: `prompt`, `prompt.text`, `prompt.kind`, `compact`, `compact.focus`, `compact.kind`, `toggle_panel`, `toggle_panel.kind`, `open_overlay`, `open_overlay.kind`, `notice`, `notice.kind`, `nothing`, `nothing.kind`
- ABI `RenderIn`: `slot`, `width`, `height`
- ABI `RenderItemIn`: `target`, `call`, `result`, `width`
- ABI `ProviderCall`: `request`, `model`, `base_url`
- ABI `ModelCall`: `request`, `tier`, `own_provider`
- ABI `ModelTier`: `cheap`, `code`
- ABI `OwnProvider`: `name`, `model`
- ABI `HttpReq`: `method`, `url`, `headers`, `body`
- ABI `HttpResp`: `status`, `headers`, `body`
- ABI `Question`: `point`, `state`, `options`, `items`
- ABI `DecidePoint`: `route`, `risk`, `approve_hint`, `compact`, `rank`, `salience`
- ABI `QuestionItem`: `state`, `options`
- ABI `Advice`: `answer`, `confidence`, `note`
- ABI `Answer`: `choice`, `choice.order`, `choice.kind`, `score`, `score.value`, `score.kind`, `noul`, `noul.p_yes`, `noul.kind`, `scores`, `scores.values`, `scores.kind`, `items`, `items.answers`, `items.kind`
- ABI `DecideOut`: `call`
- ABI `DecideResume`: `question`, `events`
- ABI `AbiError`: `not_in_this_context`, `not_in_this_context.kind`, `not_on_this_surface`, `not_on_this_surface.surface`, `not_on_this_surface.kind`, `not_granted`, `not_granted.capability`, `not_granted.kind`, `denied`, `denied.reason`, `denied.kind`, `budget`, `budget.kind`, `too_large`, `too_large.limit`, `too_large.kind`, `timeout`, `timeout.kind`, `transient`, `transient.message`, `transient.kind`, `failed`, `failed.message`, `failed.kind`
- ABI `Widget`: `text`, `list`, `list.items`, `list.selected`, `table`, `table.header`, `table.rows`, `table.widths`, `key_value`, `gauge`, `gauge.ratio`, `gauge.label`, `stack`, `stack.vertical`, `stack.children`, `stack.sizes`, `block`, `block.title`, `block.child`
- ABI `Span`: `text`, `style`, `bold`, `italic`, `link`
- ABI `StyleToken`: `text`, `dim`, `accent`, `user`, `agent`, `tool`, `ok`, `warn`, `error`, `diff_add`, `diff_del`, `diff_hunk`, `border`, `selection`
- ABI `CanvasIn`: `width_pt`, `height_pt`
- ABI `CanvasOut`: `frames`
- ABI `Frame`: `id`, `title`, `width_pt`, `html`
- ABI `CanvasSelect`: `frame`, `value`
- ABI `CanvasAttach`: `name`, `text`

### Лише для термінала

- manifest `Capabilities`: `terminal`
- manifest `UiCaps`: `keys`
- manifest `TerminalCaps`: `keys`
- ABI `InitOut`: `keys`
- ABI `KeyDecl`: `key`, `name`, `description`

### Лише для робочого стола

- manifest `Capabilities`: `desktop`
- manifest `DesktopCaps`: `inspector`, `toolbar`, `palette`, `notify`, `canvas`
- ABI `InitOut`: `desktop`
- ABI `Slot`: `desktop.inspector`
- ABI `DesktopOut`: `inspector`, `toolbar`, `palette`, `canvas`
- ABI `TabDecl`: `title`, `symbol`
- ABI `ActionDecl`: `command`, `title`, `symbol`
- ABI `CommandOut`: `open_inspector`, `open_inspector.kind`
- ABI `Widget`: `image`, `image.path`, `image.alt`
- ABI `DesktopNotice`: `level`, `title`, `body`, `actions`
- ABI `NoticeAction`: `label`, `command`
<!-- plugin-api:end -->
