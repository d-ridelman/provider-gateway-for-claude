# ADR-002. Claude открывается из копии со вставкой, перевод — по желанию

- Status: Accepted
- Date: 2026-11-24

## Context
Из-за ADR-001 сессии Code, их npm, uv, gh, git и MCP-серверы писали кеши в папку PGC
(на диске владельца: `uv`, `GitHub CLI`, `claude-cli-nodejs` в `ClaudeProviderGateway`).
Вернуть им настоящую папку может только вставка в главный процесс Claude
(`ruui::ENV_PRELUDE`), а она возможна только в копии с изменённым `app.asar`. Раньше копия
собиралась лишь для русского интерфейса.

## Decision
Claude всегда открывается из копии `%LOCALAPPDATA%\ClaudeProviderGateway\claude-ru\app-X`:
с русским переводом или без него. Копия без перевода меняет только `app.asar` (и
`claude.exe` под его новый хеш); если вставка в эту версию Claude не легла, открывается
оригинал и окно говорит об этом. Русская копия без вставки всё равно открывается — ради перевода.

## Consequences
- Процессы, которые Claude запускает через `child_process`, видят настоящую `AppData\Local`.
  Процессы `utilityProcess` (MCP на встроенном Node, терминал, MCP-хост 3p) — нет
  (`shortcut` в `ruui::ENV_PRELUDE`).
- ~700 МБ на диске и сборка копии в фоне после каждого обновления Claude.
- Копия сама не обновляет Claude (в `main.log` профиля PGC: `[updater] App is not
  installed, not enabling auto-updates`): новые версии ставит обычный Claude.
- Копия пишет свой путь в реестр (`claude://`): `repair_registry` возвращает ключи после
  старта, самоперезапуска и выхода.
