# Hopea через Nix

В корне остаются только `flake.nix` и `flake.lock`. Рецепт пакета —
`nix/package.nix`, окружение разработки — `nix/shell.nix`. Все пять модулей
установки используют одну реализацию в flake.

Образ системы не нужен. `nix run` собирает пакет или получает готовые зависимости
из кеша и запускает приложение; `nix profile add` устанавливает его в профиль.
Hopea пока не опубликована в nixpkgs и не имеет своего бинарного кеша.

## Из исходников

Команды выполняются в корне распакованного исходного архива:

```sh
nix build .#hopea
nix run .#hopea
nix profile add .#hopea
```

В старых версиях Nix вместо `profile add` используется `profile install`.
Если flakes отключены, добавьте `--extra-experimental-features 'nix-command flakes'`
после `nix`. Сборка включает ресурсы, темы значков, кодеки, FFmpeg, Python-библиотеки
импорта и Spotify helper. Pip при запуске приложения не используется.
Версии зависимостей закреплены `flake.lock`, `Cargo.lock` и хешами исходников.
Python-пакеты из nixpkgs могут отличаться от версий pip-окружения разработки.

В Git flakes видят только добавленные в Git файлы. Для текущей рабочей копии
с ещё не добавленными исходниками используйте безопасный архив:

```sh
bash build-aux/make-dist.sh
mkdir -p /tmp/hopea-install
# В архиве уже есть каталог hopea.
tar -xzf dist/hopea-src.tar.gz -C /tmp/hopea-install
cd /tmp/hopea-install/hopea
nix build .#hopea
```

Архив собирается по списку исходных каталогов; локальные окружения, скрытые
файлы, симлинки и кеши сборки исключены. Не помещайте секреты в исходники.
Для разработки: `nix develop`, либо `nix-shell nix/shell.nix` без flakes.

## NixOS, Home Manager, nix-darwin, Nix-on-Droid и NixBSD

В flake конфигурации добавьте input с исходниками Hopea. До публикации репозитория
это может быть абсолютный путь к распакованному архиву:

```nix
inputs.hopea.url = "path:/absolute/path/to/hopea";
```

Передайте `hopea` в аргументы модуля через `specialArgs = { inherit hopea; };`
(NixOS/nix-darwin) или `extraSpecialArgs = { inherit hopea; };` (Home Manager).
Пример модуля Home Manager:

```nix
{ hopea, pkgs, ... }: {
  imports = [ hopea.homeManagerModules.default ];
  programs.hopea.enable = true;
  programs.hopea.package = hopea.packages.${pkgs.stdenv.hostPlatform.system}.hopea;
}
```

Выберите соответствующий экспорт:

| Конфигурация | Модуль | Куда добавляется пакет |
| --- | --- | --- |
| Home Manager, отдельно или внутри системы | `homeManagerModules.default` | `home.packages` |
| NixOS, включая NixOS-WSL | `nixosModules.default` | `environment.systemPackages` |
| macOS с nix-darwin | `darwinModules.default` | `environment.systemPackages` |
| Nix-on-Droid | `nixOnDroidModules.default` | `environment.packages` |
| NixBSD (экспериментально) | `nixbsdModules.default` | `environment.systemPackages` |

В каждом случае опции одинаковы: `programs.hopea.enable` и `programs.hopea.package`.
Указание пакета из flake использует закреплённые зависимости Hopea; без явного
`package` рецепт использует `pkgs` вашей конфигурации. Старые nixpkgs могут не иметь
нужных версий GTK/Rust. Не подключайте системный и пользовательский модуль одновременно,
если достаточно одной установки. Модули не включают раздачу, автозапуск, VPN или
сетевые разрешения и не изменяют пользовательские настройки.

Для overlay: `nixpkgs.overlays = [ hopea.overlays.default ];`, затем `pkgs.hopea`.
Для собственного набора пакетов: `hopea.lib.mkPackage pkgs`.

## Платформы и ограничения

| Система | Путь запуска | Проверка / ограничение |
| --- | --- | --- |
| Linux, NixOS, обычный Nix | Нативный пакет x86_64/aarch64 | Вычисление обоих рецептов; сборка проверяется на x86_64 Linux |
| macOS / nix-darwin | Нативный пакет Intel/Apple Silicon | Рецепты вычисляются; нужен тест сборки и GUI на Mac. Закреплённый nixpkgs предупреждает о прекращении поддержки Intel Mac после 26.05 |
| Windows через Nix | WSL2 + WSLg, Linux-пакет | WSL2 использует виртуальную машину; требует проверки аудио, браузера и сети на Windows |
| Windows без виртуальной машины | Нативная `.exe`-сборка | `build-aux/windows/build.sh` под MSYS2 UCRT64; не Nix-пакет, нужен тест на Windows |
| Nix-on-Droid | Linux-пакет aarch64 в PRoot | Нужны отдельные графическая и звуковая сессии; установка пакета не создаёт Android Activity или медиасервис |
| Android без Linux-окружения | Нативный APK | См. [Android preview](../README.md#android-preview) |
| FreeBSD / GhostBSD / HardenedBSD / NixBSD | Нативный рецепт с BSD-набором пакетов | Закреплённый upstream nixpkgs блокирует `libproxy` для FreeBSD. Модуль подготовлен, готовой сборки нет; нужен совместимый набор зависимостей |
| NetBSD / OpenBSD / DragonFly и производные | Нативный source-build с пакетами ОС | Полный Nix-стек Hopea не подтверждён; см. `build-aux/desktop/build.sh`. Требуется проверка версий и нативной сборки |
| Redox | Отдельный порт | Нужна большая работа над GTK/GStreamer и платформенным backend; Nix-модуль это не решает |

Никакая из команд Hopea не создаёт и не запускает виртуальную машину автоматически.
WSL — отдельный добровольный вариант, если нужен именно Nix на Windows.
Nix-on-Droid не предоставляет доступ к приватным данным браузеров Android.
В WSL/PRoot VPN на хосте может быть невидим приложению; проверка VPN не заменяет
межсетевой экран. Не отключайте её автоматически ради запуска на такой системе.

На обычном Linux могут потребоваться настройки GPU/порталов вашего дистрибутива.
На BSD нужен session D-Bus для MPRIS/трея. На macOS это пока запуск `hopea` из
терминала, а не подписанный standalone `.app`.

Проверки модулей: `bash tests/test_nix.sh` (20 сочетаний включения и подмены пакета).
Они проверяют контракт модулей и не заменяют запуск на каждой ОС.

## Первичные источники

- [Nix: поддерживаемые платформы](https://nix.dev/manual/nix/2.34/installation/supported-platforms)
- [NixBSD: нативный FreeBSD и состояние nixpkgs](https://github.com/nix-community/nixbsd)
- [Microsoft: графические приложения в WSLg](https://learn.microsoft.com/en-us/windows/wsl/tutorials/gui-apps)
- [Nix-on-Droid и его интеграция с Home Manager](https://github.com/nix-community/nix-on-droid)
- [Home Manager](https://nix-community.github.io/home-manager/)
