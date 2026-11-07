//! Build-time checks for hard-coded install paths.
//!
//! The same package is installed under `~/.local` for one person and under `/`
//! for everyone, so a file that names an absolute location (`ExecStart=
//! /usr/bin/app`, a script that reads `/usr/share/app/data`) works in one mode
//! and breaks in the other. These checks look for that, and for paths that leak
//! from the machine that built the package. They only read bytes: nothing in
//! the package is ever run.
//!
//! The rules are deliberately narrow so a warning is worth reading:
//!
//! * **Own file.** An absolute path that points at a file or folder this
//!   package installs itself (after dropping a leading `usr/` or `usr/local/`).
//!   References to system tools (`/usr/bin/env`, `/etc/ssl`) are fine.
//! * **Build leak.** The directory the package was built from, or a build
//!   machine's home (`/home/<name>`, `/Users/<name>`, `/root`, CI work dirs),
//!   in text files. Binaries are only checked for the build directory itself,
//!   since compilers record paths of their own toolchain everywhere.
//! * **Runpath.** An ELF `RPATH`/`RUNPATH` that is an absolute path outside the
//!   standard library directories.
//! * **Absolute symlink.** A symlink whose target is an absolute path.

use std::{collections::HashSet, fs, io::Read, path::Path};

use anyhow::{Context, Result};
use goblin::Object;

/// Largest part of a single file that is read and scanned.
const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024;
/// A printable run in a binary must be at least this long to be looked at.
const MIN_BINARY_RUN: usize = 8;
/// Findings listed in full before the rest are only counted.
pub const MAX_SHOWN: usize = 40;
/// Where a package may list paths it hard-codes on purpose, one prefix per
/// line (`#` starts a comment).
pub const ALLOW_FILE: &str = ".npack/path-allow.txt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File(Vec<u8>),
    Symlink(String),
}

/// One file, folder or symlink of a package, with a `/`-separated path
/// relative to the package root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    OwnFile,
    BuildLeak,
    Runpath,
    AbsoluteSymlink,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub file: String,
    pub line: Option<usize>,
    pub rule: Rule,
    pub text: String,
}

impl Finding {
    pub fn advice(&self) -> &'static str {
        match self.rule {
            Rule::OwnFile => {
                "this package installs that file itself, so the path is only right for one \
                 install mode; refer to it relative to the program"
            }
            Rule::BuildLeak => "a path from the build machine; it will not exist elsewhere",
            Rule::Runpath => {
                "an absolute library search path; use $ORIGIN-relative paths so it works \
                 wherever the package is installed"
            }
            Rule::AbsoluteSymlink => "an absolute symlink target; make it relative",
        }
    }

    pub fn render(&self) -> String {
        let place = match self.line {
            Some(line) => format!("{}:{line}", self.file),
            None => self.file.clone(),
        };
        format!("  {place}  {}  ({})", self.text, self.advice())
    }
}

pub struct Options<'a> {
    /// The directory the package was built from, when known (`npack pack`).
    pub source_root: Option<&'a str>,
    /// Path prefixes the publisher says are intentional.
    pub allow: &'a [String],
}

