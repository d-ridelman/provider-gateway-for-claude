# ADR-018. Cowork в 3p: host-loop на портативной копии, VM — по желанию

- Status: Accepted
- Date: 2026-11-26
- Supersedes: —

## Context

Claude Desktop в 3p (`inferenceProvider: gateway`) принудительно включает host loop:
`keepsSessionCliOnHost() { return !0 }` в `index.chunk-*.js` провайдера custom-3p. Сессия
Cowork в 3p выполняется на хосте, а не в VM; попытка продолжить старую VM-сессию отклоняется
(«created to run inside the VM sandbox, which this deployment no longer uses»).

Тем не менее проверка `Exi()` отключает Cowork на любой копии, запущенной не из MSIX:
`if (e === "win32" && !Qi()) return { unsupportedCode: "msix_required" }`, затем проверяются
Windows 10 19041+, службы HCS и `connectNamedPipeSystemOwned`. `Qi()` смотрит только на
`process.windowsStore` или сегмент `WindowsApps` в пути `claude.exe`.

Штатный MSIX на Windows требует администратора: пакет регистрирует службу
`CoworkVMService` (LocalSystem), без неё установка идёт «без Cowork» (`0x80073D28`).
PGC — портативная программа; на доменном ПК без прав администратора MSIX поставить нельзя.
При этом Cowork в 3p и так не использует VM, поэтому проверка VM блокирует его напрасно.

Гибрид — переведённая копия с идентичностью пакета через `Invoke-CommandInDesktopPackage` —
проверен в Windows Sandbox и **не работает**: идентичность пакета она получает (`Package
family from native API: Claude_pzs8sxrjxfjjc`, VM-клиент загружается, образ VM качается), но
служба `CoworkVMService` отвечает на каждый запрос «signature verification failed: client …
was not started from the package directory C:\Program Files\WindowsApps\Claude_…». Служба
принимает только exe из каталога пакета, а менять файлы пакета нельзя. Остаётся запуск
настоящего `claude.exe` пакета, но с профилем и конфигом PGC.

## Decision

1. В `app.asar` копии PGC применяются восемь патчей:
   - в ветке `msix_required` условие `!Qi()` заменяется на `!1`;
   - E2E-проверка `<fn>("CLAUDE_E2E_ASSUME_VM_SUPPORTED") ? <ok> : <bad>()` заменяется на
     `!0 ? <ok> : <bad>()`, то есть поддержка VM считается подтверждённой (иначе на машине
     без HCS/VMP остаётся `hcs_not_available`);
   - проверка VirtualMachinePlatform (`isVirtualMachinePlatformEnabledOrUnneeded`) всегда
     отвечает «да»: при «VM поддерживается» окно Claude иначе требует включить компонент
     Windows (права администратора) и не стартует сессию (`VMP_NOT_ENABLED`);
   - `downloadVM` (образ VM, больше 1 ГБ) и `prepareForVM` (Linux-версия Claude Code) выполняются
     только если копия запущена из MSIX (`process.windowsStore` или сегмент `WindowsApps`, как у
     `Qi()`); иначе при «VM поддерживается» они качались впустую на первом сообщении;
   - IPC `startVM`, который окно зовёт при старте сессии, вне MSIX сразу отвечает «успех»: иначе
     окно показывает красный баннер «Не удалось запустить рабочее пространство (prepare: no_vm)»;
   - shell: вне MSIX `Bash` и `PowerShell` не запрещаются, псевдоним `Bash` → VM-инструмент
     снимается, сам VM-инструмент закрывается, а фильтр белого списка `tools` добавляет `Bash` и
     `PowerShell`. В `allowedTools` они не попадают (их вычищает список запретов), поэтому
     вызовы идут через подтверждение, как и во вкладке Code.
   Патчи ищутся по строке сообщения `EmeqFY8DA1`, литералу `CLAUDE_E2E_ASSUME_VM_SUPPORTED` и
   форме функции проверки VMP, а не по именам минифицированных функций. Если якорь не найден
   или найден не один раз, копия собирается без патча, PGC пишет предупреждение, а
   самопроверка (`upkeep::check`) показывает, что патч не ложится на эту версию.
2. `Qi()` не трогаем: VM-клиент не загружается, Cowork идёт host-loop, VM не скачивается.
3. В 3p-запись пишутся `desktopHome: "standard"` (Cowork разрешён), `projectsEnabled: true`;
   режим `off` пишет `desktopHome: "no-vm"`.
4. Настройка `coworkMode`:
   - `host` (по умолчанию) — портативная переведённая копия, без MSIX и прав администратора;
   - `off` — только Chat, как раньше;
   - `vm` — если найден MSIX и служба `CoworkVMService`, запускается настоящий `claude.exe` из
     каталога пакета через `Invoke-CommandInDesktopPackage` и `cmd.exe` (`set` переменных,
     `-PreventBreakaway`) со **стандартным** профилем `%LOCALAPPDATA%\Claude-3p`: служба хранит
     образ по этому пути и junction отвергает, поэтому подмены `LOCALAPPDATA` и `--user-data-dir`
     в этом режиме нет. Конфиг шлюза PGC пишется в стандартную `configLibrary` поверх чужого:
     файлы других записей не удаляются, в `_meta.json` они остаются, `appliedId` — наш.
     Работают ключи, модели, артефакты, поиск и Code; русского перевода нет — служба принимает
     только exe из каталога пакета, а его asar подписан и только для чтения, переводить нечего.
     Если MSIX или службы нет, PGC откатывается на host и говорит об этом;
   - выбор — в окне настроек, с пояснением к каждому режиму.

