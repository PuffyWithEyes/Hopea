# Пакеты Hopea

Это рецепты сборки для собственного репозитория и GitHub Releases. Они не означают,
что Hopea уже появилась в AUR, nixpkgs, Flathub или официальных репозиториях ОС.
Полная сборка проверена локально на NixOS x86_64; остальные рецепты требуют первого
запуска CI и проверки запуска на соответствующих системах. Проверка YAML и shell
не заменяет такую сборку.
Дополнительно проверено разрешение сборочных зависимостей в одноразовых контейнерах
Alpine edge и Void musl; это ещё не компиляция и не проверка интерфейса.

## Что подготовлено

| Система | Рецепт / результат | Условия |
| --- | --- | --- |
| Arch Linux и производные | `arch/PKGBUILD.in` → `.pkg.tar.zst` | CI в Arch, x86_64; рецепт также допускает aarch64 с нативным сборщиком |
| Artix Linux | Тот же `PKGBUILD` для сборки в Artix | Нет зависимости от systemd; совместимость готового Arch-пакета зависит от версий библиотек Artix, отдельного прогона Artix пока нет |
| Void Linux | `void/build.sh` → `.xbps` и индекс репозитория | Отдельные CI-контейнеры glibc и musl, x86_64; не смешивать пакеты разных libc |
| Alpine Linux | `alpine/APKBUILD.in` → подписанный `.apk` | CI: edge, x86_64, musl; рецепт допускает aarch64 с нативным сборщиком. Это пакет Alpine, не Android APK |
| LFS / BLFS | `lfs/build.sh` → архив дерева установки | Сборка под библиотеки своей системы; опциональный CI на собственном LFS runner |
| Debian / Ubuntu | `debian/` → `.deb` | CI: Debian sid и Ubuntu 26.10; не переносить эти DEB в старые стабильные версии |
| Fedora | `rpm/hopea.spec.in` → `.rpm` | CI: rawhide; дополнительные кодеки/FFmpeg должны быть доступны в репозиториях системы |
| openSUSE | Тот же RPM-рецепт с зависимостями openSUSE | CI: Tumbleweed |
| ALT Linux | RPM-рецепт с зависимостями ALT | CI: Sisyphus; стабильные ветки требуют проверки версий библиотек |
| Gentoo | `gentoo/` → overlay с ebuild и опциональный `.gpkg.tar` | Сборка из исходников Portage; зависимости импорта включены по умолчанию, есть ebuild для VK, Яндекса и Cryptodome |
| NixOS, Nix, Home Manager | [Nix-рецепт и модули](../nix/README.md) | x86_64/aarch64 Linux; CI также экспортирует бинарный кеш |
| CentOS Stream / RHEL / Rocky / Alma | Flatpak | Нативный RPM требует GTK 4.20+ / libadwaita 1.8+; старый системный стек не заменяется автоматически |
| Linux Mint и другие стабильные Linux | Flatpak | Нужны Flatpak и доступ к GNOME runtime из Flathub |
| FreeBSD / DragonFly | `bsd/build.sh` → `.pkg` и архив файлов | Экспериментальные CI-сборки в BSD; зависят от доступности современных библиотек |
| NetBSD / OpenBSD | `bsd/build.sh` → `.tgz` через native `pkg_create` | С зависимостями и ABI конкретной системы; пока не проверены на этих ОС, не официальный port/pkgsrc |
| GhostBSD / HardenedBSD и другие производные | Рецепт базовой BSD | Совместимость ABI и доступность пакетов проверяются на конкретной версии |
| Windows 10/11 x64 | MSYS2 UCRT64 → Setup EXE + ZIP + Scoop JSON | Установка для текущего пользователя; GTK, GStreamer, FFmpeg и приватный Python включены; [совместимость и сборка](windows/README.md) |
| macOS Apple Silicon / Intel | Homebrew formula + bottle | Нативная сборка и зависимости Homebrew; не standalone `.app`, нет Apple notarization |
| Android ARM64 / x86-64 | APK preview | Отдельные тяжёлые jobs, один стабильный preview-ключ; Android 12+, x86-64 ещё требует полной проверки APK и запуска |
| Flatpak | `build-aux/flatpak/` → `hopea.flatpak` | Общий вариант для Linux, runtime GNOME 51 |

