// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Desktop-category detection shared by Linux and the four BSD families.
//! No shells, process arguments, environment dumps or elevated privileges.
use gtk::glib;
use std::{
    collections::HashSet,
    fs,
    io::Read,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const MAX_OUTPUT: usize = 128 * 1024;
const MAX_DESKTOP: u64 = 64 * 1024;
static GAMES: OnceLock<Mutex<Option<(Instant, HashSet<String>)>>> = OnceLock::new();

pub(super) fn detect() -> bool {
    let mut cache = GAMES
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(60))
    {
        #[allow(unused_mut)] // BSD adds its ports/pkgsrc installation prefixes.
        let mut dirs = std::iter::once(glib::user_data_dir())
            .chain(glib::system_data_dirs())
            .map(|p| p.join("applications"))
            .collect::<Vec<_>>();
        // pkgsrc's prefix is not included in GLib's generic XDG defaults.
        #[cfg(any(
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        for path in [
            "/usr/local/share/applications",
            "/usr/pkg/share/applications",
        ] {
            let path = PathBuf::from(path);
            if !dirs.contains(&path) {
                dirs.push(path);
            }
        }
        *cache = Some((Instant::now(), installed_games(&dirs)));
    }
    let games = &cache.as_ref().unwrap().1;
    if games.is_empty() {
        return false;
    }
    // These options are supported by FreeBSD, OpenBSD, NetBSD and DragonFly ps.
    // -x keeps the default current-user selection, -c excludes arguments.
    let mut command = Command::new("ps");
    command.args(["-x", "-c", "-o", "comm="]).env("LC_ALL", "C");
    bounded_output(&mut command, Duration::from_millis(500))
        .is_some_and(|output| matching_process(&output, games))
}

fn matching_process(output: &[u8], games: &HashSet<String>) -> bool {
    let Ok(text) = std::str::from_utf8(output) else {
        return false;
    };
    text.lines()
        .take(4096)
        .filter_map(|s| executable_name(s.trim()))
        .any(|s| games.contains(&s))
}

fn executable_name(value: &str) -> Option<String> {
    let name = Path::new(value).file_name()?.to_str()?;
    if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
        return None;
    }
    // A store, interpreter or launcher staying open does not mean a game runs.
    if matches!(
        name,
        "steam"
            | "steamwebhelper"
            | "lutris"
            | "heroic"
            | "env"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "flatpak"
            | "snap"
            | "wine"
            | "wine64"
            | "python"
            | "python3"
            | "java"
            | "mono"
            | "gamemoderun"
            | "hopea"
    ) {
        return None;
    }
    Some(name.to_owned())
}

fn desktop_game(text: &str) -> Option<String> {
    let file = glib::KeyFile::new();
    file.load_from_data(text, glib::KeyFileFlags::NONE).ok()?;
    let section = "Desktop Entry";
    if file.string(section, "Type").ok()?.as_str() != "Application"
        || file.boolean(section, "Hidden").unwrap_or(false)
        || !file
            .string(section, "Categories")
            .ok()?
            .split(';')
            .any(|s| s == "Game")
    {
        return None;
    }
    let exec = file.string(section, "Exec").ok()?;
    let args = glib::shell_parse_argv(exec.as_str()).ok()?;
    let mut args = args.iter();
    let mut command = args.next()?.to_str()?;
    // Parse simple env assignments without ever executing the desktop entry.
    if Path::new(command).file_name()?.to_str()? == "env" {
        command = args.find_map(|a| a.to_str().filter(|s| !s.contains('=')))?;
        if command.starts_with('-') {
            return None;
        }
    }
    executable_name(command)
}

fn installed_games(dirs: &[PathBuf]) -> HashSet<String> {
    let mut games = HashSet::new();
    let mut seen = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut count = 0;
    for dir in dirs.iter().take(32) {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            count += 1;
            if count > 2048 || Instant::now() >= deadline {
                return games;
            }
            let path = entry.path();
            if path.extension().is_none_or(|s| s != "desktop") || !seen.insert(entry.file_name()) {
                continue;
            }
            // Refuse devices/FIFOs and oversized entries; do not block on a FIFO
            // swapped in between the directory listing and opening the file.
            let Ok(file) = fs::OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(path)
            else {
                continue;
            };
            if !file
                .metadata()
                .is_ok_and(|m| m.is_file() && m.len() <= MAX_DESKTOP)
            {
                continue;
            }
            let mut text = String::new();
            if file
                .take(MAX_DESKTOP + 1)
                .read_to_string(&mut text)
                .is_err()
                || text.len() as u64 > MAX_DESKTOP
            {
                continue;
            }
            if let Some(game) = desktop_game(&text) {
                games.insert(game);
            }
        }
    }
    games
}

