<img src="assets/flashgpt.png" alt="flashGPT icon" width="120">

# flashGPT

Текущая версия — **0.2.0-beta.2**. [English quick start](#quick-start).

Одна флешка для запуска официальных Codex CLI и OpenCode CLI. Меню, шифрование профиля и помощник автооткрытия написаны на Rust. Команды выполняются на подключённом ПК, модель работает у облачного провайдера.

## Запуск

1. Скачай `flashgpt-usb-amd64.zip` из GitHub Releases и проверь SHA-256 по `SHA256SUMS` релиза. Распакуй архив в один каталог на флешке. Для использования между Windows и Linux подходит exFAT.
2. В Windows 11 x64 открой `flashGPT.exe`. В Ubuntu 24.04 LTS и Arch Linux x86-64 открой терминал в каталоге комплекта и выполни `./flashgpt`. При ошибке выполнения проверь права файла и параметр монтирования `noexec`. На Linux-файловой системе обычный запуск требует флага выполнения для лаунчера и исполняемых файлов в `bin/linux-amd64/`.
3. При первом открытии создай пароль профиля. При каждом следующем открытии введи его, чтобы попасть в меню. Для существующего профиля подойдёт прежний пароль.
4. Выбери «Кодить с Codex» или «Кодить с OpenCode». В списке папок введи номер для перехода, `..` для выхода на уровень выше или нажми Enter для выбора открытой папки. `d` показывает диски, `p` позволяет ввести путь, `n` создаёт папку. В первый раз войди в провайдера через интерфейс Codex или `/connect` в OpenCode.
5. После выхода из клиента дождись сохранения и закрой меню перед извлечением флешки.

Пункт **6 — Язык / Language** переключает интерфейс flashGPT между русским и английским. Выбор хранится в `flashgpt-settings.json` на флешке и применяется при следующем запуске, в том числе к экрану пароля.

Пункт «Разобраться с проблемой» предлагает диагностику и исправление. В диагностике агент читает доступные файлы и объясняет причину. Codex запускается в режиме чтения, а OpenCode блокирует правки и команды оболочки. В исправлении агент может вносить правки и выполнять команды с разрешениями выбранного CLI. Быстрые пункты для кодинга открывают тот же режим с правками.

Пункт «Проверить флешку и соединение» содержит две проверки. Комплект проверяется по файлам, свободному месту и работе песочницы. Для проверки сети отправляется HTTPS HEAD-запрос к указанному адресу. Из терминала проверку комплекта можно запустить через `flashgpt doctor` или `.\flashGPT.exe doctor` в PowerShell.

## Автооткрытие

Пункт «Автозапуск» в главном меню показывает текущее состояние и переключает его одним выбором. Выключение действует сразу и оставляет текущее окно открытым. Помощник текущего пользователя распознаёт зарегистрированную флешку и открывает экран ввода пароля. Из терминала доступны `flashgpt autostart install` и `flashgpt autostart uninstall`.

Windows регистрирует запуск помощника в `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Linux использует XDG Autostart и UDisks2. В GNOME для открытия окна сразу после подключения включи автоматическое монтирование и используй GNOME Terminal, GNOME Console или xterm. В TTY смонтируй флешку и запусти `./flashgpt` вручную. Если меню уже открыто, после повторного подключения им можно пользоваться в том же окне. Закрой меню перед безопасным извлечением флешки, чтобы освободить профиль. Команда `autostart status` показывает регистрацию, а `autostart uninstall` удаляет её. Перед обновлением установленного помощника выполни `uninstall`, обнови файлы и повтори `install`.

## Профиль

При первом запуске создаётся ID комплекта в `flashgpt.json`. Сохраняй этот файл и каталог `profile/` вместе. Снимки зашифрованы паролем в формате age. Для резервного копирования достаточно скопировать их после закрытия меню. Если старый токен отозван или истёк, войди снова в нужном CLI.

Пока CLI работает, профиль расшифрован в приватный временный каталог. После штатного выхода flashGPT удаляет этот каталог. После сбоя такой каталог может остаться в `%LOCALAPPDATA%\flashGPT\sessions` на Windows или в `$XDG_RUNTIME_DIR` либо `~/.local/share/flashgpt/sessions` на Linux. При сбое записи сначала подключи тот же носитель и повтори сохранение. Если это невозможно, сохрани предложенную зашифрованную копию. Для восстановления закрой все сеансы, скопируй `profile/` в резервное место и помести копию в `profile/snapshot-0.age` рядом с исходным `flashgpt.json`. Сохрани второй снимок отдельно при ручном откате. Меню выбирает самый новый читаемый снимок.

## Сборка

Установи указанный в `rust-toolchain.toml` Rust и штатный компоновщик системы. Из корня репозитория:

```text
cargo test --locked --workspace
cargo run --locked -p xtask -- prepare
cargo run --locked -p xtask -- build
```

`prepare` скачивает официальные версии из `runtimes.lock.json`, проверяет SHA-256 и создаёт манифест. `build` создаёт лаунчер текущей платформы со встроенным манифестом. Для переносимой Linux-сборки на Ubuntu используй пакет `musl-tools` и следующие команды.

```sh
rustup target add x86_64-unknown-linux-musl
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
export CC_x86_64_unknown_linux_musl=musl-gcc
cargo run --locked -p xtask -- build x86_64-unknown-linux-musl
```

Для общего архива собери Windows и Linux на соответствующих системах, собери результаты в `dist/launchers/`, затем выполни команду упаковки.

```text
cargo run --locked -p xtask -- package
```

Результат появится в `dist/`. Подготовка физической флешки и приёмка описаны в [плане](PLAN.md). Для upstream-клиентов действуют их лицензии из `licenses/`.

## Quick start

flashGPT is a portable Rust launcher for Codex CLI and OpenCode CLI. The bundle targets Windows 11, Ubuntu 24.04 LTS, and Arch Linux on x86-64. It carries the two official clients and your encrypted profile on one drive.

1. Download the beta ZIP from [Releases](https://github.com/harzer228/flashGPT/releases) and check its SHA-256 against `SHA256SUMS`. Extract it to a folder on the USB drive.
2. Open `flashGPT.exe` on Windows or run `./flashgpt` from a terminal in that folder on Linux.
3. Set or enter the profile password. Choose **6 — Язык / Language**, then **2 — English**. The choice stays on the USB drive and applies to the password prompt on the next launch.
4. Choose Codex or OpenCode, then select a working folder. Use a number to open a subfolder, `..` to go up, or Enter to select the current folder.
5. Sign in to your cloud provider on first use. Exit the client, wait for the profile to save, and close the menu before ejecting the drive.

Use **5 — Autostart** to enable or disable the USB watcher on the current computer.
