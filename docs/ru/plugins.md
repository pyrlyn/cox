# Написание плагина cox на Rust

Это перевод [docs/plugins.md](../plugins.md). Авторитетной считается английская страница: если тексты расходятся, верна она.

Плагин cox — это модуль WebAssembly плюс манифест `plugin.toml`. cox загружает его через extism, выдаёт только те возможности (capabilities), которые одобрил пользователь, и вызывает найденные экспорты. Это руководство описывает сторону Rust: крейт `cox-plugin-sdk` в `plugins/sdk`. Дизайн, включая причину каждого правила, описан в `docs/design/plugins.md` (PL); ABI — в его §4.

Эта страница нужна для написания плагина. Чтобы установить и использовать плагин, который уже поставляется в этом репозитории, см. его собственное руководство, например `docs/plugins/cursor.md` для внешнего агента Cursor.

API состоит из [общей](#общая-часть), [терминальной](#только-для-терминала) и [десктопной](#только-для-рабочего-стола) частей. [Указатель полей](#указатель-полей-api) перечисляет каждое поле манифеста и ABI в своей части.

## Настройка

Набор инструментов Rust и его цель `wasm32-unknown-unknown` берутся из `mise.toml` репозитория. `mise install` добавит цель, если её нет.

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

Скопируйте `.wasm` рядом с `plugin.toml` под именем, которое задаёт его ключ `wasm`. Схема манифеста — `docs/plugin.schema.json`; PL§2 показывает каждый ключ.

## Экспорты

Плагин — это набор функций-обработчиков и один вызов `register!`. `init` обязателен и идёт первым. Остальные экспорты необязательны и существуют, только если вы их перечислили: cox ищет экспорты по имени и считает отсутствующий «не предоставленным».

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

Полная версия — `plugins/examples/rust`, эталонный плагин, который PL§13 требует реализовать на каждом языке: он добавляет хук, считающий сбои, и сегмент статуса, а счётчики держит и в памяти, и в kv, потому что `cox_render` не может вызывать `kv_get`.

Обработчик возвращает `Result<output, E>` для любого типа ошибки, реализующего `Display`. `Err` завершает с ошибкой только этот вызов: cox показывает сообщение как предупреждение и продолжает работу (расширения отказывают мягко, fail open).

| Ключ `register!` | Экспорт | Вход → выход | Когда вызывается |
| --- | --- | --- | --- |
| `init` | `cox_init` | `InitIn` → `InitOut` | один раз за сессию; `InitIn.config` — ваша таблица `[plugins.<id>]` из конфигурации cox (`{}`, если её нет), которую вы проверяете сами; вклады, которые вам не выданы, отбрасываются с уведомлением |
| `on_event` | `cox_on_event` | `EventBatch` → `Effects` | для событий из `InitOut.subscribe` (PL§5) |
| `hook` | `cox_hook` | `HookCall` → `HookOutcome` как `Value` | для хуков из `capabilities.hooks` (PL§6) |
| `decide` | `cox_decide` | `Question` → `Advice` | в точках решения из `capabilities.decide` (PL§6b) |
| `decide_call` | `cox_decide` | `Question` → `DecideOut` | вместо `decide`, когда для ответа нужен вызов модели через вашу секцию `[[provider]]` (T33.40.1) |
| `decide_resume` | `cox_decide_resume` | `DecideResume` → `Option<Advice>` | после этого вызова, с его событиями |
| `tool_subject`, `tool_risk`, `tool_call` | `cox_tool_*` | `ToolCallIn` → `String`, `Risk` / `ToolOutput` как `Value` | для инструментов из `InitOut.tools` (PL§7) |
| `provider_stream` | `cox_provider_stream` | `ProviderCall` → `Vec<ProviderEvent>` как `Vec<Value>` | для провайдера с `api = "plugin"` (PL§7a) |
| `command` | `cox_command` | `CommandIn` → `CommandOut` | `/<id>:<name>` на любой поверхности (PL§8) |
| `key` | `cox_key` | `CommandIn` → `CommandOut` | только лидерные клавиши терминала (PL§15.3) |
| `render` | `cox_render` | `RenderIn` → `Widget` | для сегмента статуса, панели или оверлея (PL§8) |
| `render_item` | `cox_render_item` | `RenderItemIn` → `Option<Widget>` | для целей из `ui.render` (PL§8) |
| `canvas` | `cox_canvas` | `CanvasIn` → `CanvasOut` | только на рабочем столе, пока показана панель холста: при её открытии, при `Effects.redraw`, после завершения каждого из ваших инструментов и когда размер панели меняется достаточно сильно; в пределах `min(limits.call_ms, 2000 ms)` |
| `canvas_select` | `cox_canvas_select` | `CanvasSelect` → `Option<CanvasAttach>` | при клике в одном из ваших фреймов; в пределах `min(limits.call_ms, 500 ms)` |
| `shutdown` | `cox_shutdown` | `()` → `()` | при завершении сессии, по возможности |

Полезные нагрузки, несущие тип протокола cox (`Event`, `HookOutcome`, `ToolOutput`, `ProviderEvent`, `Request`), — это `serde_json::Value`, потому что SDK не зависит от хост-крейтов. Их JSON-форма описана в `docs/protocol.jsonschema`; всё остальное — в `docs/plugin-abi.schema.json`.

## Хост-функции

Каждая обёртка возвращает `SdkResult<T>`. `SdkError::Host(AbiError)` означает, что cox отказал или вызов не удался: возможность не выдана, вызов не разрешён из текущего экспорта, движок разрешений или бюджет сказал «нет», достигнут лимит либо истёк срок. Отказы — это значения, а не ловушки (traps), поэтому плагин может деградировать вместо того, чтобы провалить весь вызов.

| Функция | Возможность | Не разрешена из | Возвращает |
| --- | --- | --- | --- |
| `log(level, text)` | — | — | `()`; попадает в журнал cox, с ограничением частоты |
| `notify(level, text)` | — | `render` | `()`; уведомление в транскрипте, уровень не выше `Warn`, очищается (sanitized) |
| `kv_get(key)`, `kv_put(key, value)`, `kv_delete(key)` | `kv` | `render` | `Option<Value>` / `()`; в пределах квоты хранилища (64 КиБ на значение, 1 МиБ на плагин, ключи до 256 байт) |
| `context()` | `context` | — | снимок сессии, свёрнутый из событий: id сессии, cwd, уровень и модель, суммы использования, последние 50 элементов с текстом, список задач, уплотнения (compactions); текст, похожий на секрет, скрыт |
| `invoke_tool(name, input)` | `invoke` содержит `name` | всё, кроме `on_event`, `command`, `key`, `tool_call` | `ToolOutput`; вызов проходит хуки, движок разрешений и песочницу так же, как вызов модели |
| `model_call(&ModelCall)` | `model` | `render` | `Vec<ProviderEvent>`; на выданном уровне или ниже, с проверкой бюджета, записывается в учёт расходов |
| `http(&HttpReq)` | `net` содержит хост | `render` | `HttpResp`; хост из `[[provider]]` — только из `provider_stream`, заголовок авторизации добавляет cox |
| `output(line)`, `cancelled()` | — | всё, кроме `tool_call` | `()` / `bool`: прогресс инструмента и отмена |
| `redraw()` | любая `ui` | — | `()`; помечает ваши слоты как требующие перерисовки |

`invoke_tool` отклоняется внутри `hook`, `decide` и `provider_stream`, потому что цикл агента ждёт эти вызовы, и запуск инструмента изнутри привёл бы к взаимной блокировке.

## Возможности движка

cox запускает каждый плагин на одной конфигурации движка; модуль, которому нужно расширение WebAssembly вне её, не загружается с ошибкой разбора.

| Возможность | Состояние | Почему |
| --- | --- | --- |
| tail calls, function references, GC | включено | extism 1.30.0 всегда включает их |
| обработка исключений (`try_table`, `throw`, теги) | включено | функция extism `wasmtime-exceptions`, включённая для всего workspace (plan.md A61): вывод Kotlin/Wasm использует её безусловно и без неё не разбирается (research.md R§4.3.5 P42–P43) |
| WASI preview 1 | на плагин | подключается только для манифеста с `wasi = true` (гость `wasip1`, например TinyGo) или при выдаче `[capabilities.fs]`; без выдачи нет preopens (T33.43, T33.14.2) |

Переключатель исключений действует на весь workspace: это один флаг движка, а не возможность отдельного плагина, так что любой плагин может использовать исключения. Он добавляет только инструкции, видимые гостю; хост по-прежнему даёт каждому плагину собственный движок и хранилище, тот же лимит памяти и тот же срок на вызов. RUSTSEC-2026-0222 называет API встраивания, связанные с исключениями (`ExnRefPre::new`, `Tag::new`), но для него хост должен переносить объекты между движками, чего не делают ни extism, ни cox, поэтому включение расширения не делает уязвимость достижимой из гостя (`deny.toml`). Поддержка обработки исключений в самом wasmtime 43 не проверялась дальше этого предупреждения; пересмотрите её при обновлении T33.43.

## Протокол обмена для других языков

SDK — тонкий слой над одним правилом, которому может следовать гость на любом языке со своим extism PDK:

- Экспорт читает вход как одно значение JSON (пустой вход означает `null`) и записывает как выход одно значение JSON. Сбой устанавливает текст ошибки extism и возвращает 1.
- Хост-функция живёт в модуле импорта `cox:host/v1` и имеет сигнатуру `(u64) -> u64`: она принимает смещение блока памяти extism с одним значением JSON и возвращает смещение выделенного ею блока. Аргумент — единственный аргумент функции: объект с ключами по именам аргументов, если их несколько (`{"level": "warn", "text": "…"}`, `{"key": "turns", "value": 3}`, `{"name": "read", "input": {…}}`), или `null`, если их нет. Ответ — `{"Ok": value}` или `{"Err": AbiError}`.

## Go

`plugins/sdk-go` (модуль `github.com/pyrlyn/cox/plugins/sdk-go`) — тот же API поверх `github.com/extism/go-pdk` v1.1.3. Go и TinyGo берутся из `plugins/mise.toml`. Плагин на Go — это модуль TinyGo `wasip1`, поэтому в его `plugin.toml` задано `wasi = true`:

```bash
tinygo build -o plugin.wasm -target wasip1 -buildmode=c-shared .
```

Каждый экспорт — это функция `//go:wasmexport`, всё тело которой — `cox.Export(handler)`, где обработчик отображает входной тип экспорта в выходной и `error`. `cox_init` обязателен; остальное cox ищет по имени. Хост-функции: `cox.Log`, `Notify`, `KVGet`, `KVPut`, `KVDelete`, `Context`, `InvokeTool`, `ModelCall`, `HTTP`, `Output`, `Cancelled`, `Redraw` и `DesktopNotify`, с возможностями и контекстами из таблицы выше. Отказ — это `*cox.HostError`, чей `Kind` равен тегу `AbiError`. В `types.go` лежат распространённые нагрузки; любую другую несёт `json.RawMessage`, а её форма описана в `docs/plugin-abi.schema.json`.

```go
//go:wasmexport cox_init
func coxInit() int32 {
	return cox.Export(func(cox.InitIn) (cox.InitOut, error) {
		return cox.InitOut{Subscribe: []string{"turn_started"}}, nil
	})
}
```

TinyGo — не gc: он вычисляет `return n, json.Unmarshal(raw, &n)`, читая `n` до вызова, поэтому сначала возьмите ошибку в переменную. Запускайте тесты пакета и под `tinygo test`, и под `go test`.

Полная версия — `plugins/examples/go`; `cox plugin new <name> --lang go` создаёт заготовку пакета с `justfile` и дымовым тестом. `just plugin-examples go` собирает пример и запускает его сквозные тесты.

## Общая часть

Идентичность пакета в манифесте, лимиты ресурсов, провайдеры, модели, агенты, серверы MCP и общие возможности действуют везде, где загружается плагин. Необязательный `surfaces` сужает загрузку до `terminal`, `desktop`, `headless` или `acp`; если его нет, разрешены все четыре. `InitIn.surface` сообщает гостю, какая из них открыла сессию. Эту часть используют `cox:host/v1`, экспорты выше, кроме `cox_key`, команды, рендереры и слоты `status.left`, `status.right`, `panel` и `overlay`. У сессий headless и ACP нет интерфейса, поэтому они опускают выдачи для UI. Записи `[[skills]]` (`name`, `dir`) поставляют Agent Skills: `dir` — папка пакета, названная по имени навыка, с `SKILL.md` в формате `.cox/skills`. Навыки выданного плагина входят в индекс навыков сессии, а навык пользователя или проекта с тем же именем имеет приоритет. Пакету только из `[[mcp]]`, `[[agents]]` и `[[skills]]` `wasm` не нужен. `Span.link` принимает URL HTTPS или путь относительно рабочей области и открывается только по клику человека; неподдерживаемые ссылки остаются текстом.

## Только для терминала

`[capabilities.terminal] keys = true` выдаёт привязки лидерных клавиш, объявленные в `InitOut.keys` (`KeyDecl`) и обрабатываемые `cox_key`. Старая запись `capabilities.ui.keys` — псевдоним в `api = 1`. `cox:tui/v1` зарезервирован и пока не содержит функций. Сессия на рабочем столе отбрасывает возвращённые клавиши с одним информационным уведомлением. `cox --plain` соответствует терминальной поверхности, но не рисует слоты.

## Только для рабочего стола

`[capabilities.desktop]` принимает `inspector`, `toolbar`, `palette`, `notify` и `canvas`. `InitOut.desktop.inspector` объявляет один `TabDecl`; `cox_render` заполняет его слот `desktop.inspector`. `toolbar` и `palette` содержат значения `ActionDecl`, чья `command` должна называть объявленную команду и запускается через `cox_command`. `CommandOut.open_inspector` выбирает вкладку. `cox:desktop/v1` предоставляет `cox_desktop_notify` с `DesktopNotice` и не более чем двумя `NoticeAction`. `Widget.image` называет PNG относительно пакета (не более 256 КиБ); терминал показывает его текст `alt`. Десктопный вызов хоста на другой поверхности возвращает `AbiError.not_on_this_surface`. Вклады, возвращённые на недоступной поверхности, отбрасываются с одним информационным уведомлением, а плагин, исключённый через `surfaces`, пропускается с уведомлением. Одна выдача по дайджесту покрывает все поверхности.

### Холст (canvas)

`canvas = true` добавляет рядом с транскриптом панель страниц, которые рисуете вы. `InitOut.desktop.canvas` объявляет один `CanvasDecl`: `title`, символ SF и `select_attr` — атрибут `data-` длиной 1–48 символов из `[a-z0-9-]`. `cox_canvas` возвращает фреймы, каждый — статическая страница HTML с `id`, `title` и `width_pt` (ограничивается диапазоном 200–2560); панель показывает их рядом, каждый высотой по содержимому, и человек может масштабировать. Выдача запрашивает `desktop.canvas` отдельной строкой, даже если остальные десктопные части плагина были выданы раньше, потому что панель показывает собственный HTML плагина. Панель открывает только человек, через ⌘⇧D или пункт палитры «Show <title>»; ничто из возвращённого плагином не может её открыть.

Лимиты: не более 12 фреймов, 1 МиБ HTML во фрейме и 4 МиБ за один вызов; `id` фрейма соответствует `^[a-z0-9_.-]{1,64}$` и уникален. Фрейм сверх лимита отбрасывается с предупреждением. Три пропущенных срока подряд останавливают холст на сессию; последние фреймы остаются. Оба экспорта выполняются в контексте рендера: без записи в kv, без уведомлений, без инструментов. cox сравнивает HTML каждого фрейма с предыдущим и перезагружает только изменившиеся фреймы.

Что страница может: компоновать себя с помощью HTML, встроенных `<style>` и атрибутов `style` и встроенного SVG; следовать светлому или тёмному оформлению приложения через `prefers-color-scheme`; помечать элементы, которые можно выбрать кликом, через `select_attr`. Чего она не может:

- выполнять скрипты: JavaScript страницы выключен, поэтому `<script>` и атрибуты `on…` ничего не делают;
- загружать что-либо: изображения, шрифты, таблицы стилей и фреймы заблокированы — из сети, из URL `data:` и из собственной схемы страницы (список правил содержимого и Content-Security-Policy `default-src 'none'`);
- переходить по ссылкам: ссылки и формы ничего не делают;
- хранить состояние: у каждого фрейма непостоянное хранилище.

Клик запускает собственный скрипт приложения, живущий в мире содержимого, недоступном странице: он обводит ближайший элемент с `select_attr` и сообщает его значение. cox отбрасывает значение длиннее 256 символов или с управляющими символами, затем вызывает `cox_canvas_select` с `id` фрейма и значением. Возвращённый `CanvasAttach` (`name` до 80 символов, `text` до 8 КиБ, оба очищаются) становится чипом `text/plain` в поле ввода, который человек отправляет со следующим сообщением или удаляет; следующий клик по тому же холсту заменяет его. `None` ничего не добавляет. Ничто с холста не попадает к модели, пока человек не отправит это. Дизайн описан в `docs/design/plugin-canvas.md`.

## Облачные агенты

Запись `[[cloud_agents]]` запускает фоновую задачу на размещённом облачном агенте. Её ведёт хост: ни один экспорт пакета для неё не вызывается. Пакет всё равно называет файл `wasm`, как и `[[external_agents]]`. Единственный бэкенд — `cursor`, и облачный плагин в этом репозитории не поставляется; офлайн-тесты используют тестовый пакет.

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

Поля: `name` (имя диспетчеризации `agent(preset: "<name>")`), `backend`, `key_env` (по умолчанию `CURSOR_API_KEY`; значение отклоняется), а также необязательные `model` и `description` (показывается при одобрении). Поля URL нет, а неизвестный ключ вроде `url` отклоняется, так что ключ нельзя отправить на другой сервер.

`cox plugin install` запрашивает одну выдачу на запись. Строка называет бэкенд, хост, переменную ключа и модель:

```text
cloud-agent:cloud cursor host=api.cursor.com key=CURSOR_API_KEY model=composer-2; sends the prompt and the repository's GitHub URL; the service clones the repository and pushes a branch on its servers, billed to your plan
```

Изменённая запись снова запрашивает выдачу. Ключ читается по имени из окружения, затем из системной связки ключей, как для любого провайдера. Работает только ключ, выпущенный в панели управления.

### Что покидает машину

Каждый запуск отправляет только следующее:

- промпт, который модель передала в `agent(preset)`;
- URL репозитория на GitHub, пересобранный из владельца и имени в `origin` (учётные данные, порт, путь и запрос в remote отбрасываются); remote на любом другом хосте отклоняется до всякого запроса;
- начальный ref: текущую ветку или коммит `HEAD` при отсоединённом HEAD;
- id модели, если она задана;
- ключ, как bearer-токен, и только на `api.cursor.com`.

cox не загружает никаких других файлов. Cursor клонирует репозиторий с GitHub, поэтому коммиты, которых нет в `origin`, не видны. Если локальный `HEAD` отсутствует в `origin` (судя по последнему fetch), ответ начинается с предупреждения. Запросы не несут git-имени и адреса электронной почты. Код, который запуск читает и правит, выполняется на серверах Cursor, вне песочницы cox и проверок путей.

### Разрешение

Каждый запуск требует решения allow для `CloudAgent(<owner>/<repo>)`. Добавьте точную запись в собственную конфигурацию, например `allow = ["CloudAgent(acme/widgets)"]` в `[permissions]`; имена сравниваются без учёта регистра. Голый allow `CloudAgent`, разрешение на сессию или собственная конфигурация проекта репозитория не считаются. Без записи, при совпавшем `deny` или в режиме плана запуск отклоняется до всякого запроса, а отказ называет правило, которое нужно добавить. У фоновой задачи нет запроса на одобрение, поэтому вопрос там никогда не задаётся.

### Запуск и завершение

`agent(preset: "<name>")` всегда работает в фоне, даже без `background`. Запуск появляется как фоновая задача, результат которой — ответ запуска, без текста `thinking`. Отмена задачи отменяет запуск на стороне Cursor. Если cox закрывается до окончания запуска, запуск продолжается; при возобновлении сессии cox снова следит за запуском либо завершает его и добавляет результат в задачу.

Токены каждого запуска, как их сообщает Cursor, записываются в журнал использования по цене $0 с установленным `billed_externally`. cox их не оценивает: расход идёт на ваш план Cursor.

### Ограничения

- Последующее сообщение в запущенный прогон не отправляется. Клиент умеет его собрать, но ни один драйвер им не пользуется.
- Фоновая задача не может показать запрос на одобрение. Вместо него она отклоняется с указанием правила, которое нужно добавить.
- Запуск, повторно подключённый после возобновления, нельзя отменить.
- Уведомление о завершении отклонённого запуска всё ещё может сообщать, что субагент не дал ответа. Ошибка задачи несёт причину отказа (T56.12 открыта).
- Только Cursor и только репозитории на GitHub.
- Клиент протестирован офлайн на написанных вручную фикстурах. С живой учётной записью Cursor он пока не проверялся (T56.10).

<!-- plugin-api:start -->
## Указатель полей API

Сгенерирован из схем [манифеста](../plugin.schema.json) и [ABI](../plugin-abi.schema.json) скриптом `scripts/plugin_api_reference.py`. Каждая запись перечисляет поля типа и варианты перечислений; типы, на которые есть ссылки, имеют собственные записи. После намеренного изменения схемы выполните `python3 scripts/plugin_api_reference.py --write` и перенесите списки ниже в переводы `docs/ru` и `docs/uk` без изменений. Идентификаторы остаются как на английской странице.

### Общая часть

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

### Только для терминала

- manifest `Capabilities`: `terminal`
- manifest `UiCaps`: `keys`
- manifest `TerminalCaps`: `keys`
- ABI `InitOut`: `keys`
- ABI `KeyDecl`: `key`, `name`, `description`

### Только для рабочего стола

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