Для нативных Linux-пакетов используются GTK **4.20+**, libadwaita **1.8+** и
GStreamer **1.20+**. Скрипт проверяет версии до компиляции Rust. Если библиотек
нет, потребуется их отдельный порт/обновление или Flatpak; снижение требований
в метаданных не сделает приложение совместимым.

Gentoo, BSD, Android и LFS включаются при ручном запуске workflow. BSD собирается в VM на
GitHub, потому что GitHub не предоставляет штатных BSD runners. **Это только
машина сборки**: полученный BSD-бинарник запускается в самой BSD без VM.
Все обычные GitHub-hosted runners также являются выделенными машинами CI;
на компьютер пользователя они не устанавливаются.

## GitHub Actions

1. Загрузите исходники в свой GitHub-репозиторий, включая новые файлы. Большая часть
   работы в текущем checkout ещё не добавлена в Git; не отправляйте кеши, профили,
   cookies или ключи.
2. Откройте **Actions → Packages → Run workflow**. Для обычной сборки оставьте
   `draft_release` выключенным. Каждый успешный job загрузит свой **Artifact**.
3. Скачайте нужные артефакты и проверьте запуск. Jobs независимы: ошибка BSD не
   удаляет успешную Windows/Linux-сборку.
4. При необходимости создайте тег вида `v2026.2`, совпадающий с версией Meson.
   Запуск по тегу создаст **черновик** релиза только после успешной основной матрицы.
   Его публикация остаётся в интерфейсе GitHub. Ручной запуск на теге с
   `draft_release=true` делает то же самое.

Workflow `Checks` проверяет Rust, импортер, Nix-модули и упаковочные скрипты.
Никакие branch protection, обязательные reviews и ограничения на коммиты не заданы.
У сборочных jobs только `contents: read`; право записи получает лишь job черновика.
Actions закреплены по commit SHA. Код из pull request не получает release-секреты.
Контейнеры rolling-дистрибутивов обновляются; их фактический digest сохраняется
рядом с пакетом. Это не заявление о побитово воспроизводимых сборках всех ОС.

Файлы для конкретного релиза создаются по `github.repository`, версии Meson и
реальным SHA-256 артефактов. В них нет ссылки на репозиторий исходного форка.
Архив исходников содержит закреплённые Rust crates для offline-сборки. Python
wheels скачиваются на отдельном подготовительном шаге для каждой ОС и Python;
установка готового пакета не скачивает зависимости через pip.

### Настройки, нужные для отдельных вариантов

- **Android:** secret `ANDROID_PREVIEW_KEYSTORE_BASE64` — base64 одного постоянного
  debug-keystore с alias `androiddebugkey` и стандартными debug-паролями `android`.
  Это preview-подпись. Для production потребуется отдельный release signing pipeline.
  Ключ не входит в исходники, артефакты или кеши и удаляется после job. Не присылайте
  его в чат. Без секрета Android job останавливается, не создавая новый несовместимый ключ.
- **Nix-кеш:** необязательный secret `NIX_SIGNING_KEY` позволяет подписать экспорт.
  Пользователям потребуется соответствующий публичный ключ в `trusted-public-keys`.
  Без него артефакт кеша остаётся неподписанным; обычная безопасная установка —
  `nix build` / `nix run` из исходников. Проверку подписей глобально отключать не нужно.
- **macOS:** формулы и bottle используют Homebrew. Для постоянного tap можно
  перенести сгенерированную формулу своей архитектуры как `Formula/hopea.rb`.
  Два platform-source архива содержат разные Python wheels и не взаимозаменяемы.
- **Gentoo:** архив recipes содержит готовую структуру overlay. Подключите его в
  Portage, создайте Manifest командой `ebuild … manifest`, затем устанавливайте
  `media-sound/hopea`. Это пока private overlay, не пакет официального Gentoo tree.
  Опциональный job `gentoo` собирает бинарный пакет; постоянный Gentoo binhost пока не настроен.
- **LFS:** опция `lfs` требует собственного runner с метками `self-hosted`, `Linux`,
  `hopea-lfs` и уже установленными зависимостями BLFS. Без такого runner оставьте
  опцию выключенной. Этот job запускается только вручную, не из pull request;
  используйте отдельного пользователя сборки без прав изменения системы.
- **Alpine:** CI создаёт ключ подписи для конкретной сборки и отдаёт только публичную
  часть рядом с APK. Для постоянного репозитория нужен постоянный ключ; сейчас
  автоматическое обновление между разными ключами не настроено.

