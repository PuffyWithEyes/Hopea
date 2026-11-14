# Разработка Hopea

У проекта нет особых требований к способу написания кода. Можно присылать
исправления, предложения и переводы, в том числе подготовленные с помощью ИИ.
Ответственность за проверку результата остаётся у автора изменений.
Обсуждения и pull request относятся к репозиторию Hopea, а не к проекту,
от которого он был ответвлён.

Полезно кратко описать проблему, что изменилось и как это проверено.
Для ошибки приложите шаги воспроизведения, систему и версию приложения.
Не публикуйте cookies, токены, приватные ссылки и содержимое профиля браузера.

## Сборка

Команды выполняются из корня проекта. На NixOS или с Nix:

```sh
nix-shell nix/shell.nix
meson setup _builddir . -Dprofile=development
meson compile -C _builddir
meson devenv -C _builddir ./src/hopea
```

Также доступен `nix develop` для полного Git checkout или распакованного архива.
Meson создаёт `src/config.rs` и ресурсы интерфейса; одного `cargo run` недостаточно.
После изменения Blueprint выполните компиляцию повторно, чтобы обновился GResource.
Системные зависимости и ограничения платформ перечислены в [README](../README.md).

## Проверки

```sh
cargo fmt --all -- --check
cargo test --target-dir _builddir/src -- --test-threads=1
cargo test -p hopea-spotify --target-dir _builddir/src
cargo test --manifest-path common/Cargo.toml
cargo test --manifest-path node/Cargo.toml
meson test -C _builddir --suite import --print-errorlogs
bash tests/test_nix.sh
```

Не используйте `cargo test --workspace`: Spotify helper и P2P node намеренно
собираются раздельно из-за несовместимых feature-комбинаций зависимостей.
GUI-тест запускается отдельно с GTK-дисплеем, временными XDG-каталогами и
`GSETTINGS_BACKEND=memory`. Тесты реальных сервисов требуют предоставленных
ссылок или явного разрешения на использование авторизованного профиля.
Подробности — в [инструкции импорта](IMPORTING.md).

Меняйте тесты соразмерно изменению. Для исправления ошибки полезен тест её
воспроизведения; для небольшого перемещения файлов достаточно проверки ссылок
и сборки. Не очищайте рабочие кеши и пользовательскую медиатеку ради тестов.

## Где искать информацию

- [Архитектура и важные связи между компонентами](ARCHITECTURE.md).
- [Безопасность и протокол узла](../node/README.md).
- [Nix, NixOS и Home Manager](../nix/README.md).
- [Пакеты и GitHub Actions](../packaging/README.md).

Лицензии и исходные сведения об авторах сохраняются. Для новых файлов используйте
SPDX-заголовок или запись в `REUSE.toml`; лицензионные условия приведены в
[LICENSES](../LICENSES) и [ADDITIONAL-TERMS.md](../ADDITIONAL-TERMS.md).
