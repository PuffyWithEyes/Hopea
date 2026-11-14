# Hopea builds and releases

Release whenever it is useful. Signed tags, special commit formats, a completed
changelog and mandatory approvals are not required.

## GitHub builds

After pushing the project to GitHub, start builds from **Actions**. The workflow
creates packages and saves them as **Artifacts**, which can be downloaded and
tested without creating a release. A separate step prepares a GitHub Release
draft for review and manual publication.

Package versions and filenames come from `meson.build`. Tags such as `v2026.2`
associate builds with a specific version. This is a CI naming convention, not a
requirement for everyday Git work. Tags do not need signatures.

### Publishing a version

1. Update `version` in `meson.build`, commit the changes and push `main`. Check
   that **Checks** succeeds and test the application.
2. Create and push a matching tag, for example:

   ```sh
   git tag v2026.2
   git push origin v2026.2
   ```

3. Pushing the tag starts **Packages**. Successful required builds create a draft
   release for the default platforms. Enable Android, BSD and Gentoo separately:
   open **Actions → Packages → Run workflow**, select the **tag** `v2026.2`, enable
   the desired platforms and `draft_release`. LFS requires a configured
   self-hosted runner.
4. Wait for the builds, open the draft under **Releases**, download and test its
   files, write a description, then select **Publish release**. Windows gets a
   Setup EXE and ZIP; Android gets separate ARM64 and x86-64 APKs; Linux, BSD and
   macOS get the packages listed in the [packaging guide](../packaging/README.md).

Running again on the same tag updates the **draft's** assets, allowing Android to
be added later. The workflow will not overwrite a published release. Publish fixes
under a new version and tag. If a required platform fails, fix its build before
publishing; successful individual packages remain available in that run's Artifacts.

After changing build scripts, start **Run workflow** on the updated branch to test
them. **Re-run jobs** uses the original run's commit, and an existing tag still
points to its original source. Neither includes later fixes automatically.

### Persistent Android preview key

Create a key once, outside the repository, using `keytool` from a JDK:

```sh
install -d -m 700 ~/.local/share/hopea-signing
keytool -genkeypair -keystore ~/.local/share/hopea-signing/debug.keystore \
  -storetype JKS -alias androiddebugkey -keyalg RSA -keysize 3072 \
  -validity 10000 -storepass android -keypass android \
  -dname 'CN=Hopea Android Preview'
chmod 600 ~/.local/share/hopea-signing/debug.keystore
```

Back up the key. In the GitHub repository, open **Settings → Secrets and variables
→ Actions → New repository secret**. Set the name to
`ANDROID_PREVIEW_KEYSTORE_BASE64` and its value to the base64 representation of the
keystore. With GitHub CLI installed and authenticated using `gh auth login`, you
can send it without printing the key:

```sh
base64 -w0 ~/.local/share/hopea-signing/debug.keystore | \
  gh secret set ANDROID_PREVIEW_KEYSTORE_BASE64 --repo PuffyWithEyes/Hopea
```

This `base64` command is for Linux. Do not commit the key or share it in chat.
Updating an existing APK requires its original signing key. This pipeline produces
a debug-signed **preview**, not a production Google Play release. Production still
needs a release build and signing configuration.

See [packaging/README.md](../packaging/README.md) for the platform matrix, individual
builds and required settings. Home Manager and other Nix installation methods are
covered in [nix/README.md](../nix/README.md).

## Local builds

```sh
# Run from the project root
nix build .#hopea
bash build-aux/flatpak/build.sh
# Windows: MSYS2 UCRT64
bash build-aux/windows/build.sh
# Then in PowerShell 7: ./packaging/windows/build.ps1
# Android: requires Docker and prepared UI resources
bash build-aux/android/build.sh
```

Scripts place distributable files in `dist/`. Initial builds download dependencies
and can take a long time; Cargo, Nix and Flatpak caches speed up subsequent builds.
Publication to AUR, Flathub and system repositories is not configured yet.

The Windows job produces a portable ZIP and a Setup EXE for Windows 10/11 x64.
See [Windows limitations and local builds](../packaging/windows/README.md).
