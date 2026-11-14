# Сборки и релизы Hopea

Выпускать версию можно когда удобно. Подписанные теги, особый формат коммитов,
заполненный changelog и обязательное одобрение для этого не требуются.

## Через GitHub

После загрузки проекта на GitHub сборки запускаются из вкладки **Actions**.
Workflow создаёт пакеты и сохраняет их в **Artifacts**. Их можно скачать и проверить,
не создавая релиз. Для публикации предусмотрен отдельный шаг создания черновика
GitHub Release; можно просмотреть файлы и опубликовать его вручную.

Имена файлов и версии пакетов берутся из `meson.build`. Если используется тег,
его вид `v2026.2` связывает сборку с конкретной версией; это технический формат CI,
а не требование к обычной работе с Git. Подписывать тег необязательно.

### Пошагово

1. Дождитесь успешного workflow **Checks** и проверьте приложение. Обновите
   `version` в `meson.build`, сохраните изменения коммитом и отправьте `main`.
2. Создайте тег с той же версией (пример для текущей `2026.2`):

   ```sh
   git tag v2026.2
   git push origin v2026.2
   ```

3. Отправка тега запускает **Packages** и после успешных сборок создаёт черновик
   релиза с обычными платформами. Android, BSD и Gentoo включаются отдельно:
   **Actions → Packages → Run workflow**, выберите **тег** `v2026.2`, включите
   нужные платформы и `draft_release`. Для LFS нужен собственный настроенный runner.
4. Дождитесь завершения сборок. В **Releases** откройте черновик, скачайте и
   проверьте нужные файлы, заполните описание и нажмите **Publish release**.
   Windows получает Setup EXE и ZIP; Android — отдельные APK для ARM64 и x86-64;
   Linux/BSD/macOS — свои пакеты/архивы, перечисленные в руководстве упаковки.

Повторный запуск на том же теге обновляет файлы **черновика**, поэтому можно
добавить Android позже. Опубликованный релиз workflow не перезаписывает:
для исправленного выпуска создайте новую версию и новый тег. Если сборка
обязательной платформы завершилась ошибкой, исправьте её до публикации;
успешные отдельные пакеты можно скачать из Artifacts соответствующего запуска.

### Постоянный ключ для Android preview

Один раз создайте ключ вне репозитория (нужен `keytool` из JDK):

```sh
install -d -m 700 ~/.local/share/hopea-signing
keytool -genkeypair -keystore ~/.local/share/hopea-signing/debug.keystore \
  -storetype JKS -alias androiddebugkey -keyalg RSA -keysize 3072 \
  -validity 10000 -storepass android -keypass android \
  -dname 'CN=Hopea Android Preview'
chmod 600 ~/.local/share/hopea-signing/debug.keystore
```

Сохраните резервную копию ключа. В репозитории GitHub откройте
**Settings → Secrets and variables → Actions → New repository secret**, имя
`ANDROID_PREVIEW_KEYSTORE_BASE64`, значение — base64 этого файла. Если установлен
GitHub CLI и выполнен `gh auth login`, можно передать ключ без вывода на экран:

```sh
base64 -w0 ~/.local/share/hopea-signing/debug.keystore | \
  gh secret set ANDROID_PREVIEW_KEYSTORE_BASE64 --repo PuffyWithEyes/Hopea
```

Команда base64 выше предназначена для Linux. Ключ не коммитьте и не присылайте
в чат. Для обновления ранее установленного APK нужен именно его прежний ключ.
Этот pipeline выпускает debug-подписанный **preview**, а не готовый релиз для
Google Play: для production ещё нужен release-вариант подписи и сборки.

Матрица платформ, запуск отдельных сборок и необходимые настройки описаны
в [packaging/README.md](../packaging/README.md). Home Manager и другие способы
установки Nix — в [nix/README.md](../nix/README.md).

## Локально

```sh
# Команды из корня проекта
nix build .#hopea
bash build-aux/flatpak/build.sh
# Windows: MSYS2 UCRT64
bash build-aux/windows/build.sh
# Затем в PowerShell 7: ./packaging/windows/build.ps1
# Android: Docker и подготовленные ресурсы интерфейса
bash build-aux/android/build.sh
```

Скрипты кладут файлы для передачи в `dist/`. Первые сборки скачивают зависимости
и могут идти долго; кеши Cargo, Nix и Flatpak ускоряют последующие запуски.
Публикация пакетов в AUR, Flathub и системных репозиториях пока не настроена.

Windows job создаёт переносимый ZIP и обычный Setup EXE. Установщик рассчитан на
Windows 10/11 x64; [ограничения и локальная сборка](../packaging/windows/README.md).