fn bounded_output(command: &mut Command, timeout: Duration) -> Option<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut pipe = child.stdout.take()?;
    let flags = unsafe { nix::libc::fcntl(pipe.as_raw_fd(), nix::libc::F_GETFL) };
    if flags < 0
        || unsafe {
            nix::libc::fcntl(
                pipe.as_raw_fd(),
                nix::libc::F_SETFL,
                flags | nix::libc::O_NONBLOCK,
            )
        } < 0
    {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let deadline = Instant::now() + timeout;
    let mut data = Vec::new();
    let mut bytes = [0; 4096];
    let result = loop {
        if Instant::now() >= deadline {
            break None;
        }
        match pipe.read(&mut bytes) {
            Ok(0) => match child.try_wait() {
                Ok(Some(status)) => break status.success().then_some(data),
                Err(_) => break None,
                Ok(None) => {}
            },
            Ok(n) => {
                if data.len() + n > MAX_OUTPUT {
                    break None;
                }
                data.extend_from_slice(&bytes[..n]);
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break None,
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Also reap failed/timed-out helpers; no accumulating worker processes.
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_overrides_and_file_size_limits_are_respected() {
        let root = glib::mkdtemp(std::env::temp_dir().join("hopea-game-desktop-XXXXXX")).unwrap();
        let user = root.join("user");
        let system = root.join("system");
        fs::create_dir(&user).unwrap();
        fs::create_dir(&system).unwrap();
        let valid = "[Desktop Entry]\nType=Application\nCategories=Game;\nExec=supertuxkart\n";
        fs::write(system.join("game.desktop"), valid).unwrap();
        let dirs = [user.clone(), system];
        assert!(installed_games(&dirs).contains("supertuxkart"));
        fs::write(user.join("game.desktop"), "[Desktop Entry]\nHidden=true\n").unwrap();
        assert!(installed_games(&dirs).is_empty());
        fs::write(
            user.join("huge.desktop"),
            format!("{valid}#{}", "x".repeat(MAX_DESKTOP as usize)),
        )
        .unwrap();
        assert!(installed_games(&dirs).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn categories_and_process_names_must_match_exactly() {
        let entry = |exec: &str, category: &str| {
            format!("[Desktop Entry]\nType=Application\nCategories={category}\nExec={exec}\n")
        };
        assert_eq!(
            desktop_game(&entry(
                "/usr/local/bin/supertuxkart --fullscreen",
                "Game;ArcadeGame;"
            )),
            Some("supertuxkart".into())
        );
        assert_eq!(
            desktop_game(&entry("env FOO=bar /opt/game %f", "Game;")),
            Some("game".into())
        );
        assert_eq!(
            desktop_game(&entry("\"/opt/My Game\" %u", "Game;")),
            Some("My Game".into())
        );
        for (exec, category) in [
            ("steam -applaunch 730", "Game;"),
            ("java -jar game.jar", "Game;"),
            ("editor", "NotGame;"),
            ("sh -c evil", "Game;"),
        ] {
            assert!(desktop_game(&entry(exec, category)).is_none());
        }
        assert!(desktop_game(&(entry("game", "Game;") + "Hidden=true\n")).is_none());
        let games = HashSet::from(["game".into()]);
        assert!(matching_process(b"/usr/local/bin/game\n", &games));
        assert!(!matching_process(b"notgame\ngame-helper\n", &games));
    }
    #[test]
    fn helper_output_and_runtime_are_bounded() {
        let mut normal = Command::new("printf");
        normal.arg("game\n");
        assert_eq!(
            bounded_output(&mut normal, Duration::from_secs(1)).unwrap(),
            b"game\n"
        );
        let mut flood = Command::new("yes");
        assert!(bounded_output(&mut flood, Duration::from_secs(1)).is_none());
        let start = Instant::now();
        let mut slow = Command::new("sleep");
        slow.arg("5");
        assert!(bounded_output(&mut slow, Duration::from_millis(50)).is_none());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
