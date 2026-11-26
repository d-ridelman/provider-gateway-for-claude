# ADR-020. Окно PGC открывается в Chat, а не в Code

- Status: Accepted
- Date: 2026-11-26
- Supersedes: —

## Context

В 3P Chat и Cowork — одна поверхность (Unified Claude), Code — отдельная. Стартовую
поверхность решает главный процесс Claude: `preferences.sidebarMode` в
`claude_desktop_config.json` профиля. Значение `code` или `epitaxy` открывает Code; всё
остальное, включая отсутствие ключа, — `/new`, то есть Chat.

PGC писал `deploymentMode: "3p"` и сохранял `preferences` как есть. После одного клика по
Code в окне в профиле оставался `sidebarMode: "epitaxy"`, и каждый следующий запуск из PGC
открывался в Code. Пользователь видел только Code и считал, что Chat не работает.

При этом Chat в 3P не требует ни MSIX, ни VM: `chatIn3p` объявлен supported,
`keepsSessionCliOnHost()` включается провайдером custom-3p, а проверки `Exi()`/HCS/VMP
касаются только Cowork и advanced file analysis (документация Anthropic и разбор кода —
`research_notes/Chat в Claude Desktop при 3P/`, отчёт `reports/Chat в Claude Desktop при 3P.md`).
Переключатель «Чат / Cowork» в композере появляется, когда Cowork доступен устройству; на
копии PGC это дают патчи `app.asar` из ADR-018.

## Decision

При записи `claude_desktop_config.json` PGC пишет `deploymentMode: "3p"` и, если в
`preferences.sidebarMode` стоит поверхность Code (`code` или `epitaxy`), меняет её на `chat`.
Явно выбранный Chat и незнакомые значения не трогаются. Остальные ключи `preferences`
сохраняются, как и раньше.

## Consequences

- Окно из PGC открывается там, где человек работал в обычном Claude: Chat и Cowork, а не
  вкладка Code. Code остаётся доступен кнопкой и при следующем запуске снова станет Chat.
- Это осознанная перезапись пользовательского выбора поверхности: человек, который держит
  Code открытым, каждый раз получает Chat. Альтернатива — открывать в Code тех, кто в нём
  был, — оставляет исходную жалобу «Chat не работает».
- `desktopHome` по-прежнему `standard` (Chat + Cowork) или `no-vm` (только Chat) — см. ADR-018.
- Видимость переключателя «Чат / Cowork» зависит от того, считает ли приложение Cowork
  доступным; на копии PGC это решают патчи ADR-018, на настоящем exe из MSIX — сама служба.
- Проверено вживую 26.11.2026 на изолированном PGC (Claude 6.28899.6, профиль с
  `sidebarMode: "epitaxy"`): PGC записал `chat`, окно открылось на Chat («You're here!»).
  Переключателя «Чат / Cowork» в композере нет: он принадлежит новому интерфейсу, который в 3P
  выключен флагом `__gb__2436273311` (hub look / web delivery), а в статической карте 3P этого
  флага нет. Включение проверено вживую: с пропатченной картой новый интерфейс поднимается, но
  падает — 3P-заглушка `/api/bootstrap` отвечает 503 (`RangeError`), окно показывает «Claude is
  temporarily unavailable». Значит, флага мало: нужна поддержка нового bootstrap в стабе 3P, и
  это отдельная задача. Патч в код не взят.