## Установка артефактов

После публикации скачивайте файлы из **Releases** этого репозитория.
`SHA256SUMS` позволяет проверить, что скачанный файл совпадает с опубликованным.
Сначала проверьте файл, затем используйте пакетный менеджер своей системы:

```sh
# Arch / Artix с подходящими версиями библиотек
sudo pacman -U ./hopea-*.pkg.tar.zst
# Debian / Ubuntu подходящей версии
sudo apt install ./hopea-*.deb
# Fedora
sudo dnf install ./hopea-*.rpm
# openSUSE
sudo zypper install ./hopea-*.rpm
# ALT
sudo apt-get install ./hopea-*.rpm
# Flatpak, в том числе на CentOS и стабильных Linux
flatpak install --user ./hopea.flatpak
flatpak run io.hopea.Hopea
# FreeBSD / DragonFly, соответствующая ABI
sudo pkg add ./hopea-*.pkg
```

Void: положите `.xbps` и соответствующий `*-repodata` в одну локальную папку,
затем установите через `sudo xbps-install --repository=/полный/путь/к/папке hopea`.
Выбирайте glibc или musl соответственно своей системе. Это локальный репозиторий;
публикация подписанного сетевого XBPS-репозитория пока не настроена.

Alpine: после проверки источника пакета и публичного ключа скопируйте этот `.pub`
в `/etc/apk/keys/`, затем выполните `sudo apk add ./hopea-2026.2-r0.apk`
(подставьте версию файла). Проверка подписи остаётся включённой.

Windows: запустите `hopea-VERSION-windows-x86_64-setup.exe`. Установка не требует
администратора; новую версию можно установить поверх прежней. Альтернативно
распакуйте ZIP и запустите `Hopea/bin/hopea.exe`, либо добавьте сгенерированный
`hopea.json` в свой Scoop bucket. XP/Vista/7/8 и 32-битная Windows этой сборкой
не поддерживаются. macOS: bottle можно установить через Homebrew;
для установки из исходников используйте соответствующую formula. Android:
`adb install -r Hopea-android-arm64-preview.apk` после включения USB debugging.

Пакеты NetBSD/OpenBSD создаются штатным `pkg_create`, с зависимостями конкретной
сборочной системы, и устанавливаются через `pkg_add`. Они ещё требуют проверки
в соответствующей BSD. Оформление в официальные ports/pkgsrc не выполнено.
Дополнительный `.tar.gz` содержит дерево установки для диагностики; не распаковывайте
его поверх рабочей системы вместо пакетного менеджера.

## Локальная сборка

Из Nix-окружения:

```sh
nix develop --command bash packaging/source.sh
HOPEA_REPOSITORY=OWNER/REPO HOPEA_VERSION=2026.2 \
  bash packaging/render.sh dist/hopea-2026.2-source.tar.gz
bash tests/test_packaging.sh
```

`native.sh prepare` загружает зависимости, `configure/build` компилирует без
сети Cargo, `install` работает только с отдельным `DESTDIR`. Скрипт
`linux-deps.sh` предназначен для одноразового контейнера CI; не запускайте его
как средство установки зависимостей на своей рабочей системе.

Для LFS подготовьте инструменты Rust/Cargo, C/C++ compiler, Meson/Ninja,
pkg-config, Blueprint, gettext, desktop-file-utils, Python с pip/venv, FFmpeg,
а также библиотеки разработки GTK 4.20+, libadwaita 1.8+, libsoup 3, OpenSSL,
GStreamer и его плагины. Описание стека есть в [книге BLFS](https://www.linuxfromscratch.org/blfs/view/svn/x/libadwaita.html).
Из распакованных исходников выполните `bash packaging/lfs/build.sh`.
Результат — `dist/hopea-VERSION-lfs-ARCH.tar.gz`, префикс по умолчанию `/usr/local`.
Сначала изучите содержимое архива и включите его в свой способ учёта установленных
файлов. Сборка сама не пишет в `/usr` и не настраивает систему; архив не считается
переносимым между произвольными установками LFS.

Большие работы, которые остаются отдельно: полноценный GTK/GStreamer порт для
Redox, поддержка старых системных библиотек, production-подпись Android,
standalone подписанный `.app` для macOS и официальное включение в репозитории ОС.
