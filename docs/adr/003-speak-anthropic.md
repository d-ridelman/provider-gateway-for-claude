# ADR-003. Шлюз отвечает языком Anthropic

- Status: Accepted
- Date: 2026-11-24

## Context
Для Claude Code шлюз за `ANTHROPIC_BASE_URL` — это сам Anthropic API. Клиент шлёт полный
набор возможностей первой стороны и сам деградирует (снимает беты, меняет thinking, сжимает
разговор), но только когда отказ сказан словами Anthropic или помечен `capability_rejected:`
(`docs/kb/reports/claude-1p-3p.md`, официальный llm-gateway-protocol).

## Decision
Всё, что видит клиент, — в форме Anthropic: тело ошибки и её тип по статусу (402 billing,
529 overloaded, 504 timeout), «prompt is too long: N tokens > M maximum» для переполнения в
любых словах провайдера, порядок событий SSE, `ping` при тишине, `error` вместо `end_turn`
при обрыве, заголовки повторов и лимитов со сквозного пути. Неизвестное не выдумывается.

## Consequences
- Самолечение Claude Code работает через шлюз так же, как с Anthropic.
- Каждый новый вид отказа провайдера — правка `error.rs` и тест.
