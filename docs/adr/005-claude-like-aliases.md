# ADR-005. Псевдонимы моделей claude-… и behavesAs для effort

- Status: Accepted
- Date: 2026-11-19 (записано 2026-11-24)

## Context
Claude Desktop (с ~1.6259) выкидывает из списка модели, чьи имена не похожи на Claude.
Уровни рассуждения (effort) Desktop показывает только у модели, которую узнаёт.

## Decision
Шлюз выдаёт модели под псевдонимами `claude-<провайдер>-<8 цифр из хеша>`
(`ctp-core/src/alias.rs`). Desktop отбрасывает и имя с `claude-`, если в нём есть слово из
его списка: тогда псевдоним запасной, `claude-p<6 цифр>-<8 цифр>`. Моделям не Claude с
потолком effort (OpenAI и Anthropic-совместимые: DeepSeek, GLM…) Desktop получает
`behavesAs: "claude-opus-4-8"` (`claude::EFFORT_PROFILE`), моделям Claude — их собственный ID.

## Consequences
- Модели видны и в Desktop, и в Claude Code; уровни effort работают.
- `behavesAs` передаёт сессиям Code ещё и окно и предел вывода той модели: у моделей не Claude
  без 1M вкладка Code считает окно 1M. Сейчас это прикрывает перевод переполнения в
  «prompt is too long» (ADR-003). Открытый вопрос — `docs/knowledge/issues.md`.
