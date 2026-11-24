# ADR-008. Провайдеру не уходит user-agent Claude Desktop

- Status: Accepted
- Date: 2026-11-24

## Context
Сквозной путь пересылает провайдеру `user-agent` Claude Code: некоторые Anthropic-совместимые
провайдеры узнают по нему Claude Code. Сессии вкладки Code у Claude Desktop подписываются
`claude-cli/<версия> (external, claude-desktop-3p)`. Anthropic на запрос по ключу API с таким
`user-agent` (и с `claude-desktop`) отвечает 400 «Your credit balance is too low to access the
Anthropic API» при положительном балансе; тот же запрос с `(external, sdk-cli)` проходит.
Проверено вживую 2026-11-24 на Claude Code 2.1.330 и Opus 5.5: вкладка Code с ключом Anthropic
не работала вовсе.

## Decision
На сквозном пути метка запуска `claude-desktop-3p` / `claude-desktop` в `user-agent` заменяется
на `cli`; остальное (`claude-cli/<версия>`, `external`) остаётся. Свой `user-agent` в
заголовках провайдера по-прежнему заменяет присланный.

## Consequences
- Вкладка Code у Claude Desktop работает с ключом Anthropic API, как консоль.
- Провайдер не узнаёт, что запрос пришёл из Desktop; для шлюза это и так одна сессия Claude Code.
- Если Anthropic начнёт проверять и другие метки, отказ будет тем же текстом о балансе —
  смотреть `user-agent` первым.
