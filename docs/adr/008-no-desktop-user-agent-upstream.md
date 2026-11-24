# ADR-008. Провайдеру Claude Code представляется как `claude -p`

- Status: Accepted
- Date: 2026-11-24

## Context
Сквозной путь пересылает провайдеру `user-agent` Claude Code: некоторые Anthropic-совместимые
провайдеры узнают по нему Claude Code. Anthropic на запрос по ключу API отвечает 400 «Your
credit balance is too low to access the Anthropic API» при положительном балансе, если
`user-agent` — интерактивный Claude Code: `claude-cli/<версия> (external, cli)`,
`(external, claude-desktop-3p)` (так подписываются сессии вкладки Code у Claude Desktop),
`(external, claude-desktop)`, `claude-cli/<версия>` без метки. С `(external, sdk-cli)` (это
`claude -p`), `(external, sdk-ts)` и чужим `user-agent` тот же запрос проходит. Проверено
вживую 2026-11-24 на Opus 5.5 через шлюз; вкладка Code и консоль PGC с ключом Anthropic не
работали вовсе.

Ту же метку Anthropic читает в блоке атрибуции в начале `system`
(`x-anthropic-billing-header: cc_version=…; cc_entrypoint=claude-desktop-3p;`): Desktop включает
этот блок сам, `CLAUDE_CODE_ATTRIBUTION_HEADER=0` из PGC он перезаписывает.

## Decision
На сквозном пути `user-agent` вида `claude-cli/<версия> …` без метки `sdk-*` уходит провайдеру
как `claude-cli/<версия> (external, sdk-cli)`, а `cc_entrypoint` в блоке атрибуции — как
`sdk-cli`; другие блоки `system` не трогаются. Метки `sdk-*` и чужие клиенты не трогаются. Свой
`user-agent` в заголовках провайдера по-прежнему заменяет присланный.

## Consequences
- Вкладка Code у Claude Desktop и консоль PGC работают с ключом Anthropic API.
- Провайдер по-прежнему видит Claude Code и его версию, но не вид запуска.
- Если Anthropic начнёт отвергать и `sdk-cli`, отказ будет тем же текстом о балансе —
  смотреть `user-agent` первым.
