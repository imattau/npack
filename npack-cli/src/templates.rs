//! Install-time placeholders: `{{prefix}}`.
//!
//! The same package is installed under `~/.local` for one person and under `/`
//! for everyone, so a service file, launcher or wrapper script cannot name its
//! own files by a fixed absolute path. A package lists the text files that need
//! the real location in `.npack/templates.txt` (one package-relative path per
//! line, `#` starts a comment), and writes `{{prefix}}` where the install
//! location belongs:
//!
//! ```text
//! ExecStart={{prefix}}/bin/app --serve
//! ```
//!
//! When the package is installed, npack replaces `{{prefix}}` in exactly the
//! listed files with the prefix it is installing under (no trailing slash; for
//! an install at `/` that is the empty string, so `{{prefix}}/bin/app` becomes
//! `/bin/app`). Nothing else is expanded: no environment, no commands, and a
//! file that is not listed is installed untouched. The list is inside the
//! archive, so it is covered by the same signed hash as the files it names, and
//! the stored archive stays pristine (a reinstall or rollback substitutes again
//! for wherever it lands).

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

pub const TEMPLATES_FILE: &str = ".npack/templates.txt";
pub const PLACEHOLDER: &str = "{{prefix}}";
/// A template larger than this is refused: it is a service file or a script,
/// not a data file.
pub const MAX_TEMPLATE_BYTES: u64 = 1024 * 1024;

/// The package-relative paths named by a templates list. Blank lines and
/// comments are ignored; an unsafe or reserved path is an error, because the
/// publisher meant something that cannot be honoured.
pub fn parse_list(text: &str) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let path = PathBuf::from(line);
        if !crate::is_safe_relative_path(&path) {
            bail!(
                "{TEMPLATES_FILE} line {}: {line} must be a path inside the package",
                index + 1
            );
        }
        if path.starts_with(".npack") {
            bail!(
                "{TEMPLATES_FILE} line {}: {line} is npack's own file and cannot be a template",
                index + 1
            );
        }
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Checks a package's templates list against its contents at pack time, so a
/// mistake is found by the publisher and not by whoever installs it. The same
/// things `apply_to_staging` would refuse at install.
pub fn validate_entries(entries: &[crate::pathcheck::Entry]) -> Result<()> {
    use crate::pathcheck::EntryKind;
    let Some(list) = entries.iter().find(|entry| entry.path == TEMPLATES_FILE) else {
        return Ok(());
    };
    let EntryKind::File(bytes) = &list.kind else {
        bail!("{TEMPLATES_FILE} must be a regular file");
    };
    let text =
        std::str::from_utf8(bytes).map_err(|_| anyhow::anyhow!("{TEMPLATES_FILE} is not text"))?;
    for path in parse_list(text)? {
        let name = path.to_string_lossy().replace('\\', "/");
        let Some(entry) = entries.iter().find(|entry| entry.path == name) else {
            bail!("template {name} is listed in {TEMPLATES_FILE} but is not in the package");
        };
        match &entry.kind {
            EntryKind::File(bytes) => {
                if bytes.len() as u64 > MAX_TEMPLATE_BYTES {
                    bail!("template {name} is larger than {MAX_TEMPLATE_BYTES} bytes");
                }
                if std::str::from_utf8(bytes).is_err() {
                    bail!("template {name} is not text");
                }
            }
            _ => bail!("template {name} must be a regular file, not a link or folder"),
        }
    }
    Ok(())
}

/// The text to put where `{{prefix}}` was. An install at `/` gives the empty
/// string, so `{{prefix}}/bin/app` reads as `/bin/app`.
pub fn prefix_text(prefix: &Path) -> Result<String> {
    let absolute = std::path::absolute(prefix)
        .with_context(|| format!("resolving install prefix {}", prefix.display()))?;
    let text = absolute
        .to_str()
        .context("the install prefix is not valid text")?;
    if text.chars().any(char::is_control) {
        bail!("the install prefix contains control characters");
    }
    Ok(text.trim_end_matches('/').to_owned())
}

