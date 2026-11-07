//! Opening an installed app from the Installed tab.
//!
//! Packages that ship a freedesktop.org launcher (a `.desktop` file) can be
//! opened with one click. This reads the launcher's `Exec` line, drops the
//! placeholders for files and URLs (`%f`, `%U`, ...), and starts the program
//! detached from the window, so closing npack never closes the app. Nothing
//! runs through a shell, and launchers that need a terminal, or that are
//! hidden from menus, are not offered.

use std::{
    collections::HashMap,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Launchers bigger than this are ignored.
const MAX_LAUNCHER_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    /// The program and its arguments, placeholders removed.
    pub argv: Vec<String>,
}

/// The launcher described by a `.desktop` file, if it can be started on its own.
pub fn parse_desktop(contents: &str) -> Option<Launcher> {
    let mut in_entry = false;
    let mut values: HashMap<&str, &str> = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            // Keys with a locale (Name[fr]) are never needed here.
            values.entry(key.trim()).or_insert_with(|| value.trim());
        }
    }
    let flag = |key: &str| {
        values
            .get(key)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    };
    if values.get("Type").copied() != Some("Application")
        || flag("Terminal")
        || flag("NoDisplay")
        || flag("Hidden")
    {
        return None;
    }
    let argv = split_exec(values.get("Exec")?)?;
    (!argv.is_empty()).then_some(Launcher { argv })
}

/// Splits an `Exec` value the way the specification describes: words
/// separated by spaces, double quotes group (with backslash escapes inside),
/// `%%` is a percent sign and every other `%x` placeholder is dropped.
/// None for an unbalanced quote.
pub fn split_exec(exec: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut in_quotes = false;
    let mut chars = exec.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            '\\' if in_quotes => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    started = true;
                }
            }
            '%' => {
                // `%%` is a percent sign; any other `%x` is a file, URL or icon
                // placeholder, and nothing is passed for it.
                if chars.next() == Some('%') {
                    current.push('%');
                    started = true;
                }
            }
            ' ' | '\t' if !in_quotes => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            other => {
                current.push(other);
                started = true;
            }
        }
    }
    if in_quotes {
        return None;
    }
    if started {
        words.push(current);
    }
    Some(words)
}

/// The first usable launcher among an installed package's files (the
/// `.desktop` files under an `applications` folder).
pub fn find_launcher(files: &[String]) -> Option<Launcher> {
    files
        .iter()
        .map(Path::new)
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "desktop")
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|dir| dir == "applications")
        })
        .filter(|path| std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_LAUNCHER_BYTES))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .find_map(|contents| parse_desktop(&contents))
}

/// Starts the launcher detached from this process. `extra_path` folders are
/// searched first (a per-user install puts programs in `~/.local/bin`, which a
/// desktop session does not always have on its path).
pub fn start(launcher: &Launcher, extra_path: &[PathBuf]) -> std::io::Result<()> {
    let (program, arguments) = launcher
        .argv
        .split_first()
        .ok_or_else(|| std::io::Error::other("the launcher has no command"))?;
    let mut folders: Vec<PathBuf> = extra_path.to_vec();
    if let Some(path) = std::env::var_os("PATH") {
        folders.extend(std::env::split_paths(&path));
    }
    let search_path = std::env::join_paths(&folders).map_err(std::io::Error::other)?;
    let mut child = Command::new(program)
        .args(arguments)
        .env("PATH", &search_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    // Reap it when it exits, so finished apps do not pile up as zombies.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = "# comment\n[Desktop Entry]\nType=Application\nName=Hello\nName[fr]=Salut\nExec=hello --flag %U\nIcon=hello\n";

    #[test]
    fn a_plain_application_launcher_is_understood() {
        let launcher = parse_desktop(ENTRY).unwrap();
        assert_eq!(launcher.argv, ["hello", "--flag"]);
    }

    #[test]
    fn launchers_that_cannot_be_started_alone_are_not_offered() {
        for extra in [
            "Terminal=true",
            "NoDisplay=true",
            "Hidden=true",
            "Type=Link",
        ] {
            let text = format!("{ENTRY}{extra}\n").replace("Type=Application\n", "");
            let text = if extra.starts_with("Type") {
                text
            } else {
                format!("{text}Type=Application\n")
            };
            assert_eq!(parse_desktop(&text), None, "{extra}");
        }
        assert_eq!(parse_desktop("[Desktop Entry]\nType=Application\n"), None);
        // Keys in other groups do not count.
        let other = "[Desktop Action x]\nExec=evil\n[Desktop Entry]\nType=Application\nExec=good\n";
        assert_eq!(parse_desktop(other).unwrap().argv, ["good"]);
    }

    #[test]
    fn exec_lines_follow_the_specification() {
        let cases: &[(&str, &[&str])] = &[
            ("app", &["app"]),
            ("app %f %F %u %U %i %c %k", &["app"]),
            (
                r#""/opt/My App/run" --name "a b""#,
                &["/opt/My App/run", "--name", "a b"],
            ),
            ("app 100%%", &["app", "100%"]),
            (r#"app "say \"hi\"""#, &["app", r#"say "hi""#]),
            ("  spaced   out  ", &["spaced", "out"]),
            (r#"app """#, &["app", ""]),
        ];
        for (exec, expected) in cases {
            assert_eq!(split_exec(exec).unwrap(), *expected, "{exec}");
        }
        assert_eq!(split_exec(r#"app "unbalanced"#), None);
    }

    #[test]
    fn only_launchers_in_an_applications_folder_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("share/applications");
        std::fs::create_dir_all(&apps).unwrap();
        let elsewhere = dir.path().join("share/other");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(apps.join("a.desktop"), ENTRY).unwrap();
        std::fs::write(elsewhere.join("b.desktop"), ENTRY).unwrap();
        let files = |names: &[&Path]| -> Vec<String> {
            names
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        };
        let found = find_launcher(&files(&[
            &elsewhere.join("b.desktop"),
            &apps.join("a.desktop"),
        ]));
        assert_eq!(found.unwrap().argv, ["hello", "--flag"]);
        assert_eq!(find_launcher(&files(&[&elsewhere.join("b.desktop")])), None);
        assert_eq!(find_launcher(&[]), None);
        assert_eq!(
            find_launcher(&files(&[&apps.join("missing.desktop")])),
            None
        );
    }

    #[test]
    fn a_launcher_is_started_with_the_extra_folders_on_its_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let marker = dir.path().join("ran");
        let script = bin.join("myapp-test-launch");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$1\" > '{}'\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let launcher = Launcher {
            argv: vec!["myapp-test-launch".into(), "opened".into()],
        };
        start(&launcher, std::slice::from_ref(&bin)).unwrap();
        for _ in 0..100 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "opened");
        // A program that does not exist is an error, not a silent nothing.
        let missing = Launcher {
            argv: vec!["definitely-not-a-program-xyz".into()],
        };
        assert!(start(&missing, &[]).is_err());
    }
}
