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
| [008](008-no-desktop-user-agent-upstream.md) | Провайдеру не уходит user-agent Claude Desktop | Accepted |