/// Replaces every `{{prefix}}` in `text`. Other `{{...}}` text is left alone.
pub fn render(text: &str, prefix: &str) -> String {
    text.replace(PLACEHOLDER, prefix)
}

/// Fills in the placeholders of the templates a staged package lists, in
/// place, and returns the files it changed. A package without a templates
/// list is untouched. Anything wrong with a listed file (missing, a symlink,
/// not text, too big) stops the install before anything is written to the
/// prefix.
pub fn apply_to_staging(staging: &Path, prefix: &Path) -> Result<Vec<PathBuf>> {
    let list_path = staging.join(TEMPLATES_FILE);
    let list = match fs::read_to_string(&list_path) {
        Ok(text) => parse_list(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", list_path.display()));
        }
    };
    let prefix = prefix_text(prefix)?;
    let mut changed = Vec::new();
    for relative in list {
        let path = staging.join(&relative);
        let metadata = fs::symlink_metadata(&path).with_context(|| {
            format!(
                "template {} is listed in {TEMPLATES_FILE} but is not in the package",
                relative.display()
            )
        })?;
        if !metadata.file_type().is_file() {
            bail!(
                "template {} must be a regular file, not a link or folder",
                relative.display()
            );
        }
        let mut bytes = Vec::new();
        fs::File::open(&path)?
            .take(MAX_TEMPLATE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_TEMPLATE_BYTES {
            bail!(
                "template {} is larger than {MAX_TEMPLATE_BYTES} bytes",
                relative.display()
            );
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("template {} is not text", relative.display()))?;
        if !text.contains(PLACEHOLDER) {
            continue;
        }
        fs::write(&path, render(&text, &prefix))?;
        fs::set_permissions(&path, metadata.permissions())?;
        changed.push(relative);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_list_ignores_comments_and_rejects_paths_that_cannot_be_honoured() {
        let list = parse_list("# services\nlib/app.service  # unit\n\nbin/run\nbin/run\n").unwrap();
        assert_eq!(
            list,
            [PathBuf::from("lib/app.service"), PathBuf::from("bin/run")]
        );
        for bad in [
            "/etc/passwd",
            "../x",
            "a/../../x",
            ".npack/manifest.json",
            ".npack",
        ] {
            assert!(parse_list(bad).is_err(), "{bad}");
        }
        assert!(parse_list("").unwrap().is_empty());
    }

    #[test]
    fn the_prefix_has_no_trailing_slash_and_root_is_empty() {
        assert_eq!(prefix_text(Path::new("/")).unwrap(), "");
        assert_eq!(
            prefix_text(Path::new("/home/me/.local")).unwrap(),
            "/home/me/.local"
        );
        assert_eq!(prefix_text(Path::new("/opt/x/")).unwrap(), "/opt/x");
        assert!(prefix_text(Path::new("/bad\nprefix")).is_err());
        // A relative store becomes absolute, so the installed file is not
        // tied to wherever npack happened to be run from.
        assert!(
            prefix_text(Path::new("store/packages"))
                .unwrap()
                .starts_with('/')
        );
    }

    #[test]
    fn only_the_prefix_placeholder_is_replaced() {
        assert_eq!(
            render(
                "a={{prefix}}/bin b={{prefix}}/lib c={{home}} d={{ prefix }}",
                "/p"
            ),
            "a=/p/bin b=/p/lib c={{home}} d={{ prefix }}"
        );
        assert_eq!(
            render("ExecStart={{prefix}}/bin/app", ""),
            "ExecStart=/bin/app"
        );
    }

    fn stage(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, text).unwrap();
        }
        dir
    }

    #[test]
    fn listed_files_are_rewritten_and_everything_else_is_left_alone() {
        let dir = stage(&[
            (TEMPLATES_FILE, "lib/app.service\nbin/plain\n"),
            ("lib/app.service", "ExecStart={{prefix}}/bin/app\n"),
            ("bin/plain", "no placeholder here\n"),
            ("share/notes", "{{prefix}} stays literal: not listed\n"),
        ]);
        let script = dir.path().join("lib/app.service");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o750)).unwrap();
        let changed = apply_to_staging(dir.path(), Path::new("/home/me/.local")).unwrap();
        assert_eq!(changed, [PathBuf::from("lib/app.service")]);
        assert_eq!(
            fs::read_to_string(&script).unwrap(),
            "ExecStart=/home/me/.local/bin/app\n"
        );
        assert_eq!(
            fs::metadata(&script).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("share/notes")).unwrap(),
            "{{prefix}} stays literal: not listed\n"
        );
    }

    #[test]
    fn a_system_install_gives_paths_from_the_root() {
        let dir = stage(&[
            (TEMPLATES_FILE, "lib/app.service\n"),
            ("lib/app.service", "ExecStart={{prefix}}/usr/bin/app\n"),
        ]);
        apply_to_staging(dir.path(), Path::new("/")).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("lib/app.service")).unwrap(),
            "ExecStart=/usr/bin/app\n"
        );
    }

    #[test]
    fn a_package_without_a_list_is_untouched() {
        let dir = stage(&[("bin/app", "{{prefix}}\n")]);
        assert!(
            apply_to_staging(dir.path(), Path::new("/p"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("bin/app")).unwrap(),
            "{{prefix}}\n"
        );
    }

    #[test]
    fn pack_time_validation_finds_what_install_would_refuse() {
        use crate::pathcheck::{Entry, EntryKind};
        let file = |path: &str, bytes: &[u8]| Entry {
            path: path.into(),
            kind: EntryKind::File(bytes.to_vec()),
        };
        assert!(validate_entries(&[]).is_ok());
        let good = [
            file(TEMPLATES_FILE, b"lib/a.service\n"),
            file("lib/a.service", b"x"),
        ];
        assert!(validate_entries(&good).is_ok());
        let missing = [file(TEMPLATES_FILE, b"lib/gone\n")];
        assert!(validate_entries(&missing).is_err());
        let unsafe_path = [file(TEMPLATES_FILE, b"../x\n")];
        assert!(validate_entries(&unsafe_path).is_err());
        let binary = [file(TEMPLATES_FILE, b"b\n"), file("b", &[0xff, 0xfe])];
        assert!(validate_entries(&binary).is_err());
        let link = [
            file(TEMPLATES_FILE, b"l\n"),
            Entry {
                path: "l".into(),
                kind: EntryKind::Symlink("x".into()),
            },
        ];
        assert!(validate_entries(&link).is_err());
    }

    #[test]
    fn a_broken_template_stops_the_install() {
        let missing = stage(&[(TEMPLATES_FILE, "lib/gone.service\n")]);
        let error = apply_to_staging(missing.path(), Path::new("/p")).unwrap_err();
        assert!(error.to_string().contains("not in the package"));

        let binary = tempfile::tempdir().unwrap();
        fs::create_dir_all(binary.path().join(".npack")).unwrap();
        fs::write(binary.path().join(TEMPLATES_FILE), "bin/blob\n").unwrap();
        fs::create_dir_all(binary.path().join("bin")).unwrap();
        fs::write(binary.path().join("bin/blob"), [0xff, 0xfe, 0x00]).unwrap();
        let error = apply_to_staging(binary.path(), Path::new("/p")).unwrap_err();
        assert!(error.to_string().contains("not text"));

        let linked = stage(&[(TEMPLATES_FILE, "bin/link\n"), ("real", "x")]);
        fs::create_dir_all(linked.path().join("bin")).unwrap();
        std::os::unix::fs::symlink("../real", linked.path().join("bin/link")).unwrap();
        let error = apply_to_staging(linked.path(), Path::new("/p")).unwrap_err();
        assert!(error.to_string().contains("regular file"));

        let big = stage(&[(TEMPLATES_FILE, "bin/big\n")]);
        fs::create_dir_all(big.path().join("bin")).unwrap();
        fs::write(
            big.path().join("bin/big"),
            vec![b'a'; MAX_TEMPLATE_BYTES as usize + 1],
        )
        .unwrap();
        let error = apply_to_staging(big.path(), Path::new("/p")).unwrap_err();
        assert!(error.to_string().contains("larger than"));
    }
}
