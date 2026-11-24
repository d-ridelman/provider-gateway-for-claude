# ADR-001. Свой профиль Claude через подмену LOCALAPPDATA

- Status: Accepted
- Date: 2026-11-19 (записано 2026-11-24)

## Context
Режим 3p Claude Desktop хранит настройку в `%LOCALAPPDATA%\Claude-3p` (`configLibrary`).
Подписанные сборки Claude не слушают `CLAUDE_USER_DATA_DIR`. У владельца рядом работает
обычный Claude с аккаунтом Anthropic, его трогать нельзя.

## Decision
PGC запускает Claude с `LOCALAPPDATA=%LOCALAPPDATA%\ClaudeProviderGateway` и
`--user-data-dir` своего профиля. Запись `configLibrary` и `deploymentMode: "3p"` пишутся
только туда; в `%APPDATA%\Claude` PGC не пишет.

## Consequences
- Обычный Claude и PGC живут рядом и не мешают друг другу.
- Процессы, которые запускает Claude, наследуют подменённую `LOCALAPPDATA` — это решает
  ADR-002.
- Открытый Claude не подхватывает смену порта и моделей: окно просит переоткрыть его.
