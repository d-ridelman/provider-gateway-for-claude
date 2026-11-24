---
paths:
  - "crates/ctp-gateway/**"
  - "crates/ctp-core/**"
---

# Шлюз: контракт Anthropic

- Клиенту — только форма Anthropic: `{"type":"error","error":{"type","message"}}`, типы по
  статусу (`error::error_kind`), события SSE по порядку: `message_start`, блоки
  start/delta/stop, `message_delta` с `stop_reason` и `usage`, `message_stop`; `ping` при тишине.
- Поток, оборванный провайдером, — событие `error`, не `end_turn`.
- Переполнение окна любыми словами провайдера → `prompt is too long: N tokens > M maximum`
  (`error::classify`), на OpenAI-пути и на сквозном.
- Сквозной путь (Anthropic-совместимые провайдеры) не переписывает тело, кроме известных
  несовместимостей; заголовки ответа `retry-after`, `x-should-retry`, `request-id`,
  `anthropic-ratelimit-*` — клиенту.
- Ключ провайдера не попадает ни в ответ, ни в журнал (`error::without_secret*`).
- Новое поведение — интеграционный тест в `tests/gateway.rs` на заглушке `wiremock`.
- Справка (локально, если есть): `notes/kb/reports/claude-1p-3p.md` (разделы «Контракт
  Messages API» и «Самолечение Claude Code»), `notes/kb/ollama-reference.md`.
