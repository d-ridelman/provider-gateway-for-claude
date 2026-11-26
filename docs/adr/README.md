# Журнал решений (ADR)

Решение записывается **до** изменения кода, если оно затрагивает больше одного модуля, меняет
контракт (с Claude, провайдером, пользователем) или идёт против уже записанного. Молча
обойти записанное нельзя — только новым ADR.

Старые записи не переписываются: новое решение ставит у старого `Status: Superseded by ADR-NNN`.

## Шаблон

```markdown
# ADR-NNN. Короткое название

- Status: Proposed | Accepted | Superseded by ADR-NNN
- Date: ГГГГ-ММ-ДД
- Supersedes: ADR-NNN (если есть)

## Context
Что происходит и почему нужно решать. Факты со ссылками (код, документация, issue).

## Decision
Что делаем. Одним абзацем, без вариантов.

## Consequences
Что станет лучше, что хуже, чем платим, что проверять.
```

## Записи

| № | Решение | Статус |
|---|---|---|
| [001](001-own-claude-profile.md) | Свой профиль Claude через подмену `LOCALAPPDATA`, обычный Claude не трогаем | Accepted |
| [002](002-claude-copy-with-env-prelude.md) | Claude открывается из копии со вставкой, перевод — по желанию | Accepted |
| [003](003-speak-anthropic.md) | Шлюз отвечает языком Anthropic: ошибки, поток, заголовки | Accepted |
| [004](004-one-main-model.md) | Одна основная модель для Claude Code; роли и советник убраны | Accepted |
| [005](005-claude-like-aliases.md) | Псевдонимы моделей `claude-…` и `behavesAs` для effort | Accepted |
| [006](006-mihomo-external-core.md) | VPN — внешнее ядро mihomo, качается по запросу | Accepted |
| [007](007-tool-search.md) | Поиск инструментов: отложенные инструменты не уходят провайдеру | Accepted |
| [008](008-no-desktop-user-agent-upstream.md) | Провайдеру Claude Code представляется как `claude -p` | Accepted |
| [009](009-catalog-translation-per-version.md) | Перевод меню усилий собирается под каждую версию Claude | Accepted |
| [010](010-code-model-left-in-user-settings.md) | Модель шлюза не остаётся в настройках Claude Code | Accepted |
| [011](011-one-menu-row-per-1m-model.md) | Одна строка в меню Claude на модель с окном 1M | Accepted |
| [012](012-critic-model-for-scripts.md) | Модель-критик для скриптов: переменные `AGENT_KIT_PGC_*` | Accepted |
| [013](013-agent-kit-in-pgc.md) | Проверки и критик PGC через agent-kit | Accepted |
| [014](014-connect-project.md) | Кнопка «Подключить проект»: agent-kit без ручных команд | Accepted |
| [015](015-agent-kit-in-the-installer.md) | agent-kit приезжает внутри PGC | Accepted |
| [016](016-smart-setup.md) | Проверки проекта отбирает код; шаг с моделью по замеру не включён | Accepted |
| [017](017-disconnect-project.md) | Одна страница «Проекты»: подключить, посмотреть, отключить с планом | Accepted |
| [018](018-cowork-on-host-without-msix.md) | Cowork в 3p: host-loop без MSIX; VM — отдельным режимом | Accepted |
| [019](019-compact-window.md) | Окно сжатия: одна настройка PGC для Desktop и консоли | Accepted |
| [020](020-chat-surface-first.md) | Окно PGC открывается в Chat, а не в Code | Accepted |
| [021](021-msix-as-claude-source.md) | Копия Claude собирается из MSIX-пакета, когда Squirrel-папки нет | Accepted |
| [022](022-claude-signature-root-pin.md) | Подпись скачиваемого Claude: CN и отпечаток удостоверяющего центра | Accepted |