/// Reads a package source directory. Files are read up to `MAX_SCAN_BYTES`.
pub fn load_dir(source: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    walk(source, source, &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn walk(root: &Path, current: &Path, entries: &mut Vec<Entry>) -> Result<()> {
    for item in fs::read_dir(current).with_context(|| format!("reading {}", current.display()))? {
        let path = item?.path();
        let relative = relative_path(root, &path);
        let file_type = fs::symlink_metadata(&path)?.file_type();
        if file_type.is_dir() {
            entries.push(Entry {
                path: relative,
                kind: EntryKind::Dir,
            });
            walk(root, &path, entries)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&path)?;
            entries.push(Entry {
                path: relative,
                kind: EntryKind::Symlink(target.to_string_lossy().into_owned()),
            });
        } else if file_type.is_file() {
            let mut bytes = Vec::new();
            fs::File::open(&path)?
                .take(MAX_SCAN_BYTES)
                .read_to_end(&mut bytes)?;
            entries.push(Entry {
                path: relative,
                kind: EntryKind::File(bytes),
            });
        }
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Reads a built `.npk` (a zstd-compressed tar).
pub fn load_npk(archive: &Path) -> Result<Vec<Entry>> {
    let decoder = zstd::Decoder::new(fs::File::open(archive)?)?;
    let mut tar = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry
            .path()?
            .to_string_lossy()
            .trim_start_matches("./")
            .trim_end_matches('/')
            .to_owned();
        if path.is_empty() {
            continue;
        }
        let kind = match entry.header().entry_type() {
            t if t.is_dir() => EntryKind::Dir,
            t if t.is_symlink() => EntryKind::Symlink(
                entry
                    .link_name()?
                    .map(|target| target.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            t if t.is_file() => {
                let mut bytes = Vec::new();
                (&mut entry).take(MAX_SCAN_BYTES).read_to_end(&mut bytes)?;
                EntryKind::File(bytes)
            }
            _ => continue,
        };
        entries.push(Entry { path, kind });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// The prefixes listed in the package's own allow file, if it has one.
pub fn allow_from_entries(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .find(|entry| entry.path == ALLOW_FILE)
        .and_then(|entry| match &entry.kind {
            EntryKind::File(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
            _ => None,
        })
        .map(|text| {
            text.lines()
                .map(|line| line.split('#').next().unwrap_or("").trim())
                .filter(|line| line.starts_with('/'))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Runs every rule over a package. Findings are sorted and de-duplicated, so
/// the output does not depend on directory order.
pub fn check(entries: &[Entry], options: &Options) -> Vec<Finding> {
    let mut own: HashSet<&str> = HashSet::new();
    for entry in entries {
        match &entry.kind {
            EntryKind::File(_) | EntryKind::Symlink(_) => {
                own.insert(entry.path.as_str());
            }
            // Folders count only when they are specific to the package; a
            // reference to `/usr/bin` is a reference to the system's.
            EntryKind::Dir if entry.path.matches('/').count() >= 1 => {
                own.insert(entry.path.as_str());
            }
            EntryKind::Dir => {}
        }
    }
    let mut findings = Vec::new();
    for entry in entries {
        if entry.path.starts_with(".npack/") {
            continue;
        }
        match &entry.kind {
            EntryKind::Dir => {}
            EntryKind::Symlink(target) => {
                if target.starts_with('/') && !allowed(target, options.allow) {
                    findings.push(Finding {
                        file: entry.path.clone(),
                        line: None,
                        rule: Rule::AbsoluteSymlink,
                        text: format!("-> {target}"),
                    });
                }
            }
            EntryKind::File(bytes) => check_file(&entry.path, bytes, &own, options, &mut findings),
        }
    }
    findings.sort();
    findings.dedup();
    findings
}

fn check_file(
    path: &str,
    bytes: &[u8],
    own: &HashSet<&str>,
    options: &Options,
    findings: &mut Vec<Finding>,
) {
    if is_document(path) {
        return;
    }
    let is_elf = bytes.starts_with(b"\x7fELF");
    if is_elf {
        elf_runpaths(path, bytes, options, findings);
    }
    let text = (!is_elf && !bytes.contains(&0))
        .then(|| std::str::from_utf8(bytes).ok())
        .flatten();
    let mut check_line = |line_number: Option<usize>, line: &str| {
        for token in absolute_paths(line) {
            if allowed(token, options.allow) {
                continue;
            }
            // Binaries mention their own toolchain's paths everywhere, so only
            // the directory the package was built from counts for them.
            let leak = if text.is_some() {
                build_leak(token, options.source_root)
            } else {
                options
                    .source_root
                    .is_some_and(|root| root.len() > 1 && under(token, root))
            };
            let rule = if leak {
                Some(Rule::BuildLeak)
            } else if points_at_own_file(token, own) {
                Some(Rule::OwnFile)
            } else {
                None
            };
            if let Some(rule) = rule {
                findings.push(Finding {
                    file: path.to_owned(),
                    line: line_number,
                    rule,
                    text: token.chars().take(120).collect(),
                });
            }
        }
    };
    match text {
        Some(text) => {
            for (index, line) in text.lines().enumerate() {
                check_line(Some(index + 1), line);
            }
        }
        None => {
            for run in printable_runs(bytes) {
                check_line(None, run);
            }
        }
    }
}

/// Paths of files that are about the software, not part of it.
fn is_document(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let documentation_folder = lower.split('/').any(|part| {
        matches!(
            part,
            "doc" | "docs" | "man" | "licenses" | "locale" | "info"
        )
    });
    let extension = lower.rsplit('.').next().unwrap_or("");
    documentation_folder
        || matches!(
            extension,
            "md" | "rst" | "txt" | "html" | "htm" | "pdf" | "adoc" | "mo" | "po" | "1" | "gz"
        )
}

fn allowed(path: &str, allow: &[String]) -> bool {
    allow.iter().any(|prefix| under(path, prefix))
}

/// Whether `path` is `prefix` or inside it (whole path segments only).
fn under(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn build_leak(token: &str, source_root: Option<&str>) -> bool {
    if source_root.is_some_and(|root| root.len() > 1 && under(token, root)) {
        return true;
    }
    const LEAKS: [&str; 6] = [
        "/home/",
        "/Users/",
        "/root/",
        "/github/workspace",
        "/__w/",
        "/builds/",
    ];
    LEAKS.iter().any(|leak| {
        if leak.ends_with('/') {
            // A home or work folder, with something inside it.
            token.starts_with(leak) && token.len() > leak.len()
        } else {
            under(token, leak)
        }
    })
}

/// Whether an absolute path names something this package installs.
fn points_at_own_file(token: &str, own: &HashSet<&str>) -> bool {
    let relative = token.trim_matches('/');
    if relative.is_empty() {
        return false;
    }
    let candidates = [
        Some(relative),
        relative.strip_prefix("usr/local/"),
        relative.strip_prefix("usr/"),
    ];
    candidates
        .into_iter()
        .flatten()
        .filter(|candidate| !candidate.is_empty())
        .any(|candidate| own.contains(candidate))
}

/// Absolute paths mentioned in a line: a `/` at the start or after a
/// delimiter, up to the next delimiter. URLs (`https://...`) are skipped.
fn absolute_paths(line: &str) -> Vec<&str> {
    const START: &[char] = &[' ', '\t', '"', '\'', '=', ':', '(', '<', ',', ';', '[', '{'];
    const END: &[char] = &[
        ' ', '\t', '"', '\'', ';', ')', '>', ',', '|', '&', '\\', ']', '}', '`', '$', ':',
    ];
    let mut found = Vec::new();
    let mut previous: Option<char> = None;
    for (index, ch) in line.char_indices() {
        let starts = ch == '/' && previous.is_none_or(|p| START.contains(&p));
        previous = Some(ch);
        if !starts {
            continue;
        }
        let rest = &line[index..];
        let end = rest.find(END).unwrap_or(rest.len());
        let token = rest[..end].trim_end_matches(['.', ':', '/']);
        if token.len() > 1 && !token.starts_with("//") {
            found.push(token);
        }
    }
    found
}

/// Runs of printable ASCII in a binary file.
fn printable_runs(bytes: &[u8]) -> Vec<&str> {
    let mut runs = Vec::new();
    let mut start = None;
    for (index, byte) in bytes.iter().enumerate() {
        let printable = (0x20..0x7f).contains(byte);
        match (printable, start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                if index - from >= MIN_BINARY_RUN
                    && let Ok(run) = std::str::from_utf8(&bytes[from..index])
                {
                    runs.push(run);
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start
        && bytes.len() - from >= MIN_BINARY_RUN
        && let Ok(run) = std::str::from_utf8(&bytes[from..])
    {
        runs.push(run);
    }
    runs
}

fn elf_runpaths(path: &str, bytes: &[u8], options: &Options, findings: &mut Vec<Finding>) {
    let Ok(Object::Elf(elf)) = Object::parse(bytes) else {
        return;
    };
    for raw in elf.rpaths.iter().chain(elf.runpaths.iter()) {
        for entry in raw.split(':').filter(|entry| !entry.is_empty()) {
            if runpath_is_hard_coded(entry) && !allowed(entry, options.allow) {
                findings.push(Finding {
                    file: path.to_owned(),
                    line: None,
                    rule: Rule::Runpath,
                    text: entry.to_owned(),
                });
            }
        }
    }
}

/// An absolute runpath entry outside the standard library directories.
/// `$ORIGIN`-relative and relative entries are fine.
fn runpath_is_hard_coded(entry: &str) -> bool {
    entry.starts_with('/')
        && !["/lib", "/lib64", "/usr/lib", "/usr/lib64"]
            .iter()
            .any(|standard| under(entry, standard))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> Entry {
        Entry {
            path: path.into(),
            kind: EntryKind::File(text.as_bytes().to_vec()),
        }
    }

    fn dir(path: &str) -> Entry {
        Entry {
            path: path.into(),
            kind: EntryKind::Dir,
        }
    }

    fn run(entries: &[Entry]) -> Vec<Finding> {
        check(
            entries,
            &Options {
                source_root: None,
                allow: &[],
            },
        )
    }

    #[test]
    fn a_unit_naming_the_packages_own_binary_by_system_path_is_flagged_with_its_line() {
        let findings = run(&[
            file("bin/app", "#!/bin/sh\n"),
            file(
                "lib/systemd/user/app.service",
                "[Service]\nExecStart=/usr/bin/app --serve\n",
            ),
        ]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, Rule::OwnFile);
        assert_eq!(findings[0].file, "lib/systemd/user/app.service");
        assert_eq!(findings[0].line, Some(2));
        assert_eq!(findings[0].text, "/usr/bin/app");
    }

    #[test]
    fn system_tools_and_folders_the_package_does_not_own_are_left_alone() {
        let findings = run(&[
            file(
                "bin/app",
                "#!/usr/bin/env sh\nexec /usr/bin/python3 /etc/ssl/certs\n",
            ),
            file("bin/helper", "x"),
            // PATH-style references to /usr/bin are about the system's folder
            // even though the package also has a top-level bin/.
            file("share/app/env", "PATH=/usr/bin:/bin\n"),
        ]);
        assert_eq!(findings, Vec::new());
    }

    #[test]
    fn a_package_specific_folder_counts_but_a_top_level_one_does_not() {
        let entries = [
            dir("bin"),
            dir("share"),
            dir("share/app"),
            file("share/app/data.json", "{}"),
            file(
                "bin/run",
                "DATA=/usr/share/app/data.json\nDIR=/usr/share/app\nBIN=/usr/bin\n",
            ),
        ];
        let findings = run(&entries);
        let texts: Vec<_> = findings.iter().map(|f| f.text.as_str()).collect();
        assert_eq!(
            texts,
            ["/usr/share/app/data.json", "/usr/share/app"],
            "the file (line 1) and the folder (line 2), not /usr/bin"
        );
    }

    #[test]
    fn urls_are_not_paths() {
        let findings = run(&[
            file("share/app/x", "y"),
            file("bin/run", "curl https://example.com/usr/share/app/x\n"),
        ]);
        assert_eq!(findings, Vec::new());
    }

    #[test]
    fn build_machine_paths_are_flagged_in_scripts_but_not_in_documents() {
        let findings = run(&[
            file("bin/run", "ROOT=/home/alice/work/app\n"),
            file("share/doc/app/README", "see /home/alice/work\n"),
            file("README.md", "see /home/alice/work\n"),
            file("bin/ci", "cd /github/workspace\nls /home/\n"),
        ]);
        let hits: Vec<_> = findings
            .iter()
            .map(|f| (f.file.as_str(), f.rule, f.text.as_str()))
            .collect();
        assert_eq!(
            hits,
            [
                ("bin/ci", Rule::BuildLeak, "/github/workspace"),
                ("bin/run", Rule::BuildLeak, "/home/alice/work/app"),
            ]
        );
    }

    #[test]
    fn the_directory_the_package_was_built_from_is_a_leak_even_in_a_binary() {
        let mut binary = b"\x00\x01junk\x00".to_vec();
        binary.extend_from_slice(b"/srv/build/app-1.0/share/data");
        binary.extend_from_slice(b"\x00");
        binary.extend_from_slice(b"/home/someone/.cargo/registry/src/x.rs\x00");
        let entries = [Entry {
            path: "bin/app".into(),
            kind: EntryKind::File(binary),
        }];
        let with_root = check(
            &entries,
            &Options {
                source_root: Some("/srv/build/app-1.0"),
                allow: &[],
            },
        );
        assert_eq!(with_root.len(), 1);
        assert_eq!(with_root[0].rule, Rule::BuildLeak);
        assert_eq!(with_root[0].line, None);
        // Without the build directory, the toolchain paths every compiler
        // records are not reported.
        assert_eq!(run(&entries), Vec::new());
    }

    #[test]
    fn allowed_prefixes_silence_findings_by_whole_segment() {
        let entries = [
            file("bin/app", "x"),
            file("lib/app.service", "ExecStart=/usr/bin/app\n"),
        ];
        assert_eq!(run(&entries).len(), 1);
        let allowed = check(
            &entries,
            &Options {
                source_root: None,
                allow: &["/usr/bin/app".into()],
            },
        );
        assert_eq!(allowed, Vec::new());
        // "/usr/bin/application" is not inside "/usr/bin/app".
        assert!(under("/usr/bin/app/x", "/usr/bin/app"));
        assert!(!under("/usr/bin/application", "/usr/bin/app"));
    }

    #[test]
    fn the_allow_file_is_read_from_the_package() {
        let entries = [file(
            ALLOW_FILE,
            "# intentional\n/usr/bin/app  # the launcher\n\nnot-a-path\n/opt/vendor\n",
        )];
        assert_eq!(
            allow_from_entries(&entries),
            ["/usr/bin/app".to_owned(), "/opt/vendor".to_owned()]
        );
        assert!(allow_from_entries(&[]).is_empty());
        // The allow file itself and the manifest are never scanned.
        assert_eq!(
            run(&[
                file("bin/app", "x"),
                file(ALLOW_FILE, "/usr/bin/app\n"),
                file(".npack/manifest.json", "{\"x\": \"/usr/bin/app\"}"),
            ]),
            Vec::new()
        );
    }

    #[test]
    fn absolute_symlinks_are_flagged_and_relative_ones_are_not() {
        let link = |path: &str, target: &str| Entry {
            path: path.into(),
            kind: EntryKind::Symlink(target.into()),
        };
        let findings = run(&[link("bin/a", "/usr/bin/real"), link("bin/b", "../lib/real")]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, Rule::AbsoluteSymlink);
        assert_eq!(findings[0].file, "bin/a");
    }

    #[test]
    fn only_non_standard_absolute_runpaths_are_hard_coded() {
        for fine in [
            "$ORIGIN/../lib",
            "$ORIGIN",
            "lib",
            "/usr/lib",
            "/usr/lib/x86_64-linux-gnu",
            "/lib64",
        ] {
            assert!(!runpath_is_hard_coded(fine), "{fine}");
        }
        for bad in [
            "/opt/app/lib",
            "/home/me/build/lib",
            "/usr/local/lib",
            "/usr/libexec/x",
        ] {
            assert!(runpath_is_hard_coded(bad), "{bad}");
        }
    }

    #[test]
    fn path_tokens_stop_at_delimiters() {
        assert_eq!(
            absolute_paths("Exec=/usr/bin/app --flag=/opt/x \"/etc/y\"; (/var/z)"),
            ["/usr/bin/app", "/opt/x", "/etc/y", "/var/z"]
        );
        assert!(absolute_paths("see //double and https://h/p and a/b").is_empty());
        assert_eq!(absolute_paths("cd /srv/app."), ["/srv/app"]);
    }

    #[test]
    fn strict_packing_refuses_before_writing_anything_and_plain_packing_only_warns() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("src");
        fs::create_dir_all(source.join("bin"))?;
        fs::create_dir_all(source.join("lib"))?;
        fs::write(source.join("bin/app"), "#!/bin/sh\n")?;
        fs::write(source.join("lib/app.service"), "ExecStart=/usr/bin/app\n")?;

        let strict_output = dir.path().join("strict.npk");
        let error = crate::pack_with_path_checks(&source, &strict_output, true, &[]).unwrap_err();
        assert!(error.to_string().contains("--strict"));
        assert!(
            !strict_output.exists(),
            "nothing is written when strict fails"
        );

        let plain_output = dir.path().join("plain.npk");
        crate::pack_with_path_checks(&source, &plain_output, false, &[])?;
        assert!(plain_output.exists());

        // Allowing the path, on the command line, makes strict succeed.
        let allowed_output = dir.path().join("allowed.npk");
        crate::pack_with_path_checks(&source, &allowed_output, true, &["/usr/bin/app".into()])?;
        assert!(allowed_output.exists());

        let relative =
            crate::pack_with_path_checks(&source, &allowed_output, false, &["usr".into()]);
        assert!(relative.unwrap_err().to_string().contains("absolute"));
        Ok(())
    }

    #[test]
    fn a_directory_and_a_built_archive_give_the_same_findings() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("src");
        fs::create_dir_all(source.join("bin"))?;
        fs::create_dir_all(source.join("lib/systemd/user"))?;
        fs::write(source.join("bin/app"), "#!/bin/sh\n")?;
        fs::write(
            source.join("lib/systemd/user/app.service"),
            "[Service]\nExecStart=/usr/bin/app\n",
        )?;
        let from_dir = load_dir(&source)?;
        let npk = dir.path().join("app.npk");
        crate::pack_npk(&source, &npk)?;
        let from_archive = load_npk(&npk)?;
        assert_eq!(run(&from_dir), run(&from_archive));
        assert_eq!(run(&from_dir).len(), 1);
        Ok(())
    }
}