## Consequences

- **Открыто, проверено в Windows Sandbox 26.11.2026 (Claude 6.28899.6, без VMP/HCS):** с тремя
  патчами сообщение с главного экрана запускает сессию Cowork в host-loop (`[HostLoop]
  sdkOptions`, CLI из `claude-code\2.1.330\…`, `hasSpawnClaudeCodeProcess: false`).
- **Закрыто 1 (проверено в песочнице).** Образ VM и Linux-версия Claude Code качались на первом
  сообщении, потому что при «VM поддерживается» их запускают `downloadVM` и `prepareForVM`. Два
  патча пропускают их, если копия запущена не из MSIX (`process.windowsStore` или сегмент
  `WindowsApps`, как у `Qi()`). Результат: скачан только хостовый Claude Code, ни `rootfs`, ни
  `claude-code-vm`, ни `vm_bundles`.
- **Закрыто 2 (проверено в песочнице).** Штатный shell Cowork — `mcp__workspace__bash` — живёт
  только в VM: без неё он отвечал «Workspace unavailable … (VM is not used here)». Седьмой патч
  (shell) оставляет модели штатные `Bash` и `PowerShell` Claude Code. Проверено: модель получает
  `PowerShell`, вызов ждёт подтверждения («Claude хочет использовать инструмент PowerShell»,
  «Отклонить» / «Разрешить один раз»), после «Разрешить» команда выполняется на хосте и
  возвращает вывод (`echo` безобидная read-only команда и проходит без вопроса, запись файла —
  нет). `Bash` в песочнице не проверен: нет Git for Windows.
- **Закрыто 3 (проверено в песочнице).** Красный баннер «Не удалось запустить рабочее
  пространство» исчез после патча IPC `startVM`.

- Cowork работает без MSIX и без администратора, сохраняются русский перевод, профиль PGC
  и портативность. Это главный сценарий для доменных ПК.
- **Shell в host-режиме не изолирован:** команды выполняются прямо на компьютере, как в
  вкладке Code на Windows. Защита — подтверждение каждого вызова; в режимах «Автоматически»
  и «Обход разрешений» его нет. Окно PGC говорит об этом в пояснении к режиму.
- В host-режиме нет advanced file analysis (нужна VM).
- Патч привязан к тексту минифицированного чанка. Новая версия Claude может его сломать;
  PGC проверяет якорь, пишет предупреждение и не выдаёт копию за рабочую.
- **VM-режим проверен на машине с MSIX (26.11.2026, вечер) и работает со стандартным профилем.**
  Сообщение в Chat запустило Cowork: `[VM:start] Configuring Windows VM service...`,
  `Windows VM service configured`, `Startup complete, total time: 13877ms`, а команда
  `echo VM_OK`, вызванная моделью через `mcp__workspace__bash`, вернула `VM_OK` из гостя.
  Профиль — `%LOCALAPPDATA%\Claude-3p`, без `--user-data-dir` и подмены `LOCALAPPDATA`.
  Пакетный exe на этой машине стоит (MSIX 6.28899.6, служба Running).
- Ранее (тот же день, до перехода на стандартный профиль) VM-режим не работал. Работает:
  пакет ставится (подпись «Anthropic, PBC» валидна), `CoworkVMService` запускается, PGC находит
  пакет и запускает настоящий `claude.exe` пакета с профилем PGC (`managedConfig type: 'local'`),
  служба принимает этого клиента (вместо «signature verification failed» — «not configured:
  call 'configure' first»), образ VM (1,3 ГБ) скачивается и распаковывается в профиль PGC.
  Не работает: служба сама ждёт образ по фиксированному пути `%LOCALAPPDATA%\Claude-3p\
  vm_bundles\claudevm.bundle\rootfs.vhdx` и отвечает «failed to set VHDX path: VHDX file not
  found», потому что клиент положил его в `--user-data-dir` профиля PGC. С нестандартной папкой
  данных VM не стартует. Junction `vm_bundles` на образ в профиле PGC проверен и отвергнут
  службой: «configure: path …\Claude-3p\vm_bundles is a symlink or junction, refusing to open».
  Остаются два пути (затрагивают ADR-001, свой профиль): (а) в режиме `vm` брать стандартную
  папку `Claude-3p` как профиль (PGC пишет туда `configLibrary` и не стирает чужие записи; в
  ней у владельца остатки прошлой попытки с MSIX), (б) убрать режим `vm`. Сам старт VM на HCS
  и shell внутри VM не проверены.
- `Invoke-CommandInDesktopPackage` не передаёт процессу переменные окружения вызывающего: без
  обёртки `cmd.exe` Claude видит настоящий `LOCALAPPDATA`, не находит конфиг 3p и показывает
  экран входа.
- `desktopHome: "standard"` даёт grant `fileAnalysis`, но VM в host-режиме нет; advanced file
  analysis в Chat может показать ошибку. Это принимается: Cowork важнее.
